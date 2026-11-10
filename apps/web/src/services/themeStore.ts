// UI 偏好：主题、缩放、语言与编辑器选项。纯 TS + localStorage，视图层订阅。
import type { Locale } from './i18n'

export type ThemeMode = 'dark' | 'light'
/** 主题偏好值：内建 'dark'/'light'，或插件主题 slug（plugin--<pluginId>--<label-slug>）。 */
export type ThemePref = string
export type WordWrap = 'off' | 'on'
export type TabSize = 2 | 4 | 8
export type AgentSendKey = 'enter' | 'ctrl-enter'
export type AgentDensity = 'comfortable' | 'compact'

export const FONT_SIZE_MIN = 11
export const FONT_SIZE_MAX = 20
export const FONT_SIZE_DEFAULT = 13
export const TAB_SIZES: readonly TabSize[] = [2, 4, 8]
export const TAB_SIZE_DEFAULT: TabSize = 2
export const WORD_WRAP_DEFAULT: WordWrap = 'off'
export const AGENT_FONT_MIN = 12
export const AGENT_FONT_MAX = 18
export const AGENT_FONT_DEFAULT = 14
export const AGENT_SEND_KEY_DEFAULT: AgentSendKey = 'enter'
export const AGENT_DENSITY_DEFAULT: AgentDensity = 'comfortable'

export interface UiPrefs {
  theme: ThemePref
  zoom: number
  locale: Locale
  fontSize: number
  tabSize: TabSize
  wordWrap: WordWrap
  agentSendKey: AgentSendKey
  agentDensity: AgentDensity
  agentFontSize: number
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
  agentSendKey: AGENT_SEND_KEY_DEFAULT,
  agentDensity: AGENT_DENSITY_DEFAULT,
  agentFontSize: AGENT_FONT_DEFAULT,
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

function normalizeAgentSendKey(value: unknown): AgentSendKey {
  return value === 'ctrl-enter' ? 'ctrl-enter' : 'enter'
}

function normalizeAgentDensity(value: unknown): AgentDensity {
  return value === 'compact' ? 'compact' : 'comfortable'
}

function normalizeAgentFontSize(value: unknown): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) return AGENT_FONT_DEFAULT
  return Math.min(AGENT_FONT_MAX, Math.max(AGENT_FONT_MIN, Math.round(value)))
}

/** 主题归一：内建 dark/light 原样接受；插件主题 slug 也保留（回落检查在 pluginThemes 同步时做）。 */
function normalizeTheme(value: unknown): ThemePref {
  if (value === 'light' || value === 'dark') return value
  if (typeof value === 'string' && isPluginThemeSlug(value)) return value
  return 'dark'
}

