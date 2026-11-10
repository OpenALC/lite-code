# preview.65 发布复核

**结论：四项中的三项可以关闭，设置浮层的键盘隔离仍未完成。建议暂缓发布。** 复核范围为 `26d4f22..c2db89b`，本地和远端 main 均指向 `c2db89b`。该提交的[主线 CI 已通过](https://github.com/OpenALC/lite-code/actions/runs/42771226481)。

| 原问题 | 判定 | 依据 |
| --- | --- | --- |
| 供应商配置被另一入口的旧副本覆盖 | 已解决 | [App.vue](../../apps/web/src/App.vue) 的活动栏、命令和 Agent 入口现在全部打开设置浮层；原先用 `v-show` 常驻的第二个编辑器已删除。桌面仅有[设置分区](../../apps/web/src/components/SettingsPanel.vue)挂载供应商编辑器。核对了代码路径，未读写真实供应商配置。 |
| 独立 demo 启动中断 | 已解决 | 浮层标记已移到初始化脚本前。本次用 Chrome 打开原复现地址 `demo/index.html?view=settings`，页面完成初始化，输出的浮层已移除 `hidden`，未出现上一轮的空节点异常。 |
| 浮层允许键盘操作后台 | 部分解决 | 打开时焦点移入，App 自身的 Ctrl+W / Ctrl+Tab 分支会跳过；独立的窗口监听和反向 Tab 仍可绕过隔离，见下文。 |
| 旧设置标签恢复为空白编辑器 | 已解决 | 恢复时过滤 `__settings__` 和 `__providers__`，若旧活动页被过滤则选取剩余标签，普通标签仍恢复内容和保存基线。本次运行[持久化测试](../../apps/web/src/services/tabsPersistence.spec.ts)，7/7 通过。 |

## 未关闭项：键盘隔离

- **后台保存及抢焦点。** [App.vue](../../apps/web/src/App.vue) 第 884 行的提前返回只退出自己的监听器。[CodeEditor.vue](../../apps/web/src/components/CodeEditor.vue) 第 264、363 行另行监听窗口 Ctrl+S；有未保存的真实文件时，在设置中按 Ctrl+S 仍可触发 `writeFile`。在 Agent 完整模式，[AgentPanel.vue](../../apps/web/src/components/AgentPanel.vue) 第 465 行也单独监听 Ctrl+F，并通过 `openFind()` 将焦点移到后台搜索框。这两条结论来自事件注册与处理路径，没有对用户文件执行保存。
- **打开后立即 Shift+Tab。** [App.vue](../../apps/web/src/App.vue) 第 772 行先聚焦对话框容器，但第 792 行仅在焦点是第一个控件或已在容器外时拦截反向 Tab。容器自身不满足任一条件，背景又未设为不可聚焦，因此第一次反向 Tab 即可离开浮层。

建议在模态边界统一隔离后台快捷键，让输入框原生编辑键正常工作；修正 Tab 首尾循环，只计入可聚焦控件。验收时分别覆盖：打开后立即 Shift+Tab 仍在浮层内、Ctrl+S 不保存后台脏文件、Agent 模式 Ctrl+F 不夺走焦点，以及 Esc 关闭后快捷键恢复。

## 版本门禁

远端尚无 `v0.6.1-preview.65` 标签。当前三个发布清单仍为 `0.6.1-preview.64`；本次执行 `node scripts/check-version.mjs --release --tag v0.6.1-preview.65`，因标签与清单不一致返回 1。这是打标前的正常版本准备步骤，与上述功能缺陷独立。修复键盘隔离后，应按[版本流程](../VERSIONING.md)提交独立的 .65 版本变更，待该提交通过 CI 和发布版本校验，再打标签。本次未修改产品或发布状态。
