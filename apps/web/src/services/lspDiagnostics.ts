// LSP 诊断缓存：Rust 事件 → 按文件聚合 → Monaco markers。
// 框架无关，CodeEditor/App 通过 subscribe 消费（UI.md §8）。
import type { LspDiagnostic } from './bridge'
import { createStore, type Store } from './store'

export type DiagnosticsByPath = Record<string, Record<string, LspDiagnostic[]>>

const store: Store<DiagnosticsByPath> = createStore<DiagnosticsByPath>({})

/** 按 path + server 存储；多个 server 对同一文件的诊断会合并展示。 */
export function setFileDiagnostics(
  path: string,
  serverId: string,
  diagnostics: LspDiagnostic[],
): void {
  if (!path || !serverId) return
  const current = store.get()
  const servers = { ...(current[path] ?? {}) }
  if (diagnostics.length === 0) delete servers[serverId]
  else servers[serverId] = diagnostics
  const next = { ...current }
  if (Object.keys(servers).length === 0) delete next[path]
  else next[path] = servers
  store.set(next)
}

export function diagnosticsFor(path: string | null | undefined): LspDiagnostic[] {
  if (!path) return []
  return Object.values(store.get()[path] ?? {}).flat()
}

export interface ProblemItem {
  path: string
  serverId: string
  diagnostic: LspDiagnostic
}

/** 把按文件/服务器聚合的诊断摊平成问题面板列表，按路径、行、列排序。 */
export function flattenDiagnostics(state: DiagnosticsByPath): ProblemItem[] {
  const items: ProblemItem[] = []
  for (const [path, servers] of Object.entries(state)) {
    for (const [serverId, diagnostics] of Object.entries(servers)) {
      for (const diagnostic of diagnostics) {
        items.push({ path, serverId, diagnostic })
      }
    }
  }
  return items.sort(
    (a, b) =>
      a.path.localeCompare(b.path) ||
      a.diagnostic.line - b.diagnostic.line ||
      a.diagnostic.column - b.diagnostic.column,
  )
}

export function clearDiagnostics(): void {
  store.set({})
}

export function subscribeDiagnostics(fn: (state: DiagnosticsByPath) => void): () => void {
  return store.subscribe(fn)
}
