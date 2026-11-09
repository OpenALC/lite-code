# 示例工作区

这是 Lite Code 鸿蒙外壳首次启动时复制到应用沙箱的只读示例项目。

目录结构：

- `src/main.rs`：入口与演示循环
- `src/util/message.rs`：中文问候语
- `src/util/constants.rs`：常量
- `docs/note.md`：说明

所有内容都来自 HAP 的 `rawfile/sample-workspace`，由 ArkTS 复制到
`filesDir/sample-workspace`，再由 Rust 宿主通过 `fs_read_dir` / `fs_read_file`
读取。修改示例请改 rawfile 后重新安装。
