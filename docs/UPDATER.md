# 更新器与回滚通道

> P6 出口门禁 A 类第 6 项（见 [RELEASE-GATES.md](RELEASE-GATES.md) §2）。
> 通道组成：GitHub Releases 托管产物 + 静态 `latest.json` + 签名状态透传。
> 当前实现**只做检查**：不下载、不安装、不自动降级；签名密钥未配置时明确标为
> `signed: false`，此时 A 类门禁不能算完全关闭。

## 1. 范围

| 能力 | 状态 |
|---|---|
| 拉取 `latest.json` 并比较版本 | 已实现（Tauri 命令 `update_check`） |
| Windows x64 / Linux x64 / Linux arm64 | 已支持（平台键见下） |
| 返回下载 URL、SHA-256、签名状态、上一版回滚目标 | 已实现 |
| 生成清单（无签名 / 有 `.sig`） | 已实现（`scripts/gen-update-manifest.mjs`） |
| 自动下载 / 安装 / 降级 | **不做**（避免执行未签名或未校验的安装包） |
| 密码学签名校验 | **未实现**（由后续安装器或 `tauri-plugin-updater` 接入） |

设计取舍：没有引入 `tauri-plugin-updater`。检查命令用 Rust `ureq` + `serde` 实现，
与 `lc-core` 复用同一个已锁定版本（不新增第三方包）；避免在签名密钥未就绪时
把自动更新插件装进发布壳。后续若要自动安装，再引入插件并配置 `pubkey`。

## 2. 清单格式

文件名固定为 `latest.json`，UTF-8，字段如下：

```json
{
  "version": "0.6.1-preview.1",
  "notes": "P6 preview 版本说明",
  "pub_date": "2026-11-05T00:00:00Z",
  "signed": true,
  "platforms": {
    "windows-x86_64": {
      "url": "https://github.com/OpenALC/lite-code/releases/download/v0.6.1-preview.1/Lite.Code_0.6.1-preview.1_x64-setup.exe",
      "signature": "untrusted comment: signer public key\nRWQ...",
      "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    },
    "linux-x86_64": {
      "url": "https://github.com/OpenALC/lite-code/releases/download/v0.6.1-preview.1/Lite.Code_0.6.1-preview.1_amd64.AppImage",
      "signature": "",
      "sha256": "..."
    },
    "linux-aarch64": {
      "url": "https://github.com/OpenALC/lite-code/releases/download/v0.6.1-preview.1/Lite.Code_0.6.1-preview.1_aarch64.AppImage",
      "signature": "",
      "sha256": "..."
    }
  },
  "previous": {
    "version": "0.6.0-preview.1",
    "notes": "上一版说明（可空）",
    "platforms": {
      "windows-x86_64": {
        "url": "https://github.com/OpenALC/lite-code/releases/download/v0.6.0-preview.1/Lite.Code_0.6.0-preview.1_x64-setup.exe",
        "signature": "untrusted comment: ...\nRWQ...",
        "sha256": "..."
      }
    }
  }
}
```

规则：

- `platforms` 键固定为 `windows-x86_64`、`linux-x86_64`、`linux-aarch64`；
  未构建的平台可以缺省，检查时缺失会返回 `not_found`。
- `sha256` 必填，64 位十六进制，由生成器对本地产物计算。
- `signature` 可省略或为空字符串；`signed` 是全局声明：
  - 所有平台都有 `.sig` 时生成器写 `signed: true`；
  - 任一平台缺少 `.sig` 时写 `signed: false`，已有签名仍按平台写入；
  - 若 `signed: true` 但选中平台的 `signature` 为空，检查命令直接报
    `contract_mismatch`，不把未签名包当作已签名。
- `previous` 是**回滚通道**：保留上一版版本号与各平台 URL。只保留一层，
  不自动降级；`previous` 的平台缺失时（例如 arm64 首版）不阻断最新版检查，
  只是该平台没有回滚目标。

## 3. 运行时检查

Tauri 命令 `update_check`（`apps/desktop/src-tauri/src/updater.rs`）：

- 无参数；启动时不调用，只在 UI 触发时执行。
- 清单 URL：
  - 默认 `https://github.com/OpenALC/lite-code/releases/download/updater-manifest/latest.json`
    （滚动 release 资产，避免 GitHub `releases/latest` 忽略 prerelease 的问题）；
  - 本地/测试可用环境变量 `LC_UPDATE_MANIFEST_URL` 覆盖。
