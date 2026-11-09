# Lite Code · HarmonyOS 外壳

`apps/ohos` 是鸿蒙（ArkWeb）产品工程，从
`spikes/harmony-arkweb` 复制并改造。首版「只读 + 对话」：工程骨架、虚拟 origin
资源管线、NAPI 桥与真实 core（`crates/lc-ohos` 静态库）均已接入；模拟器
（Relay_Phone_API26）端到端验证通过——真实 fs 文件树、三级目录只读预览、
stub 流式对话（20 delta + done）与取消（14ms）。

## 目录

```
AppScope/app.json5                    bundleName: dev.litecode.ohos
build-profile.json5                   signingConfigs: []（只出 unsigned HAP）
hvigorw / hvigorw.bat                 启动 DevEco 自带的 hvigor
hvigor/hvigor-config.json5           modelVersion 固定为 5.0.0
entry/src/main/module.json5          INTERNET；phone/tablet/2in1
entry/src/main/ets/pages/Index.ets   虚拟 origin + onInterceptRequest + Host 初始化
entry/src/main/ets/bridge/           NativeBridge（javaScriptProxy → __lcNative）
entry/src/main/resources/rawfile/    ohos.html 占位 + 构建产物 web/（gitignore）
                                     + sample-workspace/（首启复制到 filesDir）
entry/src/main/cpp/                  NAPI 胶水：lc_ohos.h / napi_init.cpp /
                                     CMakeLists.txt（按 OHOS_ARCH 链接 liblc_ohos.a）
```

## core 链路

```
Vue shim __lcNative.invoke
  → ArkTS NativeBridge (javaScriptProxy，Promise 方法只放 methodList)
  → NAPI invoke（async work）→ lc_ohos_call → Host::dispatch（crates/lc-ohos）
事件：Rust sink → napi_threadsafe_function → ArkTS
  → runJavaScript(window.__lcOhosEmit(envelopeJson)) → shim 回灌 core:event
```

- 首启把 `rawfile/sample-workspace` 复制到 `filesDir/sample-workspace` 作为
  只读工作区；`providers.json` 同步初始化。
- `lc-ohos` 需要 OHOS Rust target（`rustup target add x86_64-unknown-linux-ohos
  aarch64-unknown-linux-ohos`）；`build.mjs` 从 `DEVECO_SDK_HOME` 推导
  clang/llvm-ar 并注入 `CC_<triple>` / `AR_<triple>` / linker 环境变量
  （路径含空格时不能走 RUSTFLAGS）。
- 命令白名单与未实现命令（`not_implemented`）见 `crates/lc-ohos/src/lib.rs`
  与 `docs/plans/ohos-shell.md` §5.4。

## 资源加载

产品模式用保留域名 `https://app.litecode.example/ohos.html` 作为虚拟
origin。`Index.ets` 的 `onInterceptRequest` 只服务该 origin：

- 从 `rawfile/web/<path>` 读取资源；`rawfile/web` 由
  `scripts/ohos/build.mjs` 从 `apps/web/dist-ohos` 同步；
- 显式 MIME 表（html/js/css/json/svg/png/woff2/wasm/map 等），未知扩展名
  回 `application/octet-stream`；
- 拒绝 `..`、`\`、NUL、绝对路径、盘符和 scheme；未命中返回 404 response，
  不落到真实网络；
- 当 `rawfile/web/ohos.html` 不存在时，回退到已提交的
  `rawfile/ohos.html` 占位页，供 `--skip-web` 构建和离线冒烟使用。

Dev 模式把 `Index.ets` 里的 `USE_DEV_SERVER` 改为 `true`，WebView 会加载
`http://10.0.2.2:5173/ohos.html`（模拟器访问宿主机 Vite），此时不注册拦截。

## 构建

```bash
export DEVECO_SDK_HOME='D:\ide\Huawei\DevEco Studio\sdk'
export HVIGOR_USER_HOME='C:\Users\<you>\AppData\Local\Temp\lc-hvigor'

node scripts/ohos/build.mjs            # 前端 + 资源 + ohpm + hvigor
node scripts/ohos/build.mjs --skip-web # 复用现有 rawfile/web 或占位页
```

产物：

```
entry/build/default/outputs/default/entry-default-unsigned.hap
```

安装到模拟器：

```bash
HDC='D:\ide\Huawei\DevEco Studio\sdk\default\openharmony\toolchains\hdc.exe'
"$HDC" install -r entry/build/default/outputs/default/entry-default-unsigned.hap
"$HDC" shell aa start -a EntryAbility -b dev.litecode.ohos
```

未签名 HAP 只用于模拟器。真机签名材料（p12/p7b/cer 等）绝不入库，
`build-profile.json5` 保持 `signingConfigs: []`；`build.mjs` 会检查签名字段和
签名文件。

## 环境

详见 [`scripts/ohos/env.md`](../../scripts/ohos/env.md)。hvigor 启动器优先使用
项目内 `hvigor/hvigor-wrapper.js`，否则从 `DEVECO_HOME` 或
`DEVECO_SDK_HOME/..` 推导 DevEco 安装目录，调用其
`tools/hvigor/bin/hvigorw.js`。
