// 插件 UI 视图（docs/plans/plugin-webview.md §3.1/§3.4）：编辑器区标签页的
// 声明式节渲染（kind 闭集 markdown|form|list|kv）。分层镜像 pluginViews.ts：
// 本服务管声明收集/白名单归一/标签生命周期/节数据状态/命令别名，
// PluginUiView.vue 纯渲染。M2 交互回传：list/kv 节经 ui/values 动态回读
// （节错误态可重试），表单提交/行点击走既有命令回路（§3.5）。打开视图本身
// 零宿主往返（§3.5）。
import {
  extensionsExecuteCommand,
  extensionsList,
  extensionsUiValues,
  type PluginList,
} from './extensions'
import { closeTab, openTab, tabsStore } from './tabs'

/** 节 kind 闭集（§3.1）；未知 kind 在 Rust 校验期拒绝，web 守卫再兜一层。 */
export type UiSectionKind = 'markdown' | 'form' | 'list' | 'kv'

/** 字段 type 闭集（§3.1）。 */
export type UiFieldType = 'text' | 'number' | 'boolean' | 'select' | 'password'

/** list 单元格允许的标量（§3.2）；form default 亦限标量。 */
export type UiScalar = string | number | boolean

export interface UiField {
  key: string
  label: string
  type: UiFieldType
  placeholder?: string
  default?: UiScalar
  options?: string[]
  min?: number
  max?: number
}

export interface UiColumn {
  key: string
  label: string
  /** 1..5 权重（Rust 校验；缺省 1）。 */
  width: number
}

export interface UiKvItem {
  key: string
  value: string
}

export type UiSection =
  | { kind: 'markdown'; text: string }
  | { kind: 'form'; title?: string; command?: string; fields: UiField[] }
  | {
      kind: 'list'
      title?: string
      valuesCommand?: string
      columns: UiColumn[]
      rowCommand?: string
    }
  | { kind: 'kv'; title?: string; items?: UiKvItem[]; valuesCommand?: string }

export interface UiView {
  id: string
  name: string
  openCommand: string | null
  sections: UiSection[]
}

// ==== 守卫与归一（纯函数；不信任 IPC 对端，同 extensions.ts 守卫风格） ====

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function isScalar(value: unknown): value is UiScalar {
  return (
    typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean'
  )
}

function isUiFieldType(value: unknown): value is UiFieldType {
  return (
    value === 'text' ||
    value === 'number' ||
    value === 'boolean' ||
    value === 'select' ||
    value === 'password'
  )
}

/** 单字段白名单归一：key/label 非空字符串、type 闭集；default 限标量；
 * options 非空字符串数组；min/max 限数字；其余键一律丢弃。形状非法 → null。 */
function normalizeUiField(raw: unknown): UiField | null {
  if (!isRecord(raw)) return null
  if (typeof raw.key !== 'string' || raw.key.length === 0) return null
  if (typeof raw.label !== 'string' || raw.label.length === 0) return null
  if (!isUiFieldType(raw.type)) return null
  const field: UiField = { key: raw.key, label: raw.label, type: raw.type }
  if (typeof raw.placeholder === 'string' && raw.placeholder.length > 0) {
    field.placeholder = raw.placeholder
  }
  if (raw.default !== undefined) {
    if (!isScalar(raw.default)) return null
    field.default = raw.default
  }
  if (raw.type === 'select') {
    if (!Array.isArray(raw.options) || raw.options.length === 0) return null
    if (!raw.options.every(option => typeof option === 'string' && option.length > 0)) {
      return null
    }
    field.options = [...raw.options] as string[]
  }
  for (const bound of ['min', 'max'] as const) {
    if (raw[bound] !== undefined) {
      if (typeof raw[bound] !== 'number') return null
      field[bound] = raw[bound] as number
    }
  }
  return field
}

