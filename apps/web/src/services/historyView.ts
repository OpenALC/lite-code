// Agent 历史的视图筛选与「显示已归档」偏好。纯函数，视图层只负责渲染。
import type { HistorySummary } from './bridge'
import { samePath } from './workspaceStore'

export const SHOW_ARCHIVED_STORAGE_KEY = 'lc.ohos.history.showArchived'

type HistoryStorage = Pick<Storage, 'getItem' | 'setItem'>

function defaultStorage(): HistoryStorage | null {
  if (typeof localStorage === 'undefined') return null
  return localStorage
}

export function historyMatchesQuery(item: HistorySummary, query: string): boolean {
  const term = query.trim().toLocaleLowerCase()
  if (!term) return true
  return (
    item.title.toLocaleLowerCase().includes(term) ||
    item.model.toLocaleLowerCase().includes(term) ||
    item.mode.toLocaleLowerCase().includes(term) ||
    item.workspace.toLocaleLowerCase().includes(term) ||
    item.excerpt.toLocaleLowerCase().includes(term)
  )
}

/** 无 workspace 参数时只列出所属工作区为空的会话，与侧栏原有语义一致。 */
export function historyInWorkspace(item: HistorySummary, workspace: string): boolean {
  if (!workspace) return !item.workspace
  return !item.workspace || samePath(item.workspace, workspace)
}

export interface HistoryScopeOptions {
  query?: string
  workspace?: string
}

/** 按搜索词/工作区切片，并把归档项从默认列表中分离。 */
export function splitHistoryItems(
  items: readonly HistorySummary[],
  options: HistoryScopeOptions = {},
): { active: HistorySummary[]; archived: HistorySummary[] } {
  const active: HistorySummary[] = []
  const archived: HistorySummary[] = []
  for (const item of items) {
    if (!historyInWorkspace(item, options.workspace ?? '')) continue
    if (!historyMatchesQuery(item, options.query ?? '')) continue
    if (item.archived) archived.push(item)
    else active.push(item)
  }
  return { active, archived }
}

export interface ArchiveToggleView {
  label: 'show' | 'hide'
  count: number | null
}

/**
 * 归档开关的文案与计数：0 项时固定显示「显示已归档」且不渲染计数，
 * 有归档时才在显示/隐藏之间切换并保留 N。
 */
export function archiveToggleView(
  showArchived: boolean,
  count: number,
): ArchiveToggleView {
  if (count <= 0) return { label: 'show', count: null }
  return { label: showArchived ? 'hide' : 'show', count }
}

export function loadShowArchived(
  storage: HistoryStorage | null = defaultStorage(),
): boolean {
  if (!storage) return false
  try {
    return storage.getItem(SHOW_ARCHIVED_STORAGE_KEY) === '1'
  } catch {
    return false
  }
}

export function saveShowArchived(
  value: boolean,
  storage: HistoryStorage | null = defaultStorage(),
): void {
  if (!storage) return
  try {
    storage.setItem(SHOW_ARCHIVED_STORAGE_KEY, value ? '1' : '0')
  } catch {
    // 隐私模式/配额失败只影响记忆，不影响当前会话内的开关。
  }
}
