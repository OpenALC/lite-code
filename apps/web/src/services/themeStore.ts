// UI 偏好：主题、缩放、语言。纯 TS + localStorage，视图层订阅。
import type { Locale } from './i18n'

export type ThemeMode = 'dark' | 'light'

export interface UiPrefs {
  theme: ThemeMode
  zoom: number
  locale: Locale
}

const STORAGE_KEY = 'lc.ui.prefs'
export const DEFAULT_PREFS: UiPrefs = { theme: 'dark', zoom: 1, locale: 'zh-CN' }

export function normalizePrefs(input: Partial<UiPrefs>): UiPrefs {
  const theme = input.theme === 'light' ? 'light' : 'dark'
  const zoom = Number.isFinite(input.zoom)
    ? Math.min(1.6, Math.max(0.7, Math.round((input.zoom as number) * 10) / 10))
    : 1
  const locale = input.locale === 'en' ? 'en' : 'zh-CN'
  return { theme, zoom, locale }
}

export function loadPrefs(): UiPrefs {
  if (typeof localStorage === 'undefined') return DEFAULT_PREFS
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    return raw ? normalizePrefs(JSON.parse(raw) as Partial<UiPrefs>) : DEFAULT_PREFS
  } catch {
    return DEFAULT_PREFS
  }
}

export function savePrefs(prefs: UiPrefs): void {
  if (typeof localStorage === 'undefined') return
  localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs))
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

export function initUiPrefs(): void {
  applyTheme(current.theme)
  applyZoom(current.zoom)
}
