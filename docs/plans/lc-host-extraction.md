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
2. **M2 SessionRegistry**：lc-ohos 的 session map/回收逻辑泛型化（limit 可配、running 保护、alive 迟到抑制）进 lc-host；desktop 的 `AgentSessions` 薄化为调用方（保留落盘与路由）。风险点：桌面会话持久化格式不变。
3. **M3 TurnRunner**：抽 `TurnRunner::run_turn(factory, sink, cancel)`；lc-ohos worker 闭包与 desktop `spawn_worker` 主体改为装配 factory 后调 runner。审批：`ApprovalHook` trait——桌面注入队列实现，OHOS 注入「plan 直接放行 + NeedsApproval 时发 approval_required 后 done」。
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
