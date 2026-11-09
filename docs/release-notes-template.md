# 发布说明模板

> 每次发版复制本文件为 `docs/releases/<version>.md` 并填写。`release.yml` 在打 draft 时引用本文件。
> 已知问题一节**不允许留空**：没有问题就写"本版本无已知高风险问题"。

## vX.Y.Z

**日期**：YYYY-MM-DD
**类型**：pre-release / stable
**对应里程碑**：Pn

### 亮点

- 一句话说明这个版本最重要的能力

### 新增

- -

### 变更

- -

### 修复

- -

### 验证（门禁记录，必填）

- Web：`pnpm lint` / `pnpm typecheck` / `pnpm test`（N 项 / M 文件）/ `pnpm build`。
- Rust：`cargo fmt` / `clippy` / `test`（K 项）结果；无变更时注明“沿用基线”。
- 发布资产：数量与清单（Windows NSIS + lc-cli、Linux x64/arm64 AppImage/deb + lc-cli ×2、
  `SHA256SUMS`、`latest.json`）；`SHA256SUMS` 必须提及。
- 签名状态：`signed:true/false`（false 时注明“签名密钥未接入，A 类门禁未关闭”）。
- 覆盖说明：`cargo-deny`、`pnpm audit --prod`、lane 边界检查由 CI verify 覆盖时引用 run 结论；
  arm64 产物存在与否需写明。
- 真机/端到端：实际机型或沙箱验证点。

### 已知问题

| 问题 | 影响 | 规避方式 |
|---|---|---|
|  |  |  |

### 下载

| 平台 | 文件 | 说明 |
|---|---|---|
| Windows 10/11 x64 | `lite-code_X.Y.Z_x64-setup.exe` | NSIS 安装包，首次运行可能提示未知发布者 |
| Windows x64（便携） | `lite-code_X.Y.Z_x64.zip` | 解压即用 |
| Linux x64 | `lite-code_X.Y.Z_amd64.AppImage` | 需要 webkit2gtk-4.1 |
| Linux x64（deb） | `lite-code_X.Y.Z_amd64.deb` | Ubuntu 22.04+ |

校验：`SHA256SUMS` 随发布附带。

### 升级注意

- 配置是否兼容
- 工作区是否需要迁移
- 降级是否可行

### 平台支持

- Windows 10/11 x64：支持
- Linux x64：支持（顺带）
- macOS：不支持
- HarmonyOS：后续版本，见 `MILESTONES.md` P6
