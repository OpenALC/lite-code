# 安全审计范围（P6）

> 供第三方审计使用的范围与已知残余风险清单。P6 出口要求“审计高危清零”；
> 本文只列事实与证据，不替代审计结论。

## 1. 信任边界

| 边界 | 输入 | 信任级别 | 处置 |
|---|---|---|---|
| 前端 ↔ Rust | Tauri IPC | 低（WebView 可被 XSS 影响） | 根由 Rust 持有；命令参数校验；CSP |
| Agent ↔ LLM | 流式文本/工具调用 | 低 | 策略闸门先于工具执行；审批 |
| 工具 ↔ 文件系统 | 路径/内容 | 中 | `ensure_within_root` + canonicalize |
| 工具 ↔ shell | 命令原文 | 低 | 风险分级、静态黑名单、审批、进程树回收 |
| 工具 ↔ 数据库 | SQL / 结果 | 低 | SQL 闸门、只读连接、结果 `untrusted` 边界 |
| 工具 ↔ MCP | 外部 server 输出 | 低 | 工具名限定、Network 风险审批、不可信边界 |
| Git | 仓库路径/文件 | 中 | `git -C`、参数数组、主工作区 clean |

## 2. 高风险面与现有缓解

### 2.1 路径逃逸

- `lc-core::fs::ensure_within_root`：canonicalize + 根内校验，拒绝悬空软链。
- Agent `read/write/edit/glob/grep` 与 Tauri fs 命令共用该边界。
- 测试：`fs::tests::*escape*`、`agent::exec` 临时目录集成测试。

### 2.2 命令执行

- 策略 `plan/chat/auto/yolo`；静态黑名单（`rm -rf /`、`git push --force` 等）。
- 执行类工具默认审批；`yolo` 每次应用启动二次确认。
- shell 参数化 spawn（不拼 shell 字符串），PowerShell 7 / bash 自动探测。
- 超时杀进程树（Windows Job Object / Unix 进程组）。

### 2.3 SQL

- 单语句、注释/字符串感知解析；无 `WHERE` 的 `UPDATE/DELETE` 拒绝。
- 黑名单：`DROP DATABASE`、`TRUNCATE`、`ALTER SYSTEM`、`COPY PROGRAM` 等。
- 只读连接：分类器拒绝 + `node:sqlite` readOnly + 工具层拒绝写。
- 结果只回样本，标记 `untrusted`，并在工具输出外包显式不可信边界。

### 2.4 提示注入

- 数据库结果：`[UNTRUSTED DATABASE CONTENT …]` 边界。
- MCP 结果：`[UNTRUSTED MCP CONTENT …]` 边界。
- 工具错误回填为数据；策略判定不依赖模型文本。
- 残余：模型仍可能被内容影响；边界是缓解而非保证。

### 2.5 IPC / WebView

- Tauri CSP：`default-src 'self'`，`object-src 'none'`，`frame-src http: https:`（工作台预览）。
- 多词命令参数统一 `rename_all = "snake_case"`。
- 事件只下行；前端不持有 workspace root。

## 3. 审计重点问题

1. `ensure_within_root` 在 Windows 短路径/UNC/Junction 下的绕过可能。
2. PowerShell/bash 包装层对引号、编码、退出码的处理。
3. SQL 解析器的方言覆盖（MySQL `#`、MSSQL `#temp`、注释嵌套）。
4. MCP stdio 子进程的继承管道、环境变量与清理。
5. Tauri 命令参数反序列化与 `Option` 缺省行为。
6. 发布产物签名/更新器（当前未启用签名，见 STATUS）。

## 4. 已知残余风险

- 未签名安装包；更新器/回滚通道未接入。
- MCP 仅 stdio；HTTP/SSE 未实现。
- SQL 闸门是分类器，不是完整方言 AST；只读协议层依赖 SQLite 驱动。
- 提示注入边界依赖模型遵守标记；无形式化保证。
- 第三方依赖由 `cargo-deny` 与 pnpm lock 管理，但未做供应链人工审计。

## 5. 复现与门禁

```bash
cargo deny --locked check
cargo test --workspace --locked
C:/Git/usr/bin/bash.exe -c 'export PATH=/usr/bin:$PATH; exec scripts/check-lanes.sh'
pnpm.cmd test -- --run
```

CI 在 Windows/Ubuntu 上执行同一组门禁；`release.yml` 在 tag 上先跑 verify 再构建。
