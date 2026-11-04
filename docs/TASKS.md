# 任务与并行

本文档定义 Agent 模式下"用户下一条命令"所对应的实体，以及多任务并行、隔离、合并的协议。对应里程碑 P4。

## 1. 为什么需要 Task 这个实体

会话（Session）回答"这段对话聊了什么"，任务（Task）回答"这件事做完了没有"。两者不是一回事：

- 同一个会话可以推进多个任务；
- 一个任务也可能横跨多次会话（今天规划、明天继续）。

UI 上左侧的任务列表、右侧的变更清单、状态栏的"2 待确认"，全部挂在 Task 上。所以它必须是 Rust 侧的一等对象，不能只是前端状态。

## 2. 数据结构

```rust
pub struct Task {
    pub id: TaskId,                 // ulid
    pub title: String,              // 用户那句话的摘要，可编辑
    pub session_id: SessionId,
    pub status: TaskStatus,
    pub worktree: Option<Worktree>, // 非 git 项目为 None
    pub port: Option<u16>,          // 分配的预览端口
    pub steps: Vec<Step>,           // 计划模式产出，或运行时逐步追加
    pub changeset: Vec<FileChange>,
    pub checks: Vec<Check>,
    pub created_at: i64,
    pub base_sha: Option<String>,   // worktree 的基线提交
}

pub enum TaskStatus {
    Planning,   // 制定计划中
    Running,    // 执行中
    Blocked,    // 等待审批或用户输入
    Review,     // 待验收
    Done,       // 已应用
    Failed,     // 失败或已中止
    Queued,     // 排队（并发上限）
}

pub struct Worktree {
    pub path: PathBuf,       // <数据目录>/worktrees/<projectId>/<taskId>
    pub branch: String,      // lc/<taskId>-<slug>
    pub base_sha: String,
}

pub struct FileChange {
    pub path: PathBuf,
    pub status: ChangeStatus, // Modified / Added / Deleted / Renamed
    pub applied: bool,
    pub stats: (u32, u32),    // +/-
}

pub struct Check {
    pub name: String,         // "unit" / "typecheck" / "lint"
    pub ok: bool,
    pub output_tail: String,  // 最后 50 行
    pub duration_ms: u64,
}
```

`changeset` 与 `checks` 由工具执行事件驱动增量更新，不靠结束后整体扫描——否则长任务过程中右侧一直是空的。

## 3. worktree 生命周期

```
创建任务
  └─ project.is_git ?
       ├─ 是 → git worktree add <数据目录>/worktrees/<pid>/<tid> -b lc/<tid>-<slug> HEAD
       │        └─ 失败（分支已存在 / 无提交 / 只读 fs）→ 降级为串行任务，标记原因
       └─ 否 → worktree = None，串行执行，UI 标"串行 · 非 git"

任务运行 → 所有写工具路径校验落在 worktree 内
任务 Review → 展示 changeset + checks
应用 → 合并到主工作区分支
收尾 → git worktree remove --force，删除分支（可选保留）
```

三条硬规则：

1. **worktree 放数据目录，不放项目内**。放项目内会被 file watcher 与 `git status` 扫到，主工作区会一直显示"有改动"。
2. **worktree 路径进 watcher 忽略表、文件树过滤表**。这是 P4 出口标准之一。
3. **删除 worktree 前必须确认无未提交改动**，有则转 Review 并提示，不静默丢弃。

## 4. 并发与配额

| 项 | 值 | 说明 |
|---|---|---|
| 同时运行任务数 | 2（默认） | 设置项可调，硬上限 4 |
| 超出时 | `Queued` | 有任务结束时按创建顺序启动 |
| 预览端口 | 4300–4399 | 按任务分配，任务结束后释放 |
| 数据库写 | 按 `connectionId` 串行 | worktree 隔离不了数据库 |
| 单任务最大轮次 | 12（子代理同级） | 防止跑飞 |

并发上限卡在 2 的理由不是 CPU，而是**验收质量**：三个同时跑的任务产生的变更清单，人一次看不过来，最后会全部点"应用"。

## 5. 合并协议

一次只应用一个任务：

```
1. 主工作区必须 clean，否则先提示 stash（不自动 stash）
2. 检查基线漂移：task.base_sha 之后主分支若已前进 N 个提交
   → 提示"这个任务基于 abc123，主分支已前进 4 个提交"
   → 提供"变基到最新"或"仍然合并"
3. git merge --no-ff lc/<tid>-<slug>
4. 冲突 → status = Review(failed)，弹"在编辑器中解决冲突"（handoff）
5. 成功 → 删除 worktree 与分支，status = Done，记录合并提交
```

不自动变基：变基会重写 agent 已经验证过的代码，可能引入它没测过的新上下文。让用户决定。

## 6. 与两种模式的关系

| 界面 | 编辑器模式 | Agent 模式 |
|---|---|---|
| 左栏 | 文件树 | 项目 + 会话 + 功能 |
| 任务列表 | 顶部横幅"3 个任务在 worktree 中运行 · 查看" | 一等列表 |
| 变更 | git 面板（读主工作区） | 任务 changeset（读 worktree）+ 逐文件应用 |
| 检查 | 底部"问题"面板 | 任务检查面板 |
| 合并 | 手工 merge / rebase | "应用"按钮 |

编辑器模式必须显示横幅：编辑器改的是主工作区，任务改的是 worktree，两处不同步。这个提示缺失就会出现"我改了怎么没生效"。

## 7. 事件（进 lc-proto）

```
task:created        { taskId, title }
task:status         { taskId, status }
task:step           { taskId, stepIndex, state }
task:change         { taskId, FileChange }
task:check          { taskId, Check }
task:needs-input    { taskId, kind }      // 触发状态栏"待确认"
task:merged         { taskId, commitSha }
task:discarded      { taskId }
```

全部单向、带序号，UI 只订阅不查询（除首次加载）。

## 8. 验收标准

- 两个任务并行写同一文件的不同部分，互不覆盖，合并后两份改动都在。
- 任务 A 运行中切到编辑器模式修改主工作区，任务 A 的 diff 不含这些修改。
- 同一数据库连接上两个任务同时执行写，串行完成，无交叉事务。
- 关闭应用后重新打开，`Queued` 与 `Review` 状态保留，`Running` 标记为 `Failed(interrupted)`。
