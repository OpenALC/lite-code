# Lite Code

轻量级桌面 Agent 工作台：Tauri 2 + Rust 核心 + Vue 3 / Monaco。
不是 Claude Code / Codex 的 GUI 套壳——Agent harness、文件系统、进程与运行时都在 Rust 侧，前端只是可替换的视图层。

> 当前处于 **P1 编辑器 alpha**：文件树、Monaco 编辑、保存、Node 运行时设置与可调布局可用；Agent 模式仍是占位。

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
| [docs/CI.md](docs/CI.md) | CI、发布与签名 |
| [docs/STATUS.md](docs/STATUS.md) | 当前进度快照 |

## 许可证

`Apache-2.0 OR MIT`。
