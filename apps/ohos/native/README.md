# Native / Rust 交叉编译（Phase 3）

本目录预留给 Phase 3 的 NAPI 桥。当前 `entry/src/main/cpp` 只有一个占位
`libentry.so`（`getBridgeVersion`），没有链接 Rust 静态库；资源加载切片不依赖
它。

后续把 `crates/lc-ohos` 编成静态库后，在 `CMakeLists.txt` 里按 `OHOS_ARCH`
链接 `target/<triple>/release/liblc_ohos.a`。探针命令：

```bash
rustup target add x86_64-unknown-linux-ohos aarch64-unknown-linux-ohos
cargo build --release --target x86_64-unknown-linux-ohos -p lc-ohos
cargo build --release --target aarch64-unknown-linux-ohos -p lc-ohos
```

风险与依赖裁剪见 `docs/plans/ohos-shell.md` §5.8。
