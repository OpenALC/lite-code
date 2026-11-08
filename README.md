# Lite Code

轻量级桌面 Agent 工作台：Tauri 2 + Rust 核心 + Vue 3 / Monaco。
不是 Claude Code / Codex 的 GUI 套壳——Agent harness、文件系统、进程与运行时都在 Rust 侧，前端只是可替换的视图层。

> 当前处于 **P6 稳定化预览**：P3 Agent、P4 任务工作台、P5 数据库/Git、P6 arm64 与更新器检查可用；
> 工作区全文搜索、编辑器区设置页与亮/暗主题已进入预览版。
> 鸿蒙首版为只读预览 + 对话；签名、soak、第三方审计由维护者关闭。
> OpenAI Responses 为默认端点；Anthropic Messages 已完成事件解码，传输层在后续版本接入。

## 下载与安装

从 [Releases](https://github.com/OpenALC/lite-code/releases) 下载 Windows NSIS 安装包或 Linux AppImage/deb。

- **安装包未签名**：首次运行 Windows SmartScreen 可能提示"未知发布者"，属预期行为；可在"更多信息 → 仍要运行"继续。
- 发布页附 `SHA256SUMS`，安装前请核对。
- Linux 产物在 P2 起提供正式 AppImage/deb；需要系统 `webkit2gtk-4.1`。

## Node 是可选集成

Lite Code **不捆绑 Node**。未安装 Node 时：

- 编辑器、文件树、保存、布局等核心功能正常可用；
- Node 相关入口显示降级引导；
- 可在"Node 运行时"面板手动指定 `node.exe`，或下载官方便携版（校验 SHA256 后安装到应用数据目录）。

harness、编辑器与 Git 不依赖 Node；MCP、数据库等可选集成才需要它。

## Agent 模式

在 Agent 面板里选择模式（`plan` / `chat` / `auto` / `yolo`），输入任务即可。左侧活动栏的
**供应商**页面可以管理供应商（OpenAI Responses；Anthropic Messages 即将支持）、Base URL、
API Key、模型列表（上下文窗口/压缩阈值/思考强度）以及默认与 Subagent 模型。API Key 默认
保存在本机 `~/.lite-code/providers.json`（Unix 权限 0600），不会写入日志；也支持环境变量
`LC_API_KEY` 与 `apiKeyRef: env:NAME`。会话默认使用 `~/.lite-code/providers.json` 里的 `defaultModel`：

```json
{
  "providers": [
    {"id": "openai", "kind": "openai-responses", "label": "OpenAI",
     "baseUrl": "https://api.openai.com/v1", "apiKeyRef": "env:OPENAI_API_KEY", "enabled": true}
  ],
  "models": [
    {"id": "gpt-5", "provider": "openai", "modelId": "gpt-5",
     "thinking": ["off", "low"], "contextK": 400, "compressAtK": 300, "capabilities": ["tools"]}
  ],
  "defaultModel": "gpt-5"
}
```

- 密钥优先读 `LC_API_KEY`，否则解析 `apiKeyRef: "env:NAME"`；没有配置文件时可只设 `LC_API_KEY` / `LC_BASE_URL` / `LC_MODEL`。
- `auto` 下高风险操作会弹审批（仅本次 / 本会话同类 / 拒绝）；`yolo` 每次应用启动需二次确认。
- 上下文超过 `compressAtK` 会压缩并在会话流留下可展开记录。
- MCP：在项目根放 `.mcp.json`（Claude Code 的 `mcpServers` 格式），stdio server 的工具会以 `mcp_<server>_<tool>` 注册，调用前按 Network 风险审批。

Headless CLI（CI / 脚本）：

```bash
lc-cli agent run --prompt "读 src/lib.rs，修格式并跑测试" --workspace . --json --yes
```

退出码：`0` 正常结束，`1` 运行错误，`2` 权限拒绝或审批未通过。

## 数据库与 Git

- **数据库面板**（右侧功能栏）：连接 SQLite 文件、PostgreSQL / MySQL / SQL Server，浏览 schema、写 SQL、查询/执行/EXPLAIN。
  - 查询结果默认只回样本（最多 1000 行），并标记 `untrusted`；数据库内容不能当指令。
  - 无 `WHERE` 的 `UPDATE`/`DELETE`、多语句、DDL 黑名单会被 SQL 闸门直接拒绝。
  - Agent 工具：`db_list` / `db_schema` / `db_query` / `db_explain` / `db_execute`，调用前走审批。
  - SQLite 走 Node 22.5+ 内置 `node:sqlite`；PostgreSQL（pg）、MySQL（mysql2）、SQL Server（mssql）驱动已打包为单文件 bundle。
  - pg/mysql/mssql 已通过驱动加载、参数映射与失败路径测试，但**尚未用真实服务器做端到端验证**。
- **Git 面板**（右侧功能栏）：分支/ahead-behind、staged/unstaged 文件、diff、暂存/取消暂存、提交与最近提交。

## 插件

插件系统 v0 支持 Node/JavaScript 命令插件、WASM 命令插件，以及声明式 MCP / LSP 配置。
插件目录为 `~/.lite-code/extensions`，清单与协议见 [docs/EXTENSIONS.md](docs/EXTENSIONS.md)。
当前范围不包含 VS Code API 兼容、插件市场与插件签名。

## 开发

```bash
# Rust
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked

# Web
pnpm install --frozen-lockfile
pnpm lint && pnpm typecheck && pnpm test -- --run && pnpm build

# 本地运行桌面壳
pnpm tauri dev
```

要求：Rust `1.99.1`（见 `rust-toolchain.toml`）、Node 22、pnpm 10。
lane 边界、契约漂移、cargo-deny 等门禁见 [docs/CI.md](docs/CI.md)；本地等价命令也在其中。

## 文档

| 文档 | 内容 |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | 分层、crate 划分、关键抽象 |
| [docs/MILESTONES.md](docs/MILESTONES.md) | P0–P6 范围与出口标准 |
| [docs/RUNTIME.md](docs/RUNTIME.md) | Node / Shell 探测与降级 |
| [docs/AGENT.md](docs/AGENT.md) | Agent harness、工具与权限 |
| [docs/PLAN.md](docs/PLAN.md) | 并行开发组织与风险 |
| [docs/EXTENSIONS.md](docs/EXTENSIONS.md) | 插件清单、Node/WASM 协议、MCP/LSP |
| [docs/CI.md](docs/CI.md) | CI、发布与签名 |
| [docs/STATUS.md](docs/STATUS.md) | 当前进度快照 |
| [docs/SECURITY-AUDIT.md](docs/SECURITY-AUDIT.md) | P6 安全审计范围与残余风险 |

## 许可证

`Apache-2.0 OR MIT`。
