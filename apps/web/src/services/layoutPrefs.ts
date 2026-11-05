// 布局偏好持久化。P1 用 localStorage；P4 工作台接入 Rust 侧 workspace state
// 后替换为契约命令（docs/UI.md §1：布局保存到工作区状态）。
import { DEFAULT_LAYOUT, normalizeLayout, type LayoutPrefs } from './layoutProfile'

export const LAYOUT_STORAGE_KEY = 'lite-code.layout.v1'

type PrefsStorage = Pick<Storage, 'getItem' | 'setItem'>

function defaultStorage(): PrefsStorage | null {
  if (typeof localStorage === 'undefined') return null
  return localStorage
}

/** 解析失败/缺字段/越界值都回落到合法默认值，不抛错。 */
export function parseLayoutPrefs(raw: string | null): LayoutPrefs {
  if (!raw) return DEFAULT_LAYOUT
  try {
    const parsed: unknown = JSON.parse(raw)
    if (typeof parsed !== 'object' || parsed === null) return DEFAULT_LAYOUT
    return normalizeLayout(parsed as Partial<LayoutPrefs>)
  } catch {
    return DEFAULT_LAYOUT
  }
}

export function serializeLayoutPrefs(prefs: Partial<LayoutPrefs>): string {
  return JSON.stringify(normalizeLayout(prefs))
}

export function loadLayoutPrefs(storage: PrefsStorage | null = defaultStorage()): LayoutPrefs {
  if (!storage) return DEFAULT_LAYOUT
  return parseLayoutPrefs(storage.getItem(LAYOUT_STORAGE_KEY))
}

export function saveLayoutPrefs(
  prefs: Partial<LayoutPrefs>,
  storage: PrefsStorage | null = defaultStorage(),
): void {
  if (!storage) return
  storage.setItem(LAYOUT_STORAGE_KEY, serializeLayoutPrefs(prefs))
}
