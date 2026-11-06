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
/** 已被编辑器消费的最大 seq：组件重挂载时不会重放旧定位。 */
let consumedSeq = 0

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

/**
 * 取一次尚未消费、且属于 `tabId` 的定位请求。编辑器应用后即标记已消费，
 * 避免从设置页/其它标签返回时又跳回上一次命中。
 */
export function consumeReveal(tabId: string): RevealTarget | null {
  const target = store.get()
  if (!target || target.tabId !== tabId || target.seq <= consumedSeq) return null
  consumedSeq = target.seq
  return target
}

export function subscribeReveal(fn: (target: RevealTarget | null) => void): () => void {
  return store.subscribe(fn)
}
