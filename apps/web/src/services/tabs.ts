// 标签页状态：open / close / activate / 内容与 dirty 跟踪（UI.md §3、P1 Editor UI lane）。
// 关闭保护：dirty 标签的 closeTab 不真正关闭、不弹窗，只返回 needsConfirm，
// 由视图层决定确认交互（UI.md §7：不用模态框打断流程）。
// 框架无关：按 services/store.ts 的模式只暴露纯函数与 subscribe()（UI.md §8）。
import { basenamePath, isPathWithin, replacePathPrefix } from './fileTree'
import { resolveEditorStrategy, type EditorFileStrategy } from './largeFile'
import { createStore, type Store } from './store'

export interface Tab {
  /** 稳定标识（真实文件为文件路径；样例缓冲区为 sample） */
  id: string
  /** 展示名（文件名属数据，不走 i18n key） */
  title: string
  /** 真实文件路径；null = 非文件缓冲区（P0 样例），保存时跳过 */
  path: string | null
  /** 当前缓冲区内容（保存时直接取这里，避免视图层与 store 双份状态） */
  content: string
  /** 最近一次落盘/打开时的内容基线：dirty = content !== savedContent */
  savedContent: string
  /** 大文件分级策略（ARCHITECTURE.md §5.1）：能力裁剪与只读 */
  strategy: EditorFileStrategy
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
  /** 是否进入「重新打开」栈；文件已删除等场景传 false。默认 true。 */
  recordClosed?: boolean
}

interface ClosedTab {
  tab: Tab
  index: number
}

const CLOSED_LIMIT = 10
const closedTabs: ClosedTab[] = []

/** 清空「重新打开」栈（测试复位用；生产由进程生命周期自然清理）。 */
export function clearClosedTabs(): void {
  closedTabs.length = 0
}

/** P0 样例缓冲区内容（与 main.rs 标签对应；不可保存，仅用于起壳展示）。 */
export const SAMPLE_CONTENT = `// lite-code · P0
// 这个缓冲区证明 Monaco 起壳成功：能打开、能编辑。
// 文件系统集成（打开真实文件 / 保存 / 冲突检测）在 P1 落地。
fn main() {
    println!("Hello, lite-code");
}
`

/**
 * 初始状态：P0 的样例缓冲区（非真实文件，path = null）。
 * 所有操作都是不可变更新，复位可直接 set(INITIAL_TABS_STATE)。
 */
export const INITIAL_TABS_STATE: TabsState = {
  tabs: [
    {
      id: 'sample',
      title: 'main.rs',
      path: null,
      content: SAMPLE_CONTENT,
      savedContent: SAMPLE_CONTENT,
      strategy: resolveEditorStrategy(0),
      dirty: false,
    },
  ],
  activeId: 'sample',
}

export const tabsStore: Store<TabsState> = createStore<TabsState>(INITIAL_TABS_STATE)

export interface OpenTabInput {
  id: string
  title: string
  /** 缺省为 id（真实文件）；样例缓冲区显式传 null */
  path?: string | null
  content?: string
  /** 缺省按完整能力（样例/未知大小） */
  strategy?: EditorFileStrategy
}

/** 打开标签：已存在同 id 时不重复创建，只激活（保留原内容、策略与 dirty 状态）。 */
export function openTab(tab: OpenTabInput): void {
  const state = tabsStore.get()
  if (state.tabs.some(t => t.id === tab.id)) {
    tabsStore.set({ ...state, activeId: tab.id })
    return
  }
  tabsStore.set({
    tabs: [
      ...state.tabs,
      {
        id: tab.id,
        title: tab.title,
        path: tab.path === undefined ? tab.id : tab.path,
        content: tab.content ?? '',
        savedContent: tab.content ?? '',
        strategy: tab.strategy ?? resolveEditorStrategy(0),
        dirty: false,
      },
    ],
    activeId: tab.id,
  })
}

/**
 * 文件系统移动/重命名成功后同步标签：路径命中 from（目录含全部后代）的标签
 * 更新 id / path / title，激活标签的 id 一并跟随。返回是否有变化。
 * 内容、dirty 与编辑器策略保持不变。
 */
export function relocateTabs(from: string, to: string): boolean {
  if (!from || !to || from === to) return false
  const state = tabsStore.get()
  const idMap = new Map<string, string>()
  let changed = false

  const tabs = state.tabs.map(tab => {
    if (tab.path === null || !isPathWithin(from, tab.path)) return tab
    const nextPath = replacePathPrefix(tab.path, from, to)
    if (!nextPath || nextPath === tab.path) return tab
    const nextId = isPathWithin(from, tab.id)
      ? (replacePathPrefix(tab.id, from, to) ?? tab.id)
      : tab.id
    if (nextId !== tab.id) idMap.set(tab.id, nextId)
    changed = true
    return { ...tab, id: nextId, path: nextPath, title: basenamePath(nextPath) }
  })

  if (!changed) return false
  const activeId =
    state.activeId !== null ? (idMap.get(state.activeId) ?? state.activeId) : null
  tabsStore.set({ tabs, activeId })
  return true
}

/**
 * 文件系统删除成功后关闭被删路径上的标签（目录含全部后代）。
 * 磁盘内容已不存在，dirty 标签也强制关闭；返回关闭数量。
 */