- 网络请求在 `spawn_blocking` 中执行，10 秒超时，响应上限 1 MiB。
- 网络错误、清单缺失、版本号非法、`sha256` 非法、平台缺失都会返回可读的
  `ErrorBody`，不会 panic，也不会阻断启动。
- 只允许 `https://`；`http://` 仅允许回环地址（`localhost` / `127.0.0.1` / `[::1]`），
  便于本地清单服务器。
- 命令只做检查，不校验签名本身，也不下载产物。

返回体（序列化为 snake_case）：

```json
{
  "current_version": "0.6.0-preview.1",
  "available": true,
  "version": "0.6.1-preview.1",
  "notes": "P6 preview 版本说明",
  "pub_date": "2026-11-05T00:00:00Z",
  "url": "https://...",
  "signature": "untrusted comment: ...\nRWQ...",
  "sha256": "...",
  "signed": true,
  "previous": {
    "version": "0.6.0-preview.1",
    "notes": "...",
    "url": "https://...",
    "signature": "...",
    "sha256": "..."
  }
}
```

`available=false` 表示已是最新或清单版本更旧；`previous` 仍然返回，供 UI 展示
“可回退到上一版”的入口。

## 4. 发布流程

1. **准备版本**（维护者）：更新 `Cargo.toml` / `package.json` /
   `tauri.conf.json` 到 `0.6.1-preview.1`，补 `docs/releases/0.6.1-preview.1.md`，
   确认 `.milestone=6`。
