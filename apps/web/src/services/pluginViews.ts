// 插件树视图（docs/EXTENSIONS.md §1/§4）：ext_list 发现 contributes.views
// 声明 → 资源管理器侧栏底部渲染可折叠分区（分区头=视图 name，标注来源
// 插件）。首次展开经 ext_view_children 拉根节点，节点展开按 nodeId 拉
// children；带 command 的节点点击走 extensionsExecuteCommand。读失败/插件
// 禁用 → 分区错误态可重试。App 启动与插件增删后 sync。
import {
  extensionsExecuteCommand,
  extensionsList,
  extensionsViewChildren,
  type PluginList,
  type ViewNode,
} from './extensions'

/** 单个视图分区（一个插件视图对应一个可折叠分区）。 */
export interface ViewSection {
  /** 状态键：`${pluginId}:${viewId}`。 */
  key: string
  pluginId: string
  pluginName: string
  viewId: string
  viewName: string
}

export type ViewLoadStatus = 'idle' | 'loading' | 'ready' | 'error'

/** 一个槽位的加载结果：根节点用路径 ''，节点用其路径（见 visibleViewRows）。 */
export interface ViewSlot {
  status: ViewLoadStatus
  nodes: ViewNode[]
  /** 失败时的展示信息（code 进 tooltip）。 */
  error: { code: string; message: string } | null
}

export interface ViewSectionState {
  section: ViewSection
  collapsed: boolean
  /** 槽位按路径键控：'' = 根；节点路径 = 祖先 id 以 '/' 连接。 */
  slots: Record<string, ViewSlot>
  /** 路径键 → 是否展开。 */
  expanded: Record<string, boolean>
}

export interface PluginViewsState {
  sections: ViewSectionState[]
}

type Listener = (state: PluginViewsState) => void

const listeners = new Set<Listener>()
let state: PluginViewsState = { sections: [] }

export function getPluginViewsState(): PluginViewsState {
  return state
}

export function subscribePluginViews(listener: Listener): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

function emit(): void {
  for (const listener of listeners) listener(state)
}

/** 测试用：清空全部分区与缓存。 */
export function resetPluginViews(): void {
  state = { sections: [] }
  emit()
}

function emptySlot(): ViewSlot {
  return { status: 'idle', nodes: [], error: null }
}

// ==== 纯函数（测试入口） ====

/** 节点形状校验（docs/EXTENSIONS.md §4）：id/label 必填非空字符串，icon/
 * command/tooltip 可空字符串；其余键一律丢弃。形状非法返回 null。 */
export function normalizeViewNode(raw: unknown): ViewNode | null {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return null
  const value = raw as Record<string, unknown>
  if (typeof value.id !== 'string' || value.id.length === 0) return null
  if (typeof value.label !== 'string' || value.label.length === 0) return null
  const node: ViewNode = { id: value.id, label: value.label }
  if (typeof value.icon === 'string' && value.icon.length > 0) node.icon = value.icon
  if (typeof value.command === 'string' && value.command.length > 0) {
    node.command = value.command
  }
  if (typeof value.tooltip === 'string' && value.tooltip.length > 0) {
    node.tooltip = value.tooltip
  }
  return node
}

/** 节点数组归一：非数组或任一节点非法返回 null（调用方按读失败处理）。 */
export function normalizeViewNodes(raw: unknown): ViewNode[] | null {
  if (!Array.isArray(raw)) return null
  const nodes: ViewNode[] = []
  for (const item of raw) {
    const node = normalizeViewNode(item)
    if (node === null) return null
    nodes.push(node)
  }
  return nodes
}

/** 从插件列表收集启用插件的视图声明（error 插件没有可用清单）。 */
export function declaredViews(list: PluginList): ViewSection[] {
  const sections: ViewSection[] = []
  for (const plugin of list.plugins) {
    if (!plugin.enabled || plugin.error) continue
    for (const view of plugin.views) {
      sections.push({
        key: `${plugin.id}:${view.id}`,
        pluginId: plugin.id,
        pluginName: plugin.name,
        viewId: view.id,
        viewName: view.name,
      })
    }
  }
  return sections
}

export type ViewRow =
  | { kind: 'node'; node: ViewNode; path: string; depth: number; expanded: boolean }
  | { kind: 'loading'; path: string; depth: number }
  | {
      kind: 'error'
      path: string
      depth: number
      error: { code: string; message: string }
    }

/** 把分区摊平成渲染行（贴 FileTree 的按需一层风格）：展开的节点后跟其子
 * 槽位的 loading/error 行或子节点行。 */
