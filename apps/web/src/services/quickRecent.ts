// 快速打开（Ctrl+P）的最近使用记录：文件按标签 id、命令按命令面板 id。
export interface QuickRecent {
  files: string[]
  commands: string[]
}

export const EMPTY_QUICK_RECENT: QuickRecent = { files: [], commands: [] }
export const QUICK_RECENT_LIMIT = 10
const STORAGE_KEY = 'lc.quick.recent'

function asIdList(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  return [...new Set(value.filter((item): item is string => typeof item === 'string' && item !== ''))]
    .slice(0, QUICK_RECENT_LIMIT)
}

export function normalizeQuickRecent(value: unknown): QuickRecent {
  if (!value || typeof value !== 'object') return { ...EMPTY_QUICK_RECENT }
  const item = value as Record<string, unknown>
  return {
    files: asIdList(item.files),
    commands: asIdList(item.commands),
  }
}

export function rememberRecent(
  state: QuickRecent,
  kind: keyof QuickRecent,
  id: string,
  limit = QUICK_RECENT_LIMIT,
): QuickRecent {
  if (!id) return state
  const next = [id, ...state[kind].filter(item => item !== id)].slice(0, limit)
  return { ...state, [kind]: next }
}

/** 已知最近项按记录顺序排在前面；未知项保持原有相对顺序。 */
export function sortByRecent<T extends { id: string }>(
  items: readonly T[],
  ids: readonly string[],
): T[] {
  const rank = new Map(ids.map((id, index) => [id, index]))
  return items
    .map((item, index) => ({ item, index, rank: rank.get(item.id) ?? Number.MAX_SAFE_INTEGER }))
    .sort((a, b) => a.rank - b.rank || a.index - b.index)
    .map(entry => entry.item)
}

export function loadQuickRecent(): QuickRecent {
  try {
    return normalizeQuickRecent(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? 'null'))
  } catch {
    return { ...EMPTY_QUICK_RECENT }
  }
}

export function saveQuickRecent(state: QuickRecent): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state))
  } catch {
    // 浏览器演示模式的配额失败不影响主流程。
  }
}
