// 标签页状态：open / close / activate / dirty 跟踪（UI.md §3、P1 Editor UI lane）。
// 关闭保护：dirty 标签的 closeTab 不真正关闭、不弹窗，只返回 needsConfirm，
// 由视图层决定确认交互（UI.md §7：不用模态框打断流程）。
// 框架无关：按 services/store.ts 的模式只暴露纯函数与 subscribe()（UI.md §8）。
import { createStore, type Store } from './store'

export interface Tab {
  /** 稳定标识（真实文件接入后为文件路径） */
  id: string
  /** 展示名（文件名属数据，不走 i18n key） */
  title: string
  dirty: boolean
}

export interface TabsState {
  tabs: Tab[]
  /** 当前激活标签 id；无标签时为 null */
  activeId: string | null
}

export type CloseResult =
  | { kind: 'closed' }
  | { kind: 'needsConfirm'; tabId: string }
  | { kind: 'notFound' }

export interface CloseOptions {
  /** 跳过 dirty 保护强制关闭：视图层向用户确认后的二次调用使用 */
  force?: boolean
}

/**
 * 初始状态：P0 的样例缓冲区（与 CodeEditor.vue 的 SAMPLE 对应）。
 * 所有操作都是不可变更新，复位可直接 set(INITIAL_TABS_STATE)。
 */
export const INITIAL_TABS_STATE: TabsState = {
  tabs: [{ id: 'sample', title: 'main.rs', dirty: false }],
  activeId: 'sample',
}

export const tabsStore: Store<TabsState> = createStore<TabsState>(INITIAL_TABS_STATE)

/** 打开标签：已存在同 id 时不重复创建，只激活（保留原 dirty 状态）。 */
export function openTab(tab: { id: string; title: string }): void {
  const state = tabsStore.get()
  if (state.tabs.some(t => t.id === tab.id)) {
    tabsStore.set({ ...state, activeId: tab.id })
    return
  }
  tabsStore.set({
    tabs: [...state.tabs, { id: tab.id, title: tab.title, dirty: false }],
    activeId: tab.id,
  })
}

/** 激活标签。id 不存在时不改变状态，返回 false。 */
export function activateTab(id: string): boolean {
  const state = tabsStore.get()
  if (!state.tabs.some(t => t.id === id)) return false
  if (state.activeId === id) return true
  tabsStore.set({ ...state, activeId: id })
  return true
}

/** dirty 跟踪：编辑内容变化置 true，保存后置 false。id 不存在返回 false。值不变时不发通知。 */
export function setDirty(id: string, dirty: boolean): boolean {
  const state = tabsStore.get()
  const index = state.tabs.findIndex(t => t.id === id)
  if (index < 0) return false
  const tab = state.tabs[index]
  if (tab.dirty === dirty) return true
  const tabs = state.tabs.map(t => (t.id === id ? { ...t, dirty } : t))
  tabsStore.set({ ...state, tabs })
  return true
}

/**
 * 关闭标签（带 dirty 保护）：
 * - 干净 → 移除并返回 closed；若关闭的是激活标签，激活右邻，无右邻则左邻，最后置 null
 * - dirty 且未 force → 不关闭，返回 needsConfirm（视图层决定如何确认）
 * - id 不存在 → notFound
 */
export function closeTab(id: string, options: CloseOptions = {}): CloseResult {
  const state = tabsStore.get()
  const index = state.tabs.findIndex(t => t.id === id)
  if (index < 0) return { kind: 'notFound' }
  const tab = state.tabs[index]
  if (tab.dirty && !options.force) {
    return { kind: 'needsConfirm', tabId: id }
  }
  const tabs = state.tabs.filter(t => t.id !== id)
  let activeId = state.activeId
  if (activeId === id) {
    const right = index < tabs.length ? tabs[index] : undefined
    const left = index > 0 ? tabs[index - 1] : undefined
    activeId = (right ?? left)?.id ?? null
  }
  tabsStore.set({ tabs, activeId })
  return { kind: 'closed' }
}
