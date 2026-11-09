# Lite Code · HarmonyOS 外壳

`apps/ohos` 是鸿蒙（ArkWeb）产品工程的骨架，从
`spikes/harmony-arkweb` 复制并改造。首版目标是「只读 + 对话」，本切片只覆盖
工程骨架与资源加载，不包含前端 `OhosApp`、NAPI 桥和真实 core。

## 目录

```
AppScope/app.json5                    bundleName: dev.litecode.ohos
build-profile.json5                   signingConfigs: []（只出 unsigned HAP）
hvigorw / hvigorw.bat                 启动 DevEco 自带的 hvigor
hvigor/hvigor-config.json5           modelVersion 固定为 5.0.0
entry/src/main/module.json5          INTERNET；phone/tablet/2in1
entry/src/main/ets/pages/Index.ets   虚拟 origin + onInterceptRequest
entry/src/main/resources/rawfile/    ohos.html 占位 + 构建产物 web/（gitignore）
entry/src/main/cpp/                  NAPI 占位（Phase 3 再接入 lc-ohos）
```

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