/** 单列白名单归一：key/label 非空字符串，width 缺省 1、限正数。 */
function normalizeUiColumn(raw: unknown): UiColumn | null {
  if (!isRecord(raw)) return null
  if (typeof raw.key !== 'string' || raw.key.length === 0) return null
  if (typeof raw.label !== 'string' || raw.label.length === 0) return null
  let width = 1
  if (raw.width !== undefined) {
    if (typeof raw.width !== 'number' || !(raw.width > 0)) return null
    width = raw.width
  }
  return { key: raw.key, label: raw.label, width }
}

/** kv 条目归一：key 非空字符串、value 字符串（javascript: 等载荷按纯文本
 * 保留——渲染层不产出任何 URL/图片，零链接面）。 */
function normalizeUiKvItem(raw: unknown): UiKvItem | null {
  if (!isRecord(raw)) return null
  if (typeof raw.key !== 'string' || raw.key.length === 0) return null
  if (typeof raw.value !== 'string') return null
  return { key: raw.key, value: raw.value }
}

function normalizeOptionalString(raw: unknown): string | undefined {
  return typeof raw === 'string' && raw.length > 0 ? raw : undefined
}

/** 节白名单归一（§3.4）：按 kind 分派、多余键一律丢弃；形状非法 → null
 * （调用方渲染节错误态）。 */
export function normalizeUiSection(raw: unknown): UiSection | null {
  if (!isRecord(raw)) return null
  switch (raw.kind) {
    case 'markdown': {
      if (typeof raw.text !== 'string') return null
      return { kind: 'markdown', text: raw.text }
    }
    case 'form': {
      if (!Array.isArray(raw.fields) || raw.fields.length === 0) return null
      const fields: UiField[] = []
      for (const item of raw.fields) {
        const field = normalizeUiField(item)
        if (field === null) return null
        fields.push(field)
      }
      const section: UiSection = { kind: 'form', fields }
      const title = normalizeOptionalString(raw.title)
      if (title !== undefined) section.title = title
      const command = normalizeOptionalString(raw.command)
      if (command !== undefined) section.command = command
      return section
    }
    case 'list': {
      if (!Array.isArray(raw.columns) || raw.columns.length === 0) return null
      const columns: UiColumn[] = []
      for (const item of raw.columns) {
        const column = normalizeUiColumn(item)
        if (column === null) return null
        columns.push(column)
      }
      const section: UiSection = { kind: 'list', columns }
      const title = normalizeOptionalString(raw.title)
      if (title !== undefined) section.title = title
      const valuesCommand = normalizeOptionalString(raw.valuesCommand)
      if (valuesCommand !== undefined) section.valuesCommand = valuesCommand
      const rowCommand = normalizeOptionalString(raw.rowCommand)
      if (rowCommand !== undefined) section.rowCommand = rowCommand
      return section
    }
    case 'kv': {
      const section: UiSection = { kind: 'kv' }
      const title = normalizeOptionalString(raw.title)
      if (title !== undefined) section.title = title
      if (raw.items !== undefined) {
        if (!Array.isArray(raw.items)) return null
        const items: UiKvItem[] = []
        for (const item of raw.items) {
          const kv = normalizeUiKvItem(item)
          if (kv === null) return null
          items.push(kv)
        }
        section.items = items
      }
      const valuesCommand = normalizeOptionalString(raw.valuesCommand)
      if (valuesCommand !== undefined) section.valuesCommand = valuesCommand
      return section
    }
    default:
      return null
  }
}

/** 节形状守卫。 */
export function isUiSection(raw: unknown): raw is UiSection {
  return normalizeUiSection(raw) !== null
}

/** 视图白名单归一：id/name 字符串（name 允许空——Rust 校验期已要求非空，
 * 守卫宽容只兜形状）、openCommand 可空字符串、sections 数组逐节归一。
 * 任一节非法 → 整个视图 null（DTO 层 fail-closed，节级容错见
 * buildUiViewEntry）。 */
