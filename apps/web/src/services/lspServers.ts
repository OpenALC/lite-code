// 已启动 LSP server 的语言 id 注册表（docs/EXTENSIONS.md §3）。
// App.vue 在 lsp_start_* 回报后写入；CodeEditor 据此决定是否为某语言注册
// 补全 / 悬停 / 跳转 provider——无 server 的语言不注册，Monaco 保留内置建议。
// 框架无关（UI.md §8）：只暴露纯函数与 subscribe。
import { createStore, type Store } from './store'

const store: Store<readonly string[]> = createStore<readonly string[]>([])

/** 全量替换当前有 server 服务的语言 id 列表（通常来自 LspStartReport）。 */
export function setLspLanguages(languageIds: readonly string[]): void {
  const next = [...new Set(languageIds.filter(id => typeof id === 'string' && id.length > 0))]
  store.set(next)
}

export function lspLanguages(): readonly string[] {
  return store.get()
}

/** 该语言 id 是否有已启动的 LSP server。 */
export function hasLspLanguage(languageId: string): boolean {
  return store.get().includes(languageId)
}

export function subscribeLspLanguages(fn: (languageIds: readonly string[]) => void): () => void {
  return store.subscribe(fn)
}
