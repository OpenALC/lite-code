# main 整改复审（2026-11-10）

**M3b 复审通过；建议暂缓从当前主线发布 preview.65。** 本次比较上次审查基线 `371e707` 与本地、远端一致的 `main@26d4f22`。M3b 已由 `be84ab6` 合并，无需再次合并。发布风险来自其后的设置浮层改动 `9b8a51d`。

## 上次三条意见

| 意见 | 复核结果 |
| --- | --- |
| 计划文档冲突 | 已关闭。[抽取计划](../plans/lc-host-extraction.md)同时保留 M3b 实际落在 lc-core 和 M3c 决定不做，且已删除“待合并”字样。 |
| 完成事件说明 | 已关闭。计划分别说明正常结束发一次 done、等待审批保持 busy、拒绝发权限错误，并保留鸿蒙的宿主差异；与[核心实现](../../crates/lc-core/src/agent/events.rs)一致。 |
| 发布说明夸大 M2 范围 | 已关闭。[preview.65 说明](../releases/0.6.1-preview.65.md)已区分鸿蒙会话表迁移、两端信封复用，补入 M3b，并明确 M3c 不做。 |

## 同期新增问题

**P1 · 在一个入口保存供应商，会被另一个入口的旧配置覆盖。** [设置面板](../../apps/web/src/components/SettingsPanel.vue)第 337 行新增第二个 ProvidersPanel；[主窗口](../../apps/web/src/App.vue)第 1328 行的原实例通过 `v-show` 隐藏，一直挂载。[编辑器](../../apps/web/src/components/ProvidersPanel.vue)仅在挂载时读取配置，保存时发送完整供应商和模型列表；另一实例不订阅保存后的变更。[后端](../../crates/lc-core/src/llm/providers_view.rs)按传入列表重建文件，只为传入的供应商按 ID 保留已有密钥。从已有有效配置开始，在浮层新增供应商并保存，再到活动栏的供应商页修改旧供应商并保存，新供应商便会从文件中消失。建议将两处入口指向同一个编辑实例。验收时从两个入口连续保存，重开后应仍能看到两次修改。此项依据前后端读写路径确认，未改写真实供应商配置。

**P1 · 独立 demo 启动即中断。** 用 Chrome 实际打开 `demo/index.html?view=settings`，控制台在[第 3456 行](../../demo/index.html#L3456)报 `Cannot read properties of null (reading 'addEventListener')`。脚本立即执行，而设置浮层的 HTML 在脚本结束后才解析（第 3936 行）；此时缩放按钮尚不存在，后续快捷键绑定和初始布局也没有执行。建议将浮层标记放在初始化脚本前，或让整段初始化在 DOM 就绪后执行。普通入口和设置直达入口都应能完成初始化。

**P2 · 设置打开后，键盘仍会操作背景编辑器。** [主窗口快捷键](../../apps/web/src/App.vue)第 855 行只在 Escape 时处理浮层；在浮层内按 Ctrl+W 仍会请求关闭后台活动标签，Ctrl+Tab 仍会切换后台文件。通过 Ctrl+, 打开设置也不会主动把焦点移入对话框。建议打开时将焦点移入并限制在浮层内，屏蔽后台编辑快捷键，关闭后恢复原焦点。

**P2 · 从旧版本恢复的设置标签变成空白编辑器。** 旧版本会将 `__settings__` 存入 `lite-code.tabs.v1`；[恢复逻辑](../../apps/web/src/services/tabsPersistence.ts)仍保留它，而[新渲染分支](../../apps/web/src/App.vue)第 1329–1330 行会将它交给 CodeEditor。升级前留着设置页的用户会看到名为“设置”的空白缓冲区。建议恢复时只移除废弃的设置标签并重选有效活动页，保留其他标签及未保存内容。

## 验证与发布记录

[当前主线 CI](https://github.com/OpenALC/lite-code/actions/runs/42738711281) 已成功，包括 Web 及 Windows/Linux Rust 测试。上述 demo 异常在该提交上独立复现；其余三项为代码审查发现。本次未修改产品代码、打标签或发布。

发版前还应更新说明中的验证记录：[`be84ab6` 的 CI](https://github.com/OpenALC/lite-code/actions/runs/42736749832)实际为 cancelled，通过的是[仅追加文档后的 `8912d21`](https://github.com/OpenALC/lite-code/actions/runs/42736890843)和当前主线。若从当前主线打标，说明中的“无 web 变更”也应更新，列明设置浮层的实际变更。