export function normalizeUiView(raw: unknown): UiView | null {
  if (!isRecord(raw)) return null
  if (typeof raw.id !== 'string' || raw.id.length === 0) return null
  if (typeof raw.name !== 'string') return null
  if (raw.openCommand !== null && raw.openCommand !== undefined) {
    if (typeof raw.openCommand !== 'string') return null
  }
  if (!Array.isArray(raw.sections)) return null
  const sections: UiSection[] = []
  for (const item of raw.sections) {
    const section = normalizeUiSection(item)
    if (section === null) return null
    sections.push(section)
  }
  return {
    id: raw.id,
    name: raw.name,
    openCommand: typeof raw.openCommand === 'string' ? raw.openCommand : null,
    sections,
  }
}

/** 视图形状守卫（extensions.ts 的 isPluginInfo 复用）。 */
export function isUiView(raw: unknown): raw is UiView {
  return normalizeUiView(raw) !== null
}

// ==== 数据回读归一（§3.2 骨架；M1 无回读，M2 由 ui/values 驱动） ====

export interface UiValues {
  /** form 节：按 fieldKey 的动态初始值。 */
  values?: Record<string, unknown>
  /** list 节：行（单元格限标量，非标量键丢弃）。 */
  rows?: Record<string, UiScalar>[]
  /** kv 节：键值对。 */
  items?: UiKvItem[]
}

/** 宿主 ui/values 结果按节 kind 白名单归一（照抄 normalizeViewNodes 风格）：
 * 多余键丢弃、单元格/条目非白名单值丢弃或判非法；markdown 节无回读恒 null。
 * 宿主侧已归一一次（§3.2），这里不信任 IPC 对端再兜一层（§3.4）。 */
export function normalizeUiValues(kind: UiSectionKind, raw: unknown): UiValues | null {
  switch (kind) {
    case 'markdown':
      return null
    case 'form': {
      if (!isRecord(raw) || !isRecord(raw.values)) return null
      const values: Record<string, unknown> = {}
      for (const [key, value] of Object.entries(raw.values)) values[key] = value
      return { values }
    }
    case 'list': {
      if (!isRecord(raw) || !Array.isArray(raw.rows) || raw.rows.length > 500) return null
      const rows: Record<string, UiScalar>[] = []
      for (const row of raw.rows) {
        if (!isRecord(row)) return null
        const cells: Record<string, UiScalar> = {}
        for (const [key, value] of Object.entries(row)) {
          if (isScalar(value)) cells[key] = value
        }
        rows.push(cells)
      }
      return { rows }
    }
    case 'kv': {
      if (!isRecord(raw) || !Array.isArray(raw.items) || raw.items.length > 64) return null
      const items: UiKvItem[] = []
      for (const item of raw.items) {
        const kv = normalizeUiKvItem(item)
        if (kv === null) return null
        items.push(kv)
      }
      return { items }
    }
  }
}

// ==== 标签容器（§3.4：照 __extensions__ 先例，id `__ui__:<pluginId>:<viewId>`） ====

export const UI_TAB_PREFIX = '__ui__:'

export function uiTabId(pluginId: string, viewId: string): string {
  return `${UI_TAB_PREFIX}${pluginId}:${viewId}`
}

/** 标签 id → 视图引用。viewId 经 Rust 校验（小写字母/数字/线）不含 ':'，
 * 故从最后一个 ':' 切分，pluginId 侧可含任意合法 id 字符。 */
export function parseUiTabId(
  tabId: string,
): { pluginId: string; viewId: string } | null {
  if (!tabId.startsWith(UI_TAB_PREFIX)) return null
  const rest = tabId.slice(UI_TAB_PREFIX.length)
  const sep = rest.lastIndexOf(':')
  if (sep <= 0 || sep === rest.length - 1) return null
  return { pluginId: rest.slice(0, sep), viewId: rest.slice(sep + 1) }
}

// ==== 状态 ====

/** 一个已声明视图的渲染条目；sections 中 null = 节形状非法（节错误态）。 */
export interface UiViewEntry {
  /** 状态键：`${pluginId}:${viewId}`。 */
  key: string
  pluginId: string
  pluginName: string
  viewId: string
  viewName: string
  openCommand: string | null
  sections: Array<UiSection | null>
}