export function visibleViewRows(section: ViewSectionState): ViewRow[] {
  const rows: ViewRow[] = []
  const walk = (nodes: ViewNode[], prefix: string, depth: number): void => {
    for (const node of nodes) {
      const path = prefix ? `${prefix}/${node.id}` : node.id
      const expanded = section.expanded[path] === true
      rows.push({ kind: 'node', node, path, depth, expanded })
      if (!expanded) continue
      const slot = section.slots[path]
      if (!slot || slot.status === 'loading') {
        rows.push({ kind: 'loading', path, depth: depth + 1 })
      } else if (slot.status === 'error' && slot.error) {
        rows.push({ kind: 'error', path, depth: depth + 1, error: slot.error })
      } else if (slot.status === 'ready') {
        walk(slot.nodes, path, depth + 1)
      }
    }
  }
  const roots = section.slots['']
  if (roots?.status === 'ready') walk(roots.nodes, '', 0)
  return rows
}

// ==== 同步 ====

/**
 * 拉取最新插件列表并做视图同步；失败返回 null（视图层保持原状）。
 * App 启动与插件增删后调用。
 */
export async function syncPluginViews(): Promise<PluginList | null> {
  const res = await extensionsList()
  if (!res.ok) return null
  applyPluginViewList(res.data)
  return res.data
}

/** 对一份插件列表做视图同步（测试与复用入口）：保留仍在声明中的分区的
 * 展开状态与已加载缓存；声明消失（插件禁用/删除）的分区整体移除。 */
export function applyPluginViewList(list: PluginList): void {
  const previous = new Map(state.sections.map(section => [section.section.key, section]))
  state.sections = declaredViews(list).map(section => {
    const kept = previous.get(section.key)
    return kept ? { ...kept, section } : { section, collapsed: true, slots: {}, expanded: {} }
  })
  emit()
}

// ==== 交互 ====

function findSection(key: string): ViewSectionState | undefined {
  return state.sections.find(section => section.section.key === key)
}

/** 展开分区：首次展开拉根节点（nodeId 缺省）。 */
export async function togglePluginViewSection(key: string): Promise<void> {
  const section = findSection(key)
  if (!section) return
  section.collapsed = !section.collapsed
  emit()
  if (!section.collapsed && !section.slots['']) {
    await loadSlot(key, '', null)
  }
}

/** 展开/折叠节点：首次展开按 nodeId 拉 children。 */
export async function togglePluginViewNode(
  key: string,
  path: string,
  nodeId: string,
): Promise<void> {
  const section = findSection(key)
  if (!section) return
  section.expanded[path] = section.expanded[path] !== true
  emit()
  if (section.expanded[path] && !section.slots[path]) {
    await loadSlot(key, path, nodeId)
  }
}

/** 重试失败的槽位（读失败/插件禁用的错误态重试按钮）。 */
export async function retryPluginViewSlot(
  key: string,
  path: string,
  nodeId: string | null,
): Promise<void> {
  await loadSlot(key, path, nodeId)
}

async function loadSlot(key: string, path: string, nodeId: string | null): Promise<void> {
  const section = findSection(key)
  if (!section) return
  const slot = section.slots[path] ?? emptySlot()
  section.slots[path] = { ...slot, status: 'loading', error: null }
  emit()
  const res = await extensionsViewChildren(
    section.section.pluginId,
    section.section.viewId,
    nodeId ?? undefined,
  )
  const current = findSection(key)
  if (!current) return // 加载期间分区被同步移除
  const target = current.slots[path] ?? emptySlot()
  if (!res.ok) {
    // 读失败/插件禁用 → 错误态（code 进 tooltip，界面可重试）。
    current.slots[path] = {
      ...target,
      status: 'error',
      error: { code: res.error.code, message: res.error.message },
    }
  } else {
    const nodes = normalizeViewNodes(res.data.nodes)
    if (nodes === null) {
      current.slots[path] = {
        ...target,
        status: 'error',
        error: { code: 'internal', message: '视图返回了非法节点' },
      }
    } else {
      current.slots[path] = { ...target, status: 'ready', nodes, error: null }
    }
  }
  emit()
}

/** 点击带 command 的节点：复用既有命令执行；失败仅注记（结果不进树）。 */
export async function runPluginViewCommand(command: string): Promise<boolean> {
  const res = await extensionsExecuteCommand(command)
  if (!res.ok) {
    console.warn(`[插件视图] 命令执行失败：${command}`, res.error)
    return false
  }
  return true
}
