# 鸿蒙构建环境

## 必需变量

| 变量 | 示例 | 用途 |
|---|---|---|
| `DEVECO_SDK_HOME` | `D:\ide\Huawei\DevEco Studio\sdk` | SDK；`build.mjs` 会从它推导 DevEco 根目录 |
| `HVIGOR_USER_HOME` | `C:\Users\<you>\AppData\Local\Temp\lc-hvigor` | hvigor 缓存；不写用户目录 |
| `DEVECO_HOME`（可选） | `D:\ide\Huawei\DevEco Studio` | SDK 不在 DevEco 目录下时用于定位 hvigor |

Git Bash：

```bash
export DEVECO_SDK_HOME='D:\ide\Huawei\DevEco Studio\sdk'
export HVIGOR_USER_HOME="$TEMP/lc-hvigor"   # 或显式 Windows 路径
```

## 工具

- hvigor：`<DevEco>/tools/hvigor/bin/hvigorw.js`，由项目根目录的
  `hvigorw` / `hvigorw.bat` 启动。`hvigor/hvigor-config.json5` 固定
  `modelVersion: 5.0.0`，与 DevEco Studio 26（hvigor 6.26.9）配套。
- ohpm：`<DevEco>/tools/ohpm/bin/ohpm.bat`（也可用 `OHPM_HOME` 覆盖）。
- hdc：`<SDK>/default/openharmony/toolchains/hdc.exe`。

## 模拟器

DevEco 26 已部署的实例：`Relay_Phone_API26`（x86_64）、`Relay_Tablet_API26`。

```bash
EMU='D:\ide\Huawei\DevEco Studio\tools\emulator\Emulator.exe'
"$EMU" -list
"$EMU" -start Relay_Phone_API26        # 无窗口：加 -noWindow
"$EMU" -stop Relay_Phone_API26
```

同时连着真机时，用 `hdc list targets -v` 找模拟器地址（通常是
`127.0.0.1:5555`），再显式指定目标：

```bash
HDC='D:\ide\Huawei\DevEco Studio\sdk\default\openharmony\toolchains\hdc.exe'
"$HDC" -t 127.0.0.1:5555 install -r apps/ohos/entry/build/default/outputs/default/entry-default-unsigned.hap
"$HDC" -t 127.0.0.1:5555 shell aa start -a EntryAbility -b dev.litecode.ohos
```

未签名 HAP 只能装在模拟器；真机会报 `no signature file`。

## 命令

```bash
node scripts/ohos/build.mjs --skip-web
node scripts/ohos/build.mjs
```

`--skip-web` 不运行 `pnpm --filter @lite-code/web build:ohos`，也不清空
`rawfile/web`；它使用已有的 web 产物，若没有则回退到
`entry/src/main/resources/rawfile/ohos.html` 占位页。

完整构建要求 `apps/web/package.json` 已有 `build:ohos` 脚本；缺失时脚本会给出
明确错误，不静默降级。
