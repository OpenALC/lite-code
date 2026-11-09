# 鸿蒙真机签名准备（维护者事项）

> 目标：让 `apps/ohos` 的 HAP 能从「仅模拟器」走到「真机可装」。
> 未签名 HAP 仅模拟器可装（真机报 `9568320 no signature file`，已实测）。

## 路径 A：DevEco 自动签名（开发/内测，最快）

1. 打开 DevEco Studio → `apps/ohos` 工程 → **File → Project Structure → Signing Configs**。
2. 勾选 **Automatically generate signature**（需登录华为账号并完成实名认证）。
3. 连接真机（USB 调试 + 允许 hdc），DevEco 自动生成本地证书/p7b/profile 并写入
   `build-profile.json5` 的 `signingConfigs`。
4. `node scripts/ohos/build.mjs` 出签名 HAP；`hdc install` 到真机。

**入库纪律（重要）**：
- `build-profile.json5` 的 `signingConfigs` 在仓库中**必须保持为空数组**；
  本地签名后提交前 `git diff` 检查，出现 `storePassword/keyPassword/p12/p7b` 等
  字段即还原（build.mjs 已有签名字段守卫，发现即失败）。
- 个人自动签名材料的 profile 绑定设备 UDID，仅本机有效，入库无意义且有泄露风险。

## 路径 B：发布签名（CI/分发，需 AGC 账号）

1. [AppGallery Connect](https://developer.huawei.com/consumer/cn/service/josp/agc/index.html)
   创建应用（包名 `dev.litecode.ohos`）→ 生成**发布证书**（.cer）与**发布 profile**（.p7b）。
2. 证书私钥（.p12）+ 密码存入 **GitHub Secrets**：
   `OHOS_SIGN_P12`（base64）、`OHOS_SIGN_P12_PASSWORD`、`OHOS_SIGN_CER`、`OHOS_SIGN_PROFILE`。
3. CI 工作流（新增 `.github/workflows/ohos.yml`，方案 §6.3）：
   checkout → 写 secrets 到临时文件（0600）→ `jq` 注入 `build-profile.json5`
   signingConfigs → `build.mjs` → 上传签名 HAP 为 artifact（不进 release 资产，
   直到上架策略确定）。
4. 版本 `versionName` 须与 AGC 上架版本一致；`versionCode` 单调递增
   （build.mjs 当前用 `1000000`，需改为读取 git tag 序号）。

## 验收清单

- [ ] 真机（arm64）安装路径 A 签名 HAP 成功，App 启动、对话可用。
- [ ] 真机中文 IME 输入验证（计划文档风险 #4：IME 不可用 = 产品不可用）。
- [ ] CI 路径 B 出签名 HAP artifact；secrets 不入库、不进日志。
- [ ] `build-profile.json5` 在 main 始终 `signingConfigs: []`（CI 注入为运行时行为）。

## 参考

- 方案文档：`docs/plans/ohos-shell.md` §6.2（签名表）、§7（风险 #8）。
- 设备侧已验证：模拟器 API 26（手机/平板）；真机 `192.168.31.178`（MatePad Mini）
  连接正常，等签名包。
