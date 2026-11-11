// 插件 UI 视图（docs/plans/plugin-webview.md §3.1/§3.4/§3.6 M2-R）：编辑器区
// 标签页的声明式节渲染（kind 闭集 markdown|form|list|kv）。分层镜像
// pluginViews.ts：本服务管声明收集/白名单归一/标签生命周期/节数据状态/
// 命令别名，PluginUiView.vue 纯渲染。M2-R 生命周期：节身份=稳定 sectionId
// （不用显示序号）、initialize 下发完整节描述 DTO（source:"provider" 为唯一
// 回读开关，valuesCommand 已移除）、同步代次守卫在途请求（旧结果丢弃、作废旧
// 请求时复位 loading/busy）、同 id 节语义声明变化重置该节。list/kv 节经
// ui/values 动态回读（节错误态可重试），表单提交/行点击走既有命令回路（§3.5）。
// 打开视图本身零宿主往返（§3.5）。
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

/** 清单 initialize 下发的节字段描述 DTO（§3.2；label/placeholder 不在 DTO 内，
 * 渲染文案仍从清单 PluginInfo.uiViews 取）。 */
export interface UiFieldSpec {
  key: string
  type: UiFieldType
  default?: UiScalar
  options?: string[]
  min?: number
  max?: number
}

/** 清单 initialize 下发的节列描述 DTO（§3.2；label/width 不在 DTO 内）。 */
export interface UiColumnSpec {
  key: string
}

/** 归一后的节字段：清单完整声明 ⊕ initialize DTO（label/placeholder 是渲染
 * 文案、可缺省回退 key；语义约束字段来自 DTO；多余键一律丢弃）。 */
export interface UiField extends UiFieldSpec {
  label?: string
  placeholder?: string
}

export interface UiColumn extends UiColumnSpec {
  label?: string
  /** 1..5 权重（Rust 校验；缺省 1）。 */
  width: number
}

export interface UiKvItem {
  key: string
  value: string
}

export type UiSection =
  | { id: string; kind: 'markdown'; text: string }
  | {
      id: string
      kind: 'form'
      title?: string
      command?: string
      fields: UiField[]
    }
  | {
      id: string
      kind: 'list'
      title?: string
      /** 唯一回读开关（§3.2）：source === 'provider' 才回读。 */
      source?: 'provider'
      columns: UiColumn[]
      rowCommand?: string
    }
  | { id: string; kind: 'kv'; title?: string; items?: UiKvItem[]; source?: 'provider' }

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

/** 单字段白名单归一：key 非空字符串、type 闭集；default 限标量；
 * options 非空字符串数组；min/max 限数字；其余键一律丢弃（含 valuesCommand
 * 等旧协议字段——M2-R 起不存在，视为无回读）。形状非法 → null。 */
