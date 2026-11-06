// 编辑器定位请求：搜索命中、问题面板等把"打开到某行"的意图交给编辑器，
// 避免视图层直接持有 Monaco 实例。载荷带 seq，重复点击同一行也能触发。
// 框架无关（UI.md §8）：只暴露纯函数与 subscribe。
import { createStore, type Store } from './store'

export interface RevealTarget {
  /** 目标标签 id（真实文件为路径） */
  tabId: string
  line: number
  column: number
  seq: number
}

const store: Store<RevealTarget | null> = createStore<RevealTarget | null>(null)

let seq = 0

export function requestReveal(tabId: string, line: number, column = 1): void {
  if (!tabId) return
  seq += 1
  store.set({
    tabId,
    line: Math.max(1, Math.trunc(line)),
    column: Math.max(1, Math.trunc(column)),
    seq,
  })
}

export function currentReveal(): RevealTarget | null {
  return store.get()
}

export function subscribeReveal(fn: (target: RevealTarget | null) => void): () => void {
  return store.subscribe(fn)
}
