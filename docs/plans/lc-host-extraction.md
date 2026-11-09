# lc-host 抽取设计：会话编排共享化（技术债，preview.53 起记录）

> 状态：设计定稿，待执行（模型供应商限额重置后由 coder 切片实施）。
> 范围：**仅会话编排**（AgentLoop 生命周期/事件转发/取消/审批钩子）。
> providers 视图/合并已先行下沉 lc-core（preview.59 完成），本设计不复述。

## 1. 现状

| 侧 | 位置 | 规模 | 职责 |
|---|---|---:|---|
| 桌面 | `apps/desktop/src-tauri/src/agent.rs` | 2166 行 | `AgentSessions`（map/路由/落盘）、`Worker`（线程 + 命令通道）、`spawn_worker`（AgentLoop 装配：task/db/MCP/LSP 钩子、审批队列）、`EventSink` trait（Tauri 事件/预览/task 回调） |
| 鸿蒙 | `crates/lc-ohos/src/lib.rs` | 2663 行（含 history/providers/fs） | worker 闭包内惰性构建 AgentLoop、取消原子令牌、stub 回退、TCP 预探、事件 seq 信封 |

两端重复的本质：**AgentLoop 装配 + 轮次生命周期 + 事件序列化信封**。桌面版重（审批/task/db），鸿蒙版轻（plan 只读、无审批 UI），但骨架相同。

## 2. 目标架构

```
crates/lc-host（新 crate，workspace 成员）
  ├─ SessionRegistry        // session map + 回收（lc-ohos 的 MAX/alive 语义推广为可配）
  ├─ TurnRunner             // 一轮：build loop(可注入 factory) → run → 事件出口
  │    配置：AgentFactory（OHOS=RealAgentFactory+stub 回退；桌面=spawn_worker 装配体）
  │    出口：EventSink trait（已有，lc-host 直接复用 desktop 的定义并下移）
  ├─ event envelope 封装    // seq 原子 + EventEnvelope 序列化（两端统一）
  └─ cancel 语义            // Arc<AtomicBool> 令牌 + RunningGuard（lc-ohos 已有，推广）

apps/desktop/src-tauri/src/agent.rs
  → 保留：Tauri 命令面、审批队列、task/db 钩子（作为 AgentFactory 注入）
  → 删除：Worker/线程管理/envelope 逻辑（换 lc-host）

crates/lc-ohos
  → 保留：ffi/history/providers/fs/dispatch、stub 回退策略、TCP 预探（作为 factory 前置）
  → 删除：worker 闭包骨架/envelope/seq（换 lc-host）
```

## 3. 迁移步骤（每步独立可发布）

1. ~~**M1 lc-host 骨架**~~：**已完成（preview.64）**——lc-host `envelope` 模块
   （EventSink/Seq/emit_envelope）落地，lc-ohos 已切换；桌面侧 EventSink
   trait 与 ohos 的 fn-pointer 形状不同，统一推迟到 M3（随 TurnRunner 一起做）。
2. ~~**M2 SessionRegistry**~~：**已完成（main，preview.65 随版发布）**——
   lc-ohos 的 session map/回收逻辑泛型化（limit 可配、running 保护、alive
   迟到抑制）进 lc-host `sessions` 模块（`SessionHandle`/`SessionEntry<P>`/
   `Sessions`）；desktop 的 `AgentSessions` 未动（按原计划留 M3，随
   TurnRunner 一起切，避免两次回归）。风险点「桌面会话持久化格式不变」未触及。

   **M2 实施记录（已完成）**：
   - 移动单元（lc-ohos → lc-host，签名不变）：`SessionHandle`（info/cancel/running/alive
     四原语 + `info_snapshot`）、`sessions map` 与 `reclaim_terminated_sessions`、
     `begin_turn`（锁内查表 + CAS running）、`next_session` 序号、`MAX_SESSIONS`。
   - lc-host 新类型命名：`Sessions`（map + limit + ordinal）；`SessionHandle` 保持
     原名；`is_reclaimable = !running`（cancel 残留语义已在注释中固化，未引入）。
   - 宿主负载泛型化：`SessionEntry<P>`（lc-ohos `P = AgentSlot`，桌面 M3 自定）；
     lc-host 不依赖 lc-core。
   - 留在 lc-ohos 的部分：`TurnEmitter`（依赖 SessionInfo 写回与 Plan 模式
     强约束）、`RunningGuard`、`create_session` 的 SessionInfo 构造（mode 强转 Plan）。
   - 验收（全绿）：lc-ohos 36 项测试不改语义全过 + lc-host 12 项（+9 Sessions
     单测：回收序/running 保留/alive 抑制/并发轮次拒绝/unknown not_found）；
     workspace 663；check-all 全量门禁通过；模拟器切片 H 子集 b/c/d/f 全过
     （40 会话回收：ohos-5 not_found、ohos-41 ack；取消 cancelled；hilog 无 panic）。
   - 明确不做：desktop `AgentSessions` 切换（M3 随 TurnRunner 一起）。