export function removeTabsUnder(path: string): number {
  if (!path) return 0
  const ids = tabsStore
    .get()
    .tabs.filter(tab => tab.path !== null && isPathWithin(path, tab.path))
    .map(tab => tab.id)
  for (const id of ids) closeTab(id, { force: true, recordClosed: false })
  return ids.length
}

export type TabDropPosition = 'before' | 'after'

/**
 * 拖拽排序：把 fromId 移到 toId 之前/之后。
 * 位置未变化、id 不存在或拖到自身时返回 false，不触发订阅通知。
 */
export function moveTabTo(fromId: string, toId: string, position: TabDropPosition): boolean {
  if (fromId === toId) return false
  const state = tabsStore.get()
  const moved = state.tabs.find(t => t.id === fromId)
  if (!moved || !state.tabs.some(t => t.id === toId)) return false
  const rest = state.tabs.filter(t => t.id !== fromId)
  let index = rest.findIndex(t => t.id === toId)
  if (position === 'after') index += 1
  rest.splice(index, 0, moved)
  if (rest.every((tab, i) => tab === state.tabs[i])) return false
  tabsStore.set({ ...state, tabs: rest })
  return true
}

/** 激活标签。id 不存在时不改变状态，返回 false。 */
export function activateTab(id: string): boolean {
  const state = tabsStore.get()
  if (!state.tabs.some(t => t.id === id)) return false
  if (state.activeId === id) return true
  tabsStore.set({ ...state, activeId: id })
  return true
}

/** 仅更新展示标题（语言切换等）；路径、内容、dirty 不变。 */
export function renameTab(id: string, title: string): boolean {
  const state = tabsStore.get()
  const tab = state.tabs.find(t => t.id === id)
  if (!tab || tab.title === title) return false
  tabsStore.set({
    ...state,
    tabs: state.tabs.map(t => (t.id === id ? { ...t, title } : t)),
  })
  return true
}

/** dirty 跟踪：编辑内容变化置 true，保存后置 false。id 不存在返回 false。值不变时不发通知。 */export function setDirty(id: string, dirty: boolean): boolean {
  const state = tabsStore.get()
  const index = state.tabs.findIndex(t => t.id === id)
  if (index < 0) return false
  const tab = state.tabs[index]
  if (tab.dirty === dirty) return true
  const tabs = state.tabs.map(t => (t.id === id ? { ...t, dirty } : t))
  tabsStore.set({ ...state, tabs })
  return true
}

/** 更新缓冲区内容；dirty 始终由 content 与 savedContent 的差异推导。 */
export function updateContent(id: string, content: string): boolean {
  const state = tabsStore.get()
  const index = state.tabs.findIndex(t => t.id === id)
  if (index < 0) return false
  const tab = state.tabs[index]
  const dirty = content !== tab.savedContent
  if (tab.content === content && tab.dirty === dirty) return true
  const tabs = state.tabs.map(t => (t.id === id ? { ...t, content, dirty } : t))
  tabsStore.set({ ...state, tabs })
  return true
}

/** 保存成功：把当前内容设为新基线并清除 dirty。 */
export function markSaved(id: string): boolean {
  const state = tabsStore.get()
  const index = state.tabs.findIndex(t => t.id === id)
  if (index < 0) return false
  const tab = state.tabs[index]
  if (tab.savedContent === tab.content && !tab.dirty) return true
  const tabs = state.tabs.map(t =>
    t.id === id ? { ...t, savedContent: t.content, dirty: false } : t,
  )
  tabsStore.set({ ...state, tabs })
  return true
}

/** Ctrl+Tab / Ctrl+Shift+Tab 的目标标签；少于两个标签或激活项未知时返回 null。 */
export function adjacentTabId(state: TabsState, direction: 1 | -1): string | null {
  if (state.tabs.length < 2) return null
  const index = state.tabs.findIndex(t => t.id === state.activeId)
  if (index < 0) return state.tabs[0]?.id ?? null
  const next = (index + direction + state.tabs.length) % state.tabs.length
  return state.tabs[next]?.id ?? null
}

/** 当前激活标签；无激活标签时返回 null。 */
export function activeTab(state: TabsState = tabsStore.get()): Tab | null {
  return state.tabs.find(t => t.id === state.activeId) ?? null
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
  if (options.recordClosed !== false) {
    closedTabs.push({ tab: { ...tab }, index })
    if (closedTabs.length > CLOSED_LIMIT) closedTabs.shift()
  }
  tabsStore.set({ tabs, activeId })
  return { kind: 'closed' }
}

/**
 * 重新打开最近关闭的标签（Ctrl+Shift+T）。
 * 已存在同 id 时只激活；否则按关闭时的内容/dirty/策略原样恢复并追加到末尾。
 */
export function reopenClosedTab(): Tab | null {
  while (closedTabs.length > 0) {
    const { tab, index } = closedTabs.pop()!
    const state = tabsStore.get()
    const existing = state.tabs.find(t => t.id === tab.id)
    if (existing) {
      tabsStore.set({ ...state, activeId: existing.id })
      return existing
    }
    const tabs = [...state.tabs]
    tabs.splice(Math.min(index, tabs.length), 0, tab)
    tabsStore.set({ tabs, activeId: tab.id })
    return tab
  }
  return null
}
