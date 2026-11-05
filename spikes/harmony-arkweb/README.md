# HarmonyOS ArkWeb spike（P6）

验证 P6 门禁三件事：ArkWeb 跑现有前端、NAPI 承载 CoreHandle、Monaco 内存/输入法。
本目录是最小 DevEco 工程，不是产品外壳。

## 环境

- DevEco Studio `D:\ide\Huawei\DevEco Studio`（build `DS-261.23567.138.36.2600865`）
- SDK `sdk/default`：HarmonyOS 26.0.0 / API 26
- 工具：`tools/hvigor/bin/hvigorw.bat`、`tools/ohpm/bin/ohpm.bat`、`tools/node/node.exe`

## 构建与安装

```bash
export DEVECO_SDK_HOME='D:\ide\Huawei\DevEco Studio\sdk'
export HVIGOR_USER_HOME='C:\Users\<you>\AppData\Local\Temp\lc-hvigor'

# 完整前端：先在宿主启动 Vite（模拟器通过 10.0.2.2 访问）
pnpm --filter @lite-code/web exec vite --host 0.0.0.0 --port 5173

# 构建未签名 HAP
'D:\ide\Huawei\DevEco Studio\tools\hvigor\bin\hvigorw.bat' assembleHap \
  --mode module -p product=default -p module=entry@default -p buildMode=debug --no-daemon

# 安装到模拟器/设备
HDC='D:\ide\Huawei\DevEco Studio\sdk\default\openharmony\toolchains\hdc.exe'
"$HDC" install -r entry/build/default/outputs/default/entry-default-unsigned.hap
"$HDC" shell aa start -a EntryAbility -b dev.litecode.spike
```

`Index.ets` 默认加载 `http://10.0.2.2:5173/`（宿主 Vite）。内联页面
`entry/src/main/resources/rawfile/spike/index.html` 可在无 dev server 时验证 ArkWeb 渲染，
把 `src` 换成 `$rawfile('spike/index.html')` 即可。

## 结论（2026-11-05，Relay_Phone_API26 模拟器）

| 项 | 结果 | 证据 |
|---|---|---|
| ArkWeb 跑现有前端 | ✅ | 现有 `apps/web` 通过 HTTP 加载，布局、文件树、编辑器、任务/状态栏均渲染 |
| rawfile 直载 Vite 产物 | ❌ | `resource://rawfile` 下 ES module/CSS 被 CORS 拒绝（origin null） |
| NAPI 承载句柄 | ✅ | `libentry.so` 的 `createHandle()` 返回 `{id,native}`，ArkTS 顶部显示 `core-handle-42` |
| Monaco 渲染 | ✅ | 语法高亮、折叠、缩略图、编辑器布局可见 |
| Monaco 内存 | ⚠️ | 应用 PSS ~142MB + ArkWeb render PSS ~275MB（模拟器，完整前端 + Monaco） |
| Monaco 输入法 | ⚠️ | `uitest uiInput inputText` 未见文本落盘；需真机/输入法人工复核 |

## 决策

按 `docs/MILESTONES.md` 的鸿蒙门禁：三项全过才做独立外壳；只过前两项则交付
“只读 + 对话”，编辑器降级为代码高亮预览。本 spike 归为**只过前两项**：

- 保留 ArkWeb + NAPI 的只读代码预览与会话能力；
- 不在鸿蒙首版启用可编辑 Monaco（内存与 IME 风险）；
- 若后续优化到可编辑，再复用 `apps/web` 完整产物。
