// 标签状态持久化：窗口销毁/重建后恢复未保存缓冲区（P1 过渡）。
// P3 起会话与缓冲区迁到 Rust 侧后，本模块由契约命令替代（docs/ARCHITECTURE.md §6）。
import { strategyForTier, type LargeFileTier } from './largeFile'
import { tabsStore, type Tab, type TabsState } from './tabs'

export const TABS_STORAGE_KEY = 'lite-code.tabs.v1'

type TabsStorage = Pick<Storage, 'getItem' | 'setItem'>

interface PersistedTab {
  id: string
  title: string
  path: string | null
  content: string
  savedContent: string
  tier: LargeFileTier
}

interface PersistedTabs {
  activeId: string | null
  tabs: PersistedTab[]
}

const TIERS: readonly LargeFileTier[] = ['full', 'reduced', 'readonly']

function isTier(value: unknown): value is LargeFileTier {
  return typeof value === 'string' && (TIERS as readonly string[]).includes(value)
}

function defaultStorage(): TabsStorage | null {
  if (typeof localStorage === 'undefined') return null
  return localStorage
}

export function serializeTabs(state: TabsState): string {
  const payload: PersistedTabs = {
    activeId: state.activeId,
    tabs: state.tabs.map(tab => ({
      id: tab.id,
      title: tab.title,
      path: tab.path,
      content: tab.content,
      savedContent: tab.savedContent,
      tier: tab.strategy.tier,
    })),
  }
  return JSON.stringify(payload)
}

/** 不合法数据整包丢弃（回到默认样例），避免半损坏状态进入 store。 */
export function parseTabs(raw: string | null): TabsState | null {
  if (!raw) return null
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return null
  }
  if (typeof parsed !== 'object' || parsed === null) return null
  const candidate = parsed as { activeId?: unknown; tabs?: unknown }
  if (!Array.isArray(candidate.tabs)) return null

  const tabs: Tab[] = []
  for (const item of candidate.tabs) {
    if (typeof item !== 'object' || item === null) continue
    const t = item as Record<string, unknown>
    if (typeof t.id !== 'string' || t.id.length === 0) continue
    if (typeof t.title !== 'string' || t.title.length === 0) continue
    if (typeof t.content !== 'string') continue
    const savedContent = typeof t.savedContent === 'string' ? t.savedContent : t.content
    const path = typeof t.path === 'string' ? t.path : null
    const tier: LargeFileTier = isTier(t.tier) ? t.tier : 'full'
    tabs.push({
      id: t.id,
      title: t.title,
      path,
      content: t.content,
      savedContent,
      strategy: strategyForTier(tier),
      dirty: t.content !== savedContent,
    })
  }

  if (tabs.length === 0) return { tabs: [], activeId: null }
  const activeId =
    typeof candidate.activeId === 'string' &&
    tabs.some(tab => tab.id === candidate.activeId)
      ? candidate.activeId
      : tabs[0].id
  return { tabs, activeId }
}

export function loadTabs(storage: TabsStorage | null = defaultStorage()): TabsState | null {
  if (!storage) return null
  return parseTabs(storage.getItem(TABS_STORAGE_KEY))
}

/** 配额/隐私模式失败时静默保留内存状态；下次变更会重试。 */
export function saveTabs(state: TabsState, storage: TabsStorage | null = defaultStorage()): void {
  if (!storage) return
  try {
    storage.setItem(TABS_STORAGE_KEY, serializeTabs(state))
  } catch {
    // localStorage 配额不足：P1 过渡方案限制，P3 迁 Rust 后解决。
  }
}

/** 启动时恢复并订阅后续变更；返回取消订阅函数。 */
export function installTabsPersistence(
  storage: TabsStorage | null = defaultStorage(),
): () => void {
  const restored = loadTabs(storage)
  if (restored) tabsStore.set(restored)
  return tabsStore.subscribe(state => saveTabs(state, storage))
}
