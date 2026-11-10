# M3b 合入 main 审查意见（2026-11-10）

**结论：暂缓直接合并。** `host/m3b-completion-events` 只有一个待合入提交；未发现该提交改变桌面完成事件的运行行为。当前唯一的合并阻塞是抽取计划的内容冲突。保留两边各自有效的决策、让合并结果通过仓库现有 CI 后，建议合入。

## 项目进度与审查范围

| 项目 | 核实结果 |
| --- | --- |
| 主线与发布 | 本地及远端 `main` 均为 `371e707`，[该提交的 CI 已通过](https://github.com/OpenALC/lite-code/actions/runs/42706221876)。项目处于 P6 稳定化；[preview.64](https://github.com/OpenALC/lite-code/releases/tag/v0.6.1-preview.64) 已发布，preview.65 有说明草稿，尚无远端标签。 |
| 宿主抽取 | M1 已随 .64 发布；M2（鸿蒙会话表）与 M3a（事件信封和序号原语）已在主线。M3c（桌面改用同一会话表）和 M3d（审批钩子）已决定不做；M4 收尾仍在计划中。 |
| 本次候选 | 远端分支 `host/m3b-completion-events` 指向 `a5678a9`，共同基点为 `1b4a9c9`。分叉后主线新增 7 个提交，候选新增 1 个，涉及 5 个文件。旧 DB、文件树、性能分支的补丁在主线已有等价改动；历史发布分支不属于本轮候选。 |

## 审查发现

**1. 阻塞合并：抽取计划中的 M3b 和 M3c 必须分别取舍。** [抽取计划第 3 节](../plans/lc-host-extraction.md)第 82 行起，候选把 M3b 的实际落点从计划中的 `lc-host` 改为 `lc-core::agent`；主线则在相邻的 M3c 段落记录了桌面保留私有会话表的决定。`git merge-tree --write-tree main host/m3b-completion-events` 返回 1，只报告这一处文件冲突。整块取主线会丢失 M3b 的实际实现，整块取候选会把已关闭的 M3c 恢复为待办。解决方式：采用候选的 M3b 段落，保留主线的 M3c 段落及理由；合入版删除“待审查后合并”的临时状态。Rust 文件可自动合并。

**2. 随冲突修正：完成事件的说明容易被读成正常结束也没有 `agent:done`。** 候选计划第 87–89 行把三个结果概括为“不补 agent:done”。[事件映射](../../crates/lc-core/src/agent/events.rs)实际先发 `session_state`，随后按结果区分：正常结束发一次 `agent:done(end_turn)`；等待审批发 `approval_required`，保持 busy 等待恢复；策略拒绝发 `permission_denied` 错误。建议直接列出这三种结果。[鸿蒙宿主](../../crates/lc-ohos/src/lib.rs)仍自行写回会话，并对审批或拒绝额外发结束事件；本次没有改动该路径。

**3. 发布前修正：preview.65 草稿高估了桌面的迁移范围。** [发布说明](../releases/0.6.1-preview.65.md)第 9–11 行称两端会话生命周期已由同一份代码承载，但[桌面仍使用自己的 AgentSessions](../../apps/desktop/src-tauri/src/agent.rs)，M2 迁移的是鸿蒙。第 54 行还将 M3c 列为后续排期。建议亮点明确区分“M2 迁移鸿蒙会话表”和“M3a 两端复用事件信封”，将 M3c 改为“决定不做”，M3b 按最终打标提交填写。这是发版说明问题，不阻塞 Rust 变更。

## 验证依据

逐项比对了候选的五个文件：桌面原私有函数的函数体原样迁入核心层，桌面调用点及审批恢复逻辑未变，契约类型未改。其余变更为核心层导出与测试、计划说明，以及忽略本地证据和草稿目录；未将这些目录的内容纳入审查。

在候选提交的独立工作树执行 `cargo test --locked -p lc-core -p lc-desktop --lib completion_events`，核心层和桌面层各 1 项通过，0 失败。提交说明记载的 workspace 667 项通过属于作者记录，本次未复跑全量。审查时没有打开的 PR，也未查到候选分支的 Actions 运行记录；上表的绿色 CI 仅对应主线，不能替代冲突解决后的合并检查。