2. **打 tag**：`v0.6.1-preview.1`。`release.yml` 的 prep 会校验 tag 与三处 manifest。
3. **构建**：Windows NSIS、Linux AppImage/deb（x64 与 arm64）、`lc-cli`。
   更新器签名需要：
   - `TAURI_SIGNING_PRIVATE_KEY`（私钥内容或路径）
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`（有密码时）

   并在 `tauri build` 时启用 `bundle.createUpdaterArtifacts`，
   让 Tauri 在安装包旁生成 `<产物>.sig`。**当前 release.yml 的 build job 尚未注入
   这两项**（见 §6 第 3 条），所以没有 `.sig` 时清单会写 `signed:false`。
   仓库里**只允许提交公钥**，私钥只能进 GitHub Secrets。
4. **生成清单**（在能访问全部产物的 job 中）：

   ```bash
   node scripts/gen-update-manifest.mjs \
     --version 0.6.1-preview.1 \
     --tag v0.6.1-preview.1 \
     --repo OpenALC/lite-code \
     --artifacts artifacts \
     --notes-file docs/releases/0.6.1-preview.1.md \
     --previous-manifest previous-latest.json \
     --output latest.json
   ```

   - `--artifacts` 递归扫描 `bundle/nsis/*.exe`、`bundle/appimage/*.AppImage`、
     `bundle/deb/*.deb`；按文件名中的 `x64/amd64/aarch64/arm64` 归类；
     Linux 同时存在 AppImage 与 deb 时优先 AppImage（自更新路径）。
   - GitHub 上传资产时会把文件名中的空格替换成点（如 `Lite Code…` →
     `Lite.Code…`），脚本按上传后的名字生成 URL；不要在发布后手工改名。
   - 签名默认读安装包旁的 `<文件>.sig`；也可用
     `--sig <平台=路径>` 显式指定。
   - 回滚目标来自 `--previous-manifest`（上一版 `latest.json`）；首次发布可省略。
   - `--require-signatures` 用于稳定版门禁：任一平台缺 `.sig` 直接失败。
5. **发布静态 JSON（注意 draft 时序）**：
   - `publish` job 已自动把 `latest.json` 生成到 `artifacts/latest.json` 并挂到当次
     draft release（同时进入 `SHA256SUMS`）；
   - **等维护者把 draft 发布后**，`updater-manifest.yml`（`release: published`）
     会自动完成滚动清单提升：

   ```bash
   # 版本 release 已发布，安装包 URL 才可公开访问
   gh release download "v0.6.1-preview.1" --pattern latest.json --output latest.json --clobber
   gh release view updater-manifest >/dev/null 2>&1 \
     || gh release create updater-manifest --prerelease \
        --title "updater manifest" --notes "滚动更新清单"
   gh release upload updater-manifest latest.json --clobber
   ```

   上面的命令是工作流的等价人工操作。不要在 draft 未发布时更新滚动清单：
   清单里的 URL 会指向尚不可见的资产，客户端检查成功但下载 404。
6. **验证**：本地设置 `LC_UPDATE_MANIFEST_URL` 指向清单，调用 `update_check`，
   确认 `available`、`sha256`、`signed` 与产物一致。

## 5. 签名依赖与门禁状态

有两类互不替代的签名：

| 类型 | 用途 | 依赖 | 缺失时 |
|---|---|---|---|
| Authenticode / SignPath | Windows 安装包来源可信，减少 SmartScreen 警告 | `SIGNPATH_ENABLED` + SignPath 凭据 | 安装包未做代码签名 |
| Tauri updater 签名（minisign） | 更新清单中的 `.sig`，供更新器校验安装包 | `TAURI_SIGNING_PRIVATE_KEY` | 无 `.sig`，`signed:false` |

因此：

- 只有 `SIGNPATH_ENABLED=true` **不等于**更新通道已签名；`.sig` 取决于
  `TAURI_SIGNING_PRIVATE_KEY`。
- 没有 `TAURI_SIGNING_PRIVATE_KEY` 时，清单仍可生成并可用：
  `signed:false`、`signature:""`。检查通道本身可验证，但**不能执行安装**，
  A 类第 6 项的“签名校验”关闭条件不成立。
- 公钥可用 `pnpm tauri signer generate` 生成并提交；私钥与密码只放 GitHub
  Secrets。不要为了本地测试把私钥写进仓库或测试夹具。
- 当前客户端只透传签名、不校验证书链或 minisign 签名；真正校验应在引入
  `tauri-plugin-updater` 或安装器阶段完成，并配置对应 `pubkey`。

## 6. 工作流集成（已接入）

`release.yml` 与 `updater-manifest.yml` 已按本节接入。没有签名密钥时清单仍然生成，
但明确写 `signed:false`，不会伪造签名。

1. **prep 输出签名开关**（`secrets` 不能在 job 级 `if` 中使用）：
   - `signing`：来自仓库变量 `SIGNPATH_ENABLED`；
   - `updater_signing`：`TAURI_SIGNING_PRIVATE_KEY` 是否非空，在 step 级 `env`
     读取后转成 output。它只用于日志/step summary 提示，不参与伪造签名。

2. **publish：生成并挂载清单**（`release.yml`）：
   - `actions/setup-node` 固定 Node 22；
   - 从滚动 release 读取上一版：
     `gh release download updater-manifest --pattern latest.json --output previous-latest.json --clobber`；
     不存在时用 `::notice::` 说明首次发布；同一 tag 重跑且版本相同时忽略 `previous`；
   - 运行生成器，产物目录与上传/校验共用同一 `$win` 选择
     （`artifacts/win-x64`，签名启用时为 `artifacts/win-x64-signed`）：

     ```bash
     node scripts/gen-update-manifest.mjs \
       --version "$VERSION" --tag "$GITHUB_REF_NAME" \
       --repo "$GITHUB_REPOSITORY" \
       --artifacts "$win/bundle" \
       --artifacts artifacts/linux-x64/bundle \
       --artifacts artifacts/linux-arm64/bundle \
       --notes-file "docs/releases/$VERSION.md" \
       --previous-manifest previous-latest.json \
       --output artifacts/latest.json
     ```

   - 输出 `artifacts/latest.json` 后会被加入 `release-assets.txt`，进而进入
     `SHA256SUMS` 与 `gh release create` 的资产列表；
   - 生成后读取 `signed`：为 `false` 时在日志写 `::warning::` 并追加到
     `$GITHUB_STEP_SUMMARY`，说明是密钥缺失还是“密钥存在但并非所有平台都有 `.sig`”；
   - draft 未发布前**不会**更新滚动 `updater-manifest`。

3. **构建侧签名注入仍是 TODO**：
   - 当前三个 build job 尚未注入 `TAURI_SIGNING_PRIVATE_KEY`，也没有在
     `pnpm tauri build` 时启用 `bundle.createUpdaterArtifacts`。因此即使配置了
     私钥，只要安装包旁没有 `.sig`，清单仍是 `signed:false`；
   - 接入时应在 tag 构建 job 注入私钥与密码，并追加
     `--config '{"bundle":{"createUpdaterArtifacts":true}}'`，或对产物运行
     `pnpm tauri signer sign --app-version "$VERSION" <installer>`；
   - `upload-artifact` 的 `bundle/**` 会自动包含 `.sig`；
   - 仓库里**只允许提交公钥**，私钥与密码只能进 GitHub Secrets。

4. **`release: published` 提升滚动清单**（`.github/workflows/updater-manifest.yml`）：
   - 只处理以 `v` 开头的版本 tag；`updater-manifest` 自身的 published 事件被过滤；
   - 从该 tag 下载 `latest.json`，校验 `version` 与 tag 一致、`platforms` 非空、
     `signed` 为布尔值；
   - `updater-manifest` 不存在时创建为 prerelease，然后 `--clobber` 上传；
   - 这样 draft 期间清单 URL 不会指向 404：事件只在人工发布后触发。

5. **ci.yml 回归**：
   - `updater-manifest` job 运行 `node --test scripts/gen-update-manifest.test.mjs`；
   - `audit` job 运行 `pnpm audit --prod --audit-level high`，并进入 `summary` 的 `needs`。

产物路径参考（`publish` 下载到 `artifacts/` 下）：

```
artifacts/win-x64[-signed]/bundle/nsis/*.exe
artifacts/win-x64[-signed]/lc-cli.exe
artifacts/linux-x64/bundle/appimage/*.AppImage
artifacts/linux-x64/bundle/deb/*.deb
artifacts/linux-x64/lc-cli
artifacts/linux-arm64/bundle/appimage/*.AppImage
artifacts/linux-arm64/bundle/deb/*.deb
artifacts/linux-arm64/lc-cli-arm64
artifacts/latest.json
artifacts/SHA256SUMS
```


## 7. 回滚步骤（人工，不自动降级）

当前版本出问题时：

1. 从 `latest.json` 的 `previous` 取上一版的 `version`、`url`、`sha256`、`signature`。
   如果本地没有清单，直接打开对应 tag 的 GitHub Release 页面。
2. 下载上一版安装包，核对 `sha256`：

   ```bash
   sha256sum Lite.Code_0.6.0-preview.1_x64-setup.exe
   # Windows: certutil -hashfile <file> SHA256
   ```

3. 有 `signature` 时用 minisign 公钥验证（Tauri CLI 2.12 只有
   `signer generate/sign`，没有 `verify` 子命令）：

   ```bash
   minisign -Vm <installer> -P <public-key>
   ```

   校验失败或 `signed:false` 时不要安装。后续接入 `tauri-plugin-updater`
   后，插件会用内置 `pubkey` 自动完成同样的校验。
4. 退出 Lite Code（托盘“退出”），安装上一版覆盖当前版本：
   - Windows：运行上一版 NSIS 安装包；
   - Linux AppImage：替换现有 AppImage 并恢复可执行位；
   - Linux deb：`sudo apt install ./<旧版本>.deb`（必要时先卸载当前版本）。
5. 回滚后不要立刻再次触发更新检查，否则 UI 会再次提示最新版。是否允许降级
   必须由用户显式确认，应用不做自动降级。

## 8. 已知限制

- **不自动安装**：检查命令只返回信息；安装与签名校验留给后续里程碑。
- **签名未闭环**：`signed:false` 时门禁不关闭；`signed:true` 也只表示清单携带
  `.sig`，客户端尚未做密码学校验。
- **回滚只有一层**：`previous` 只保留上一版；更早版本从 GitHub Releases 取。
- **预览版不能走 `/releases/latest/`**：GitHub 的 latest 忽略 prerelease，
  所以清单放在固定 tag 的滚动 release（`updater-manifest`）。
- **draft 时序**：release.yml 只创建 draft；draft 未发布时其资产不可公开访问，
  滚动清单由 `updater-manifest.yml` 在 `release: published` 后更新（§4 第 5 步）。
- **平台覆盖**：仅 Windows x64、Linux x64、Linux arm64；macOS 与其他架构返回
  `not_implemented`。
- **清单一致性**：`sha256` 与 URL 必须来自同一批已上传产物；先生成清单后重命名/
  重新打包会导致校验失败。发布时不要改写产物文件名（空格由脚本 URL 编码）。
- **无增量更新**：每次都是完整安装包。
- **未签名清单不能被 tauri-plugin-updater 直接消费**：该插件会校验清单中每个
  平台的 `signature` 非空；`signed:false` 的清单只适用于本 lane 的检查通道。
  迁移到插件前必须先接入签名密钥并生成 `.sig`。
- **本地覆盖开关**：`LC_UPDATE_MANIFEST_URL` 面向开发/测试，不应在正式包中暴露
  任意 URL 的来源；它只影响检查，不执行下载。