export interface PluginUiState {
  views: UiViewEntry[]
  /** M2 节数据状态：键 `${pluginId}:${viewId}:${sectionIndex}`（list/kv 回读、
   * form 提交注记共用；markdown 与静态节无条目）。 */
  sectionData: Record<string, UiSectionDataState>
}

type Listener = (state: PluginUiState) => void

const listeners = new Set<Listener>()
let state: PluginUiState = { views: [], sectionData: {} }

export function getPluginUiState(): PluginUiState {
  return state
}

export function subscribePluginUi(listener: Listener): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

function emit(): void {
  for (const listener of listeners) listener(state)
}

/** 测试用：清空声明状态。 */
export function resetPluginUi(): void {
  state = { views: [], sectionData: {} }
  emit()
}

/** 从插件列表收集启用且无 error 插件的 UI 视图（error 插件没有可用清单）。 */
export function declaredUiViews(list: PluginList): UiViewEntry[] {
  const views: UiViewEntry[] = []
  for (const plugin of list.plugins) {
    if (!plugin.enabled || plugin.error) continue
    for (const view of plugin.uiViews) {
      const entry = buildUiViewEntry(plugin.id, plugin.name, view)
      if (entry !== null) views.push(entry)
    }
  }
  return views
}

/** DTO → 渲染条目：顶层形状非法（id/sections 坏）丢弃整个视图；单节非法
 * 置 null，渲染层显示节错误态（§3.4）。 */
function buildUiViewEntry(
  pluginId: string,
  pluginName: string,
  raw: unknown,
): UiViewEntry | null {
  if (!isRecord(raw)) return null
  if (typeof raw.id !== 'string' || raw.id.length === 0) return null
  if (typeof raw.name !== 'string') return null
  if (raw.openCommand !== null && typeof raw.openCommand !== 'string') return null
  if (!Array.isArray(raw.sections)) return null
  const sections = raw.sections.map(item => normalizeUiSection(item))
  return {
    key: `${pluginId}:${raw.id as string}`,
    pluginId,
    pluginName,
    viewId: raw.id,
    viewName: raw.name,
    openCommand: raw.openCommand,
    sections,
  }
}

export function findUiView(pluginId: string, viewId: string): UiViewEntry | undefined {
  return state.views.find(view => view.pluginId === pluginId && view.viewId === viewId)
}

// ==== 同步（App 启动与插件增删后调用，照 syncPluginViews 挂点） ====

/**
 * 拉取最新插件列表并做 UI 视图同步；失败返回 null（保持原状）。
 * 插件禁用/删除 → 其视图标签关闭、不进「重新打开」栈（§3.6）。
 */
export async function syncPluginUiViews(): Promise<PluginList | null> {
  const res = await extensionsList()
  if (!res.ok) return null
  applyPluginUiList(res.data)
  return res.data
}

/** 对一份插件列表做同步（测试与复用入口）。 */
export function applyPluginUiList(list: PluginList): void {
  const views = declaredUiViews(list)
  const alive = new Set(views.map(view => view.key))
  state.views = views
  emit()
  for (const tab of [...tabsStore.get().tabs]) {
    const ref = parseUiTabId(tab.id)
    if (ref && !alive.has(`${ref.pluginId}:${ref.viewId}`)) {
      closeTab(tab.id, { force: true, recordClosed: false })
    }
  }
}

// ==== 打开视图与 openCommand 别名（§3.4 入口；打开本身零宿主往返） ====

/** 打开（或激活）一个已声明视图的编辑器区标签；视图未声明返回 false。 */
export function openPluginUiView(pluginId: string, viewId: string): boolean {
  const entry = findUiView(pluginId, viewId)
  if (!entry) return false
  openTab({
    id: uiTabId(pluginId, viewId),
    title: entry.viewName,
    path: null,
    content: '',
  })
  return true
}

/** 按 openCommand 别名查视图。 */
export function resolveUiOpenCommand(
  command: string,
): { pluginId: string; viewId: string } | null {
  const needle = command.trim()
  if (needle.length === 0) return null
  for (const view of state.views) {
    if (view.openCommand === needle) return { pluginId: view.pluginId, viewId: view.viewId }
  }
  return null
}