export function normalizePrefs(input: PrefsInput): UiPrefs {
  const theme = normalizeTheme(input.theme)
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
    agentSendKey: normalizeAgentSendKey(input.agentSendKey),
    agentDensity: normalizeAgentDensity(input.agentDensity),
    agentFontSize: normalizeAgentFontSize(input.agentFontSize),
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

/** 已注入插件主题的基色（清单 uiTheme），供 colorScheme 与 toggle 判断。 */
const pluginThemeModes = new Map<string, ThemeMode>()

/** 主题对应的基色：dark/light 直接用；插件主题取其声明的 uiTheme（未知按 dark）。 */
export function themeBaseMode(theme: ThemePref): ThemeMode {
  if (theme === 'light' || theme === 'dark') return theme
  return pluginThemeModes.get(theme) ?? 'dark'
}

export function registerPluginTheme(slug: string, uiTheme: ThemeMode): void {
  pluginThemeModes.set(slug, uiTheme)
}

export function unregisterPluginTheme(slug: string): void {
  pluginThemeModes.delete(slug)
}

export function applyTheme(theme: ThemePref): void {
  if (typeof document === 'undefined') return
  document.documentElement.dataset.theme = theme
  document.documentElement.style.colorScheme = themeBaseMode(theme)
}

export function applyZoom(zoom: number): void {
  if (typeof document === 'undefined') return
  // CSS zoom 对整个 webview 生效（含 Monaco 布局）；Tauri WebView2/WebKit 均支持。
  document.documentElement.style.zoom = String(zoom)
}

export function setTheme(theme: ThemePref): void {
  current = normalizePrefs({ ...current, theme })
  applyTheme(current.theme)
  savePrefs(current)
  emit()
}

export function toggleTheme(): void {
  setTheme(themeBaseMode(current.theme) === 'dark' ? 'light' : 'dark')
}

// ---- 插件主题（docs/EXTENSIONS.md §1）：slug、样式注入与清理 ----

/** 插件主题 slug 前缀；持久化的 theme 值以它为标记。 */
export const PLUGIN_THEME_PREFIX = 'plugin--'

/**
 * slug 规则：`plugin--<pluginId>--<label-slug>`。label-slug 把 label 转小写、
 * 非字母数字折叠成单个 `-` 并去首尾 `-`，只保留 CSS 属性选择器安全字符；
 * 折叠后为空回落 `theme`（如纯中文 label）。
 */
export function pluginThemeSlug(pluginId: string, label: string): string {
  const labelSlug = label
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
  return `${PLUGIN_THEME_PREFIX}${pluginId}--${labelSlug || 'theme'}`
}

export function isPluginThemeSlug(theme: string): boolean {
  return theme.startsWith(PLUGIN_THEME_PREFIX)
}

/** 从 slug 还原插件 id（label-slug 不含 `--`，按最后一次出现切开）。 */
export function pluginThemeOwner(theme: string): string | null {
  if (!isPluginThemeSlug(theme)) return null
  const rest = theme.slice(PLUGIN_THEME_PREFIX.length)
  const index = rest.lastIndexOf('--')
  return index > 0 ? rest.slice(0, index) : null
}

/**
 * 主题 colors 白名单（防注入）：只接受 `--` 前缀的合法 CSS 变量名
 * （字母/数字/中划线/下划线），值为非空字符串；其余键与值一律忽略。
 */
export function normalizePluginThemeColors(raw: unknown): Record<string, string> {
  if (typeof raw !== 'object' || raw === null) return {}
  const colors: Record<string, string> = {}
  for (const [name, value] of Object.entries(raw)) {
    if (!/^--[a-zA-Z0-9-_]+$/.test(name)) continue
    if (typeof value !== 'string') continue
    const trimmed = value.trim()
    if (trimmed) colors[name] = trimmed
  }
  return colors
}

/** 归一化主题 JSON：形状为 `{ name, colors }`；结构非法返回 null。 */
export function normalizePluginThemeJson(
  raw: unknown,
): { name: string; colors: Record<string, string> } | null {
  if (typeof raw !== 'object' || raw === null) return null
  const record = raw as Record<string, unknown>
  if (typeof record.colors !== 'object' || record.colors === null) return null
  return {
    name: typeof record.name === 'string' ? record.name : '',
    colors: normalizePluginThemeColors(record.colors),
  }
}

/** 生成 `:root[data-theme='<slug>']` 规则；colors 为空返回 null（不注入空规则）。 */
export function pluginThemeCss(slug: string, colors: Record<string, string>): string | null {
  const entries = Object.entries(colors)
  if (entries.length === 0) return null
  const body = entries.map(([name, value]) => `  ${name}: ${value};`).join('\n')
  return `:root[data-theme='${slug}'] {\n${body}\n}`
}

/** 样式标签的 data 属性名；属性值即 slug。 */
export const PLUGIN_THEME_ATTR = 'data-plugin-theme'

interface ThemeStyleTag {
  textContent: string
  remove(): void
}

/** node 环境测试可注入的假 Document；缺省回落到全局 document（无则 no-op）。 */
export interface PluginThemeDoc {
  querySelector(selector: string): unknown
  createElement(tagName: string): {
    setAttribute(name: string, value: string): void
    textContent: string
  }
  head: { appendChild(node: unknown): void }
}

function themeDoc(doc?: PluginThemeDoc): PluginThemeDoc | null {
  if (doc) return doc
  return typeof document === 'undefined' ? null : document
}

function themeStyleSelector(slug: string): string {
  return `style[${PLUGIN_THEME_ATTR}="${slug}"]`
}

/**
 * 注入或更新某 slug 的 `<style data-plugin-theme="...">`（挂在 :root[data-theme]
 * 上）；colors 为空时移除既有标签。返回是否写入/保留了一段有效样式。
 */
export function applyPluginTheme(
  slug: string,
  colors: Record<string, string>,
  doc?: PluginThemeDoc,
): boolean {
  const host = themeDoc(doc)
  if (!host) return false
  const css = pluginThemeCss(slug, colors)
  const tag = host.querySelector(themeStyleSelector(slug)) as ThemeStyleTag | null
  if (!css) {
    tag?.remove()
    return false
  }
  if (tag) {
    if (tag.textContent !== css) tag.textContent = css
    return true
  }
  const style = host.createElement('style')
  style.setAttribute(PLUGIN_THEME_ATTR, slug)
  style.textContent = css
  host.head.appendChild(style)
  return true
}

/** 移除某 slug 的样式标签（插件禁用/删除后的清理）；返回是否真的移除了。 */
export function removePluginTheme(slug: string, doc?: PluginThemeDoc): boolean {
  const host = themeDoc(doc)
  if (!host) return false
  const tag = host.querySelector(themeStyleSelector(slug)) as ThemeStyleTag | null
  if (!tag) return false
  tag.remove()
  return true
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

export function setAgentSendKey(agentSendKey: AgentSendKey): void {
  current = normalizePrefs({ ...current, agentSendKey })
  savePrefs(current)
  emit()
}

export function setAgentDensity(agentDensity: AgentDensity): void {
  current = normalizePrefs({ ...current, agentDensity })
  savePrefs(current)
  emit()
}

export function setAgentFontSize(agentFontSize: number): void {
  current = normalizePrefs({ ...current, agentFontSize })
  savePrefs(current)
  emit()
}

export function initUiPrefs(): void {
  applyTheme(current.theme)
  applyZoom(current.zoom)
}