function normalizeUiField(raw: unknown): UiField | null {
  if (!isRecord(raw)) return null
  if (typeof raw.key !== 'string' || raw.key.length === 0) return null
  if (!isUiFieldType(raw.type)) return null
  const field: UiField = { key: raw.key, type: raw.type }
  if (typeof raw.label === 'string' && raw.label.length > 0) field.label = raw.label
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

/** 单列白名单归一：key 非空字符串，label 可缺省，width 缺省 1、限正数。 */
function normalizeUiColumn(raw: unknown): UiColumn | null {
  if (!isRecord(raw)) return null
  if (typeof raw.key !== 'string' || raw.key.length === 0) return null
  let width = 1
  if (raw.width !== undefined) {
    if (typeof raw.width !== 'number' || !(raw.width > 0)) return null
    width = raw.width
  }
  const column: UiColumn = { key: raw.key, width }
  if (typeof raw.label === 'string' && raw.label.length > 0) column.label = raw.label
  return column
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

/** 节白名单归一（§3.2 DTO + §3.4）：稳定 id 必填（initialize DTO 下发），按
 * kind 分派、多余键一律丢弃（valuesCommand 字段存在也丢弃——视为无回读）；
 * 形状非法 → null（调用方渲染节错误态）。 */
export function normalizeUiSection(raw: unknown): UiSection | null {
  if (!isRecord(raw)) return null
  if (typeof raw.id !== 'string' || raw.id.length === 0) return null
  const id = raw.id
  switch (raw.kind) {
    case 'markdown': {
      if (typeof raw.text !== 'string') return null
      return { id, kind: 'markdown', text: raw.text }
    }
    case 'form': {
      if (!Array.isArray(raw.fields) || raw.fields.length === 0) return null
      const fields: UiField[] = []
      for (const item of raw.fields) {
        const field = normalizeUiField(item)
        if (field === null) return null
        fields.push(field)
      }
      const section: UiSection = { id, kind: 'form', fields }
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
      const section: UiSection = { id, kind: 'list', columns }
      const title = normalizeOptionalString(raw.title)
      if (title !== undefined) section.title = title
      if (raw.source === 'provider') section.source = 'provider'
      const rowCommand = normalizeOptionalString(raw.rowCommand)
      if (rowCommand !== undefined) section.rowCommand = rowCommand
      return section
    }
    case 'kv': {
      const section: UiSection = { id, kind: 'kv' }
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
      if (raw.source === 'provider') section.source = 'provider'
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

// ==== 数据回读归一（§3.2 双端白名单归一） ====

export interface UiValues {
  /** form 节：按 fieldKey 的动态初始值（v1 不回读，仅协议面保留）。 */
  values?: Record<string, unknown>
  /** list 节：行（单元格限标量，非标量键丢弃）。 */
  rows?: Record<string, UiScalar>[]
  /** kv 节：键值对。 */
  items?: UiKvItem[]
}

/** 宿主 ui/values 结果按节 kind 白名单归一（照抄 normalizeViewNodes 风格）：
 * 多余键丢弃、单元格/条目非白名单值丢弃或判非法；markdown/form 节无回读
 * 恒 null。宿主侧已归一一次（§3.2），这里不信任 IPC 对端再兜一层（§3.4）。 */
export function normalizeUiValues(kind: UiSectionKind, raw: unknown): UiValues | null {
  switch (kind) {
    case 'markdown':
    case 'form':
      return null
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
  /** M2-R 节数据状态：键 `${pluginId}:${viewId}:${sectionId}`（稳定节 id，
   * list/kv 回读、form 提交注记共用；markdown 与静态节无条目）。 */
  sectionData: Record<string, UiSectionDataState>
  /** 同步代次（§3.6）：applyPluginUiList 每次成功 +1，异步完成时校验。 */
  generation: number
}

type Listener = (state: PluginUiState) => void

const listeners = new Set<Listener>()
let state: PluginUiState = { views: [], sectionData: {}, generation: 0 }

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
  state = { views: [], sectionData: {}, generation: 0 }
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

// ==== 节身份与语义指纹（§3.1/§3.6） ====

/** 节状态键：`${pluginId}:${viewId}:${sectionId}`——稳定节 id 定位，与显示
 * 序号无关（ui/values 的 sectionId 同为该稳定 id；插入/重排节不改变键）。 */
export function uiSectionDataKey(
  pluginId: string,
  viewId: string,
  sectionId: string,
): string {
  return `${pluginId}:${viewId}:${sectionId}`
}

/** 节语义指纹（§3.6）：kind/fields/columns/command/source/default 等语义声明
 * 的序列化。仅 title/label/placeholder/顺序等渲染层变化不改变指纹。 */
export function uiSectionFingerprint(section: UiSection): string {
  switch (section.kind) {
    case 'markdown':
      return JSON.stringify(['markdown', section.text])
    case 'form':
      return JSON.stringify([
        'form',
        section.fields.map(field => [
          field.key,
          field.type,
          field.default ?? null,
          field.options ?? null,
          field.min ?? null,
          field.max ?? null,
        ]),
        section.command ?? null,
      ])
    case 'list':
      return JSON.stringify([
        'list',
        section.columns.map(column => column.key),
        section.rowCommand ?? null,
        section.source ?? null,
      ])
    case 'kv':
      return JSON.stringify(['kv', section.source ?? null])
  }
}

/** 可回读节：list 必填 source:"provider"；kv 有 source:"provider" 才回读，
 * 省略时只渲染静态 items（§3.2）。form/markdown 无回读。 */
function readableSection(
  entry: UiViewEntry | undefined,
  sectionId: string,
): Extract<UiSection, { kind: 'list' | 'kv' }> | null {
  const section = entry?.sections.find(item => item?.id === sectionId)
  if (!section || (section.kind !== 'list' && section.kind !== 'kv')) return null
  return section.source === 'provider' ? section : null
}

// ==== 在途请求登记（§3.6：代次 + 最新请求校验） ====

/** 令牌：作废旧请求靠递增序号（每节 id 一个槽位）。 */
function nextToken(registry: Map<string, number>, key: string): number {
  const token = (registry.get(key) ?? 0) + 1
  registry.set(key, token)
  return token
}

/** 视图级在途请求集合（节 id → 最新令牌）。 */
interface ViewInflight {
  loads: Map<string, number>
  busies: Map<string, number>
}

/** 同一视图的所有在途请求：closeUiView 与同步路径统一作废（§3.6）。 */
const viewInflight = new Map<string, ViewInflight>()

function inflightFor(key: string): ViewInflight {
  let inflight = viewInflight.get(key)
  if (!inflight) {
    inflight = { loads: new Map(), busies: new Map() }
    viewInflight.set(key, inflight)
  }
  return inflight
}

/** 请求正常完成：摘掉令牌；该视图无在途请求时整体摘掉登记。 */
function finishRequest(
  inflight: ViewInflight,
  viewKey: string,
  registry: Map<string, number>,
  key: string,
): void {
  registry.delete(key)
  if (inflight.loads.size === 0 && inflight.busies.size === 0) {
    viewInflight.delete(viewKey)
  }
}

// ==== 同步（App 启动与插件增删后调用，照 syncPluginViews 挂点；§3.6） ====

/**
 * 拉取最新插件列表并做 UI 视图同步；失败返回 null（保持原状）。
 * 插件禁用/删除 → 其视图标签关闭、运行时状态（含在途请求接收资格）清理，
 * 不进「重新打开」栈（§3.6）。
 */
export async function syncPluginUiViews(): Promise<PluginList | null> {
  const res = await extensionsList()
  if (!res.ok) return null
  applyPluginUiList(res.data)
  return res.data
}

/**
 * 对一份插件列表做同步（测试与复用入口）。每次成功推进同步代次
 * （§3.6）：回读/提交/行命令完成时只有代次匹配且仍是该节最新请求才生效。
 * 同 id 节语义声明变化（uiSectionFingerprint）→ 重置该节（清数据与注记，
 * 作废旧请求）；仅 title/label/顺序变化保留输入。删除节/视图/插件清状态。
 */
export function applyPluginUiList(list: PluginList): void {
  const views = declaredUiViews(list)
  const alive = new Set(views.map(view => view.key))
  const generation = state.generation + 1

  // 语义变更重置（§3.6）：沿用旧状态的节，只有指纹一致才保留；同 id 节
  // kind/fields/columns/command/source/default 变化 → 重置（清数据与注记，
  // 作废旧请求）；仅 title/label/placeholder/顺序变化保留输入。被保留节的
  // 在途请求因代次推进一律作废：loading 复位为 idle（组件会按新代次重新
  // 回读）、busy 复位（防止控件永久禁用）。
  const previous = new Map(state.views.map(view => [view.key, view]))
  const keptData: Record<string, UiSectionDataState> = {}
  for (const view of views) {
    const before = previous.get(view.key)
    const inflight = viewInflight.get(view.key)
    view.sections.forEach(section => {
      if (section === null) return
      const key = uiSectionDataKey(view.pluginId, view.viewId, section.id)
      const prior = before?.sections.find(item => item !== null && item.id === section.id)
      if (
        prior !== null &&
        prior !== undefined &&
        uiSectionFingerprint(prior) === uiSectionFingerprint(section)
      ) {
        const existing = state.sectionData[key]
        if (!existing) return
        const reset: Partial<UiSectionDataState> = {}
        if (inflight?.loads.has(key)) {
          nextToken(inflight.loads, key)
          if (existing.status === 'loading') reset.status = 'idle'
        }
        if (inflight?.busies.has(key)) {
          nextToken(inflight.busies, key)
          if (existing.busy) reset.busy = false
        }
        keptData[key] =
          Object.keys(reset).length > 0 ? { ...existing, ...reset } : existing
        return
      }
      // 新增或语义变化：在途请求作废（状态不进 keptData，默认 idle/busy=false）。
      if (inflight?.loads.has(key)) nextToken(inflight.loads, key)
      if (inflight?.busies.has(key)) nextToken(inflight.busies, key)
    })
  }

  // 插件禁用/删除/视图移除：其全部在途请求失去接收资格（§3.6），登记整体清除。
  for (const viewKey of [...viewInflight.keys()]) {
    if (alive.has(viewKey)) continue
    viewInflight.delete(viewKey)
  }

  state = { views, sectionData: keptData, generation }
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

/** 关闭视图：清理该视图全部运行时状态（§3.6）——节数据、在途请求（作废旧
 * 请求并复位 loading/busy，防止控件永久禁用）与登记。组件随之卸载，其本地
 * 草稿（含 password）一并销毁，不进任何全局/持久化存储。重开从清单 default
 * 与新 provider 数据开始。 */
export function closeUiView(pluginId: string, viewId: string): void {
  const viewKey = `${pluginId}:${viewId}`
  const prefix = `${viewKey}:`
  const inflight = viewInflight.get(viewKey)
  if (inflight) {
    for (const key of inflight.loads.keys()) nextToken(inflight.loads, key)
    for (const key of inflight.busies.keys()) nextToken(inflight.busies, key)
    viewInflight.delete(viewKey)
  }
  const sectionData: Record<string, UiSectionDataState> = {}
  for (const [key, value] of Object.entries(state.sectionData)) {
    if (!key.startsWith(prefix)) sectionData[key] = value
  }
  state = { views: state.views, sectionData, generation: state.generation }
  emit()
}

// ==== 节数据回读与交互（§3.2/§3.5，M2-R 代次与最新请求守卫） ====

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

function emptySectionData(): UiSectionDataState {
  return { status: 'idle', values: null, error: null, note: null, busy: false }
}

function sectionDataOf(key: string): UiSectionDataState {
  return state.sectionData[key] ?? emptySectionData()
}

/** 节数据 patch。必须整体换根 state 对象：订阅方（App/PluginUiView）用
 * `ref.value = next` 接 emit，同根对象引用 ref.set 判 hasChanged 为 false
 * 不触发渲染（运行时 e2e 抓到：提交后 store 已新、DOM 停旧）。 */
function patchSectionData(key: string, patch: Partial<UiSectionDataState>): void {
  state = {
    views: state.views,
    generation: state.generation,
    sectionData: { ...state.sectionData, [key]: { ...sectionDataOf(key), ...patch } },
  }
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

/**
 * 拉取节数据（ui/values，§3.2）：sectionId=稳定节 id，仅 source:"provider"
 * 的 list/kv 节回读。失败/形状非法 → 节错误态（可重试，照树视图错误态）。
 * 完成时只有插件/视图/节仍有效、同步代次匹配且仍是该节最新请求才写入
 * （§3.6）；被更新请求取代的旧响应直接丢弃。
 */
export async function loadUiSectionValues(
  pluginId: string,
  viewId: string,
  sectionId: string,
): Promise<void> {
  const entry = findUiView(pluginId, viewId)
  const section = readableSection(entry, sectionId)
  if (!section) return
  const key = uiSectionDataKey(pluginId, viewId, sectionId)
  const generation = state.generation
  const inflight = inflightFor(`${pluginId}:${viewId}`)
  const token = nextToken(inflight.loads, key)
  patchSectionData(key, { status: 'loading', error: null })
  const res = await extensionsUiValues(pluginId, viewId, sectionId)
  // §3.6：节被移除/语义重置（代次变化）或被更新请求取代 → 丢弃旧结果。
  if (state.generation !== generation) return
  if (inflight.loads.get(key) !== token) return
  finishRequest(inflight, `${pluginId}:${viewId}`, inflight.loads, key)
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
  sectionId: string,
): Promise<void> {
  const existing = state.sectionData[uiSectionDataKey(pluginId, viewId, sectionId)]
  if (!existing || existing.status === 'idle') {
    await loadUiSectionValues(pluginId, viewId, sectionId)
  }
}

/** 错误态重试（节错误态的「重试」按钮）。 */
export async function retryUiSection(
  pluginId: string,
  viewId: string,
  sectionId: string,
): Promise<void> {
  await loadUiSectionValues(pluginId, viewId, sectionId)
}

/** 视图内全部 provider 节回读（提交/行命令成功后刷新，§3.5）。 */
function refreshProviderSections(entry: UiViewEntry): void {
  for (const section of entry.sections) {
    if (
      section !== null &&
      (section.kind === 'list' || section.kind === 'kv') &&
      section.source === 'provider'
    ) {
      void retryUiSection(entry.pluginId, entry.viewId, section.id)
    }
  }
}

/**
 * 表单提交（§3.5）：ext_execute_command(section.command, { values })；ok →
 * 成功注记并刷新同视图全部 provider 节（不重置表单草稿）；error → 错误注记
 * （用户输入由组件持有的 draft 保留）。完成时同样受 §3.6 代次与最新请求
 * 守卫：被丢弃时复位 busy，防止控件永久禁用。
 */
export async function submitUiForm(
  pluginId: string,
  viewId: string,
  sectionId: string,
  draft: Record<string, unknown>,
): Promise<boolean> {
  const entry = findUiView(pluginId, viewId)
  const section = entry?.sections.find(item => item?.id === sectionId)
  if (!entry || !section || section.kind !== 'form' || !section.command) return false
  const key = uiSectionDataKey(pluginId, viewId, sectionId)
  const generation = state.generation
  const inflight = inflightFor(`${pluginId}:${viewId}`)
  const token = nextToken(inflight.busies, key)
  const values = coerceUiFormValues(section, draft)
  patchSectionData(key, { busy: true, note: null })
  const res = await extensionsExecuteCommand(section.command, { values })
  // §3.6：代次失配/被新请求取代 → 丢弃结果（busy 由同步路径的复位兜住，
  // 这里不再写入注记/刷新）。
  if (state.generation !== generation) return false
  if (inflight.busies.get(key) !== token) return false
  finishRequest(inflight, `${pluginId}:${viewId}`, inflight.busies, key)
  if (!res.ok) {
    patchSectionData(key, {
      busy: false,
      note: { kind: 'error', message: res.error.message },
    })
    return false
  }
  patchSectionData(key, { busy: false, note: { kind: 'success', message: '' } })
  // §3.5 成功后回读刷新：命令 handler 可能改写 provider 数据，重拉同视图
  // 全部 provider 节（行命令路径 runUiRowCommand 已带同款刷新）。
  refreshProviderSections(entry)
  return true
}

/** list 行点击（§3.5）：rowCommand(row)（整行对象作 args，单元格为归一
 * 标量）；成功后回读刷新 + 成功注记，失败 → 错误注记。§3.6 守卫同
 * submitUiForm。 */
export async function runUiRowCommand(
  pluginId: string,
  viewId: string,
  sectionId: string,
  row: Record<string, UiScalar>,
): Promise<boolean> {
  const entry = findUiView(pluginId, viewId)
  const section = entry?.sections.find(item => item?.id === sectionId)
  if (!entry || !section || section.kind !== 'list' || !section.rowCommand) return false
  const key = uiSectionDataKey(pluginId, viewId, sectionId)
  const generation = state.generation
  const inflight = inflightFor(`${pluginId}:${viewId}`)
  const token = nextToken(inflight.busies, key)
  patchSectionData(key, { busy: true, note: null })
  const res = await extensionsExecuteCommand(section.rowCommand, { ...row })
  if (state.generation !== generation) return false
  if (inflight.busies.get(key) !== token) return false
  finishRequest(inflight, `${pluginId}:${viewId}`, inflight.busies, key)
  if (!res.ok) {
    patchSectionData(key, {
      busy: false,
      note: { kind: 'error', message: res.error.message },
    })
    return false
  }
  await loadUiSectionValues(pluginId, viewId, sectionId)
  // 本节回读完成（回读自身另有代次/令牌守卫）后收尾：busy 复位 + 成功注记。
  // busy 令牌已 finish，这里只校验代次（命令资格在上面的校验已确立）。
  if (state.generation !== generation) return false
  patchSectionData(key, { busy: false, note: { kind: 'success', message: '' } })
  return true
}