/**
 * palette 动作分流：命中 openCommand 别名 → 打开 UI 视图标签（纯 web 拦截，
 * 宿主 handler 不执行，零 invoke）；未命中返回 false，由调用方走
 * extensionsExecuteCommand。
 */
export function openPluginUiViewByCommand(command: string): boolean {
  const alias = resolveUiOpenCommand(command)
  if (!alias) return false
  return openPluginUiView(alias.pluginId, alias.viewId)
}

// ==== 节数据回读与交互（§3.2/§3.5，M2） ====

export type UiSectionStatus = 'idle' | 'loading' | 'ready' | 'error'

/** 单节运行时状态：list/kv 数据回读 + form/list 交互注记共用。 */
export interface UiSectionDataState {
  status: UiSectionStatus
  /** list→rows、kv→items（回读成功时；form 不读回，恒 null）。 */
  values: UiValues | null
  /** 回读失败的展示信息（code 进 tooltip）。 */
  error: { code: string; message: string } | null
  /** 提交/行点击注记：success=成功；error=失败并保留用户输入。 */
  note: { kind: 'success' | 'error'; message: string } | null
  /** 提交/行点击进行中。 */
  busy: boolean
}

/** 节状态键：`${pluginId}:${viewId}:${sectionIndex}`（节序号即 ui/values 的
 * sectionId，与宿主侧节序号一致）。 */
export function uiSectionDataKey(
  pluginId: string,
  viewId: string,
  sectionIndex: number,
): string {
  return `${pluginId}:${viewId}:${sectionIndex}`
}

function emptySectionData(): UiSectionDataState {
  return { status: 'idle', values: null, error: null, note: null, busy: false }
}

function sectionDataOf(key: string): UiSectionDataState {
  return state.sectionData[key] ?? emptySectionData()
}

function patchSectionData(key: string, patch: Partial<UiSectionDataState>): void {
  state.sectionData = { ...state.sectionData, [key]: { ...sectionDataOf(key), ...patch } }
  emit()
}

/** 表单提交值组装（§3.5，纯函数测试入口）：按 field.type 强制转换——number
 * 空输入回落数字 default（无则省略该键）、非有限数省略；boolean 仅 true；
 * text/password/select 归一为字符串。password 值不进日志（调用方不得打印
 * 本返回对象）。 */
export function coerceUiFormValues(
  section: Extract<UiSection, { kind: 'form' }>,
  raw: Record<string, unknown>,
): Record<string, unknown> {
  const values: Record<string, unknown> = {}
  for (const field of section.fields) {
    const value = raw[field.key]
    switch (field.type) {
      case 'number': {
        if (typeof value === 'number' && Number.isFinite(value)) {
          values[field.key] = value
          break
        }
        const text = typeof value === 'string' ? value.trim() : ''
        if (text === '') {
          if (typeof field.default === 'number') values[field.key] = field.default
          break
        }
        const num = Number(text)
        if (Number.isFinite(num)) values[field.key] = num
        break
      }
      case 'boolean':
        values[field.key] = value === true
        break
      default:
        values[field.key] = typeof value === 'string' ? value : String(value ?? '')
        break
    }
  }
  return values
}

type ReadableUiSection =
  | Extract<UiSection, { kind: 'list' }>
  | Extract<UiSection, { kind: 'kv' }>

/** 可回读节：list/kv 且声明 valuesCommand。form 动态初始值协议面已支持
 * （§3.2），web M2 仍用静态 default；markdown 无回读。 */
function readableSection(
  entry: UiViewEntry | undefined,
  sectionIndex: number,
): ReadableUiSection | null {
  const section = entry?.sections[sectionIndex]
  if (!section || (section.kind !== 'list' && section.kind !== 'kv')) return null
  if (!section.valuesCommand) return null
  return section
}

/** 拉取节数据（ui/values，§3.2）：失败/形状非法 → 节错误态（可重试，
 * 照树视图错误态）。 */
