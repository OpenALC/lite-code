// UI 偏好：主题、缩放、语言与编辑器选项。纯 TS + localStorage，视图层订阅。
import type { Locale } from './i18n'

export type ThemeMode = 'dark' | 'light'
export type WordWrap = 'off' | 'on'
export type TabSize = 2 | 4 | 8

export const FONT_SIZE_MIN = 11
export const FONT_SIZE_MAX = 20
export const FONT_SIZE_DEFAULT = 13
export const TAB_SIZES: readonly TabSize[] = [2, 4, 8]
export const TAB_SIZE_DEFAULT: TabSize = 2
export const WORD_WRAP_DEFAULT: WordWrap = 'off'

export interface UiPrefs {
  theme: ThemeMode
  zoom: number
  locale: Locale
  fontSize: number
  tabSize: TabSize
  wordWrap: WordWrap
}

type PrefsStorage = Pick<Storage, 'getItem' | 'setItem'>

export const PREFS_STORAGE_KEY = 'lc.ui.prefs'

export const DEFAULT_PREFS: UiPrefs = {
  theme: 'dark',
  zoom: 1,
  locale: 'zh-CN',
  fontSize: FONT_SIZE_DEFAULT,
  tabSize: TAB_SIZE_DEFAULT,
  wordWrap: WORD_WRAP_DEFAULT,
}

/** 读取自 localStorage 的原始值可能是任意类型，逐项归一化。 */
export type PrefsInput = { [K in keyof UiPrefs]?: unknown }

function normalizeFontSize(value: unknown): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) return FONT_SIZE_DEFAULT
  return Math.min(FONT_SIZE_MAX, Math.max(FONT_SIZE_MIN, Math.round(value)))
}

function normalizeTabSize(value: unknown): TabSize {
  return TAB_SIZES.includes(value as TabSize) ? (value as TabSize) : TAB_SIZE_DEFAULT
}

function normalizeWordWrap(value: unknown): WordWrap {
  return value === 'on' ? 'on' : 'off'
}

export function normalizePrefs(input: PrefsInput): UiPrefs {
  const theme = input.theme === 'light' ? 'light' : 'dark'
  const zoom = Number.isFinite(input.zoom)
    ? Math.min(1.6, Math.max(0.7, Math.round((input.zoom as number) * 10) / 10))
    : 1
  const locale = input.locale === 'en' ? 'en' : 'zh-CN'
  return {
    theme,
    zoom,
    locale,
    fontSize: normalizeFontSize(input.fontSize),
    tabSize: normalizeTabSize(input.tabSize),
    wordWrap: normalizeWordWrap(input.wordWrap),
  }
}

function defaultStorage(): PrefsStorage | null {
  if (typeof localStorage === 'undefined') return null
  return localStorage
}

export function loadPrefs(storage: PrefsStorage | null = defaultStorage()): UiPrefs {
  if (!storage) return { ...DEFAULT_PREFS }
  try {
    const raw = storage.getItem(PREFS_STORAGE_KEY)
    if (!raw) return { ...DEFAULT_PREFS }
    const parsed: unknown = JSON.parse(raw)
    if (typeof parsed !== 'object' || parsed === null) return { ...DEFAULT_PREFS }
    return normalizePrefs(parsed as PrefsInput)
  } catch {
    return { ...DEFAULT_PREFS }
  }
}

export function savePrefs(
  prefs: UiPrefs,
  storage: PrefsStorage | null = defaultStorage(),
): void {
  if (!storage) return
  storage.setItem(PREFS_STORAGE_KEY, JSON.stringify(prefs))
}

type Listener = (prefs: UiPrefs) => void
const listeners = new Set<Listener>()
let current = loadPrefs()

export function uiPrefs(): UiPrefs {
  return current
}

export function subscribePrefs(listener: Listener): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

function emit(): void {
  for (const listener of listeners) listener(current)
}

export function applyTheme(theme: ThemeMode): void {
  if (typeof document === 'undefined') return
  document.documentElement.dataset.theme = theme
  document.documentElement.style.colorScheme = theme
}

export function applyZoom(zoom: number): void {
  if (typeof document === 'undefined') return
  // CSS zoom 对整个 webview 生效（含 Monaco 布局）；Tauri WebView2/WebKit 均支持。
  document.documentElement.style.zoom = String(zoom)
}

export function setTheme(theme: ThemeMode): void {
  current = normalizePrefs({ ...current, theme })
  applyTheme(current.theme)
  savePrefs(current)
  emit()
}

export function toggleTheme(): void {
  setTheme(current.theme === 'dark' ? 'light' : 'dark')
}

export function setZoom(zoom: number): void {
  current = normalizePrefs({ ...current, zoom })
  applyZoom(current.zoom)
  savePrefs(current)
  emit()
}

export function zoomBy(delta: number): void {
  setZoom(current.zoom + delta)
}

export function resetZoom(): void {
  setZoom(1)
}

export function setLocale(locale: Locale): void {
  current = normalizePrefs({ ...current, locale })
  savePrefs(current)
  emit()
}

export function setFontSize(fontSize: number): void {
  current = normalizePrefs({ ...current, fontSize })
  savePrefs(current)
  emit()
}

export function setTabSize(tabSize: TabSize): void {
  current = normalizePrefs({ ...current, tabSize })
  savePrefs(current)
  emit()
}

export function setWordWrap(wordWrap: WordWrap): void {
  current = normalizePrefs({ ...current, wordWrap })
  savePrefs(current)
  emit()
}

export function initUiPrefs(): void {
  applyTheme(current.theme)
  applyZoom(current.zoom)
}