3. **M3 TurnRunner（切片修订，M2 后研读 desktop agent.rs 更新）**：

   **修订说明**：原设想「统一 `run_turn(factory, sink, cancel)` + ApprovalHook」。
   研读桌面 `agent.rs`（2166 行）后确认两侧 worker 骨架本质不同——桌面是**长驻
   worker 线程 + 命令通道**（Send/ResolveApproval/Cancel 三命令循环，审批 resume
   复用同一 AgentLoop，`busy` latch，task 状态联动，MCP 生命周期托管），鸿蒙是
   **每轮 spawn 线程 + RunningGuard**（agent 槽位常驻 `SessionEntry`，plan 模式
   只读工具天然无审批）。把桌面审批队列塞进单一 run_turn 会迫使
   `spawn_worker`/`Worker::run` 重写，风险远超收益（违反 §5 护栏「M3 前不改
   spawn_worker 签名之外的行为」）。M3 改为下沉真正重复的三块，worker 循环
   留在宿主：

   - **3a. EventSink trait 下沉 lc-host（envelope 模块）**：
     `pub trait EventSink: Send + Sync { fn emit(&self, event: CoreEvent); }`
     （CoreEvent 级，桌面形状）+ `EnvelopeSink` 适配器（内部 `Seq`，
     `emit` → 契约 `EventEnvelope` JSON → ohos 的 fn-pointer sink 走既有
     `emit_envelope`）。桌面 `AppEventSink`/`RecordingSink` 改实现 lc-host
     trait，删本地 `next_envelope`/`AtomicU64` seq。seq 起点：桌面现 0 起、
     ohos 1 起；统一走 `Seq`（1 起）后桌面首信封 seq 0→1——web 当前不消费
     envelope seq（已核实无丢序检测逻辑），单调性不变，风险低。
   - **3b. 完成事件组合下沉（分支 `host/m3b-completion-events` 已实施，
     待审查后合并）**：落点修订为 **lc-core `agent::events`**（而非 lc-host——
     `completion_events` 依赖 `loop_outcome`/`session_info`，lc-host 不依赖
     lc-core 是 M2 定下的边界）。桌面 `agent.rs` 私有 `completion_events`
     删除、改调共享实现（含测试经 import 继续覆盖）；lc-core +1 项排序契约
     单测（三终态均「session_state 在前、loop_outcome 事件在后，不补
     agent:done」）。鸿蒙保持 emitter 内写回交织的实现，事件顺序以共享
     函数为准。
   - **3c. 桌面 `AgentSessions` 表切 `Sessions<P>`（stretch，可选）**：桌面无
     回收语义（会话活到断开/退出），limit/ordinal 价值有限；若做则
     `limit = usize::MAX`、`P = desktop SessionHandle`，落盘/路由不动。
     优先级低，可并入 M4。
   - **3d. ApprovalHook trait：不做**。plan 模式只读工具不会 NeedsApproval；
     桌面审批 resume 绑定长驻 worker 命令循环，抽象后两端都用不上。第三
     宿主出现再提取。

   **M3 验收**：桌面 agent 单测全过（worker 审批/取消/busy 语义为安全网）；
   两端信封形状不变（`isCoreEvent`/TS 零改动）；模拟器切片 H 子集 b/c/d/f
   回归；web 431 + workspace 全绿 + check-all；lc-host 新增 cancel 中断/
   seq 连续单测。
4. **M4 收尾**：`spawn_worker` 仅剩桌面装配参数；lc-ohos `RealAgentFactory`/stub/预探成为 factory 组合。删除两端死代码。

## 4. 测试策略

- 每步：`cargo test --workspace --locked`（桌面 agent 现有单测是主要安全网）+ clippy/fmt。
- M3 新增 lc-host 单测：cancel 中断、panic 捕获补 error+done、迟到事件 alive 抑制、envelope seq 连续。
- 回归：桌面 `pnpm test` 不变；鸿蒙模拟器 e2e 跑切片 H 的 a–f 用例子集（真实流式 + 取消 + 回退）。

## 5. 风险与护栏

| 风险 | 护栏 |
|---|---|
| 桌面审批/task 钩子回归 | M3 前不改 spawn_worker 签名之外的行为；审批队列测试先行 |
| OHOS 行为漂移（stub 回退/预探时序） | 切片 F/G 的耗时断言（<2.5s 回退）纳入 M3 验收 |
| 事件信封形状变化破坏 web | `isCoreEvent`/TS 形状零改动，seq 由 lc-host 统一生成 |
| 范围蔓延 | 不动 providers/history（已完成）；不动 Tauri 命令面 |

## 6. 验收

- 两端 `agent` 相关重复代码净减 ≥ 40%（wc -l 对比）。
- 切片 H 回归子集全过；`check-all.sh` 全绿。
- 新 crate 过 lane 边界检查（lanes 列表加入 `host` 或复用 `agent`，按 check-lanes.sh 规则登记）。