export async function loadUiSectionValues(
  pluginId: string,
  viewId: string,
  sectionIndex: number,
): Promise<void> {
  const entry = findUiView(pluginId, viewId)
  const section = readableSection(entry, sectionIndex)
  if (!section) return
  const key = uiSectionDataKey(pluginId, viewId, sectionIndex)
  if (state.sectionData[key]?.status === 'loading') return
  patchSectionData(key, { status: 'loading', error: null })
  const res = await extensionsUiValues(pluginId, viewId, String(sectionIndex))
  const current = state.sectionData[key]
  if (!current || current.status !== 'loading') return // 加载期间被重置
  if (!res.ok) {
    patchSectionData(key, {
      status: 'error',
      error: { code: res.error.code, message: res.error.message },
    })
    return
  }
  const values = normalizeUiValues(section.kind, res.data.values)
  if (values === null) {
    patchSectionData(key, {
      status: 'error',
      error: { code: 'internal', message: '节数据形状非法' },
    })
    return
  }
  patchSectionData(key, { status: 'ready', values, error: null })
}

/** 挂载入口：无条目或 idle 才拉（watchEffect 可能重复触发）。 */
export async function ensureUiSectionLoaded(
  pluginId: string,
  viewId: string,
  sectionIndex: number,
): Promise<void> {
  const existing = state.sectionData[uiSectionDataKey(pluginId, viewId, sectionIndex)]
  if (!existing || existing.status === 'idle') {
    await loadUiSectionValues(pluginId, viewId, sectionIndex)
  }
}

/** 错误态重试（节错误态的「重试」按钮）。 */
export async function retryUiSection(
  pluginId: string,
  viewId: string,
  sectionIndex: number,
): Promise<void> {
  await loadUiSectionValues(pluginId, viewId, sectionIndex)
}

/** 表单提交（§3.5）：ext_execute_command(section.command, { values })；ok →
 * 成功注记；error → 错误注记（用户输入由组件持有的 draft 保留）。 */
export async function submitUiForm(
  pluginId: string,
  viewId: string,
  sectionIndex: number,
  draft: Record<string, unknown>,
): Promise<boolean> {
  const section = findUiView(pluginId, viewId)?.sections[sectionIndex]
  if (!section || section.kind !== 'form' || !section.command) return false
  const key = uiSectionDataKey(pluginId, viewId, sectionIndex)
  const values = coerceUiFormValues(section, draft)
  patchSectionData(key, { busy: true, note: null })
  const res = await extensionsExecuteCommand(section.command, { values })
  const current = state.sectionData[key]
  if (!current?.busy) return false // 提交期间被重置
  if (!res.ok) {
    patchSectionData(key, {
      busy: false,
      note: { kind: 'error', message: res.error.message },
    })
    return false
  }
  patchSectionData(key, { busy: false, note: { kind: 'success', message: '' } })
  return true
}

/** list 行点击（§3.5）：rowCommand(row)（整行对象作 args，单元格为归一
 * 标量）；成功后回读刷新 + 成功注记，失败 → 错误注记。 */
export async function runUiRowCommand(
  pluginId: string,
  viewId: string,
  sectionIndex: number,
  row: Record<string, UiScalar>,
): Promise<boolean> {
  const section = findUiView(pluginId, viewId)?.sections[sectionIndex]
  if (!section || section.kind !== 'list' || !section.rowCommand) return false
  const key = uiSectionDataKey(pluginId, viewId, sectionIndex)
  patchSectionData(key, { busy: true, note: null })
  const res = await extensionsExecuteCommand(section.rowCommand, { ...row })
  const current = state.sectionData[key]
  if (!current?.busy) return false // 执行期间被重置
  if (!res.ok) {
    patchSectionData(key, {
      busy: false,
      note: { kind: 'error', message: res.error.message },
    })
    return false
  }
  await loadUiSectionValues(pluginId, viewId, sectionIndex)
  patchSectionData(uiSectionDataKey(pluginId, viewId, sectionIndex), {
    busy: false,
    note: { kind: 'success', message: '' },
  })
  return true
}
