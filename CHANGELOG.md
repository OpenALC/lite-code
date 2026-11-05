# Changelog

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号规则见 [docs/VERSIONING.md](docs/VERSIONING.md)：`0.<里程碑>.<发版序号>-preview.<重建号>`。
发版时由 Conventional Commits 标题生成初稿，人工合并同类项。

## [Unreleased]

### Added

- Cargo workspace 骨架：`lc-proto` / `lc-core` / `lc-cli` / `lc-desktop`（Tauri 薄壳）
- `lc-proto` 契约 v0：命令、DTO、`ErrorCode`、事件枚举；TS 绑定生成与漂移检查
- `lc-proto` 契约 v0.1：新增 `workspace_root` / `fs_write_file`；TS 绑定改为带 `export` 的 ES 模块，web 直接 `import type`
- `lc-proto` 契约 v0.2：新增 `workspace_open`；工作区根由 Tauri 状态持有并持久化到 `config.json`
- 打开工作区：顶栏输入目录路径，校验后切换文件树根；无工作区时编辑器仍可用
- `lc-core::runtime`：Node 探测状态机（Tier 1–4 候选、Store 别名桩识别、GhostInstall 自检）与 Shell 探测（PowerShell 7 / bash），纯函数设计可注入测试
- `lc-cli`：`node detect` / `shell detect` / `contract info`，golden 输出测试
- `apps/web`：Vue 3 + Vite 8 + TS 5.9 + Monaco 起壳，`LayoutProfile` 骨架与模式切换器
- CI：fmt / clippy / test（Windows + Ubuntu）、契约漂移、lane 边界、cargo-deny、web 四件套
