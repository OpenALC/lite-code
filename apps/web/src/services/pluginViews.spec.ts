import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { ExtViewChildrenResult, PluginInfo, PluginList } from './extensions'
import { extensionsViewChildren } from './extensions'
import {
  applyPluginViewList,
  declaredViews,
  getPluginViewsState,
  normalizeViewNode,
  normalizeViewNodes,
  resetPluginViews,
  retryPluginViewSlot,
  runPluginViewCommand,
  syncPluginViews,
  togglePluginViewNode,
  togglePluginViewSection,
  visibleViewRows,
} from './pluginViews'

// vi.mock 提升：用可变状态驱动 mock 的 list / 视图节点 / 命令执行。
const extState = vi.hoisted(() => ({
  list: null as PluginList | null,
  nodes: new Map<string, unknown>(),
  failCommands: new Set<string>(),
  commandCalls: [] as string[],
}))

function internalError(message: string) {
  return { code: 'internal' as const, message, detail: null }
}

type BridgeResult<T> =
  | { ok: true; data: T; fallback: boolean }
  | { ok: false; error: ReturnType<typeof internalError>; fallback: boolean }

vi.mock('./extensions', async importOriginal => {
  const actual = await importOriginal<typeof import('./extensions')>()
  return {
    ...actual,
    extensionsList: vi.fn(async (): Promise<BridgeResult<PluginList>> => {
      if (!extState.list) {
        return { ok: false, error: internalError('列表不可用'), fallback: false }
      }
      return { ok: true, data: extState.list, fallback: false }
    }),
    extensionsViewChildren: vi.fn(
      async (
        pluginId: string,
        viewId: string,
        nodeId?: string,
      ): Promise<BridgeResult<ExtViewChildrenResult>> => {
        const key = `${pluginId}>${viewId}>${nodeId ?? ''}`
        if (!extState.nodes.has(key)) {
          return {
            ok: false,
            error: internalError(`视图不可用：${key}`),
            fallback: false,
          }
        }
        return { ok: true, data: { nodes: extState.nodes.get(key) as unknown[] }, fallback: false }
      },
    ),
    extensionsExecuteCommand: vi.fn(async (command: string): Promise<BridgeResult<unknown>> => {
      extState.commandCalls.push(command)
      if (extState.failCommands.has(command)) {
        return { ok: false, error: internalError('命令失败'), fallback: false }
      }
      return { ok: true, data: { result: null }, fallback: false }
    }),
  }
})

function plugin(partial: Partial<PluginInfo> & { id: string }): PluginInfo {
  return {
    name: partial.id,
    version: '0.1.0',
    description: '',
    runtime: 'node',
    enabled: true,
    path: `~/.lite-code/extensions/${partial.id}`,
    error: null,
    commands: [],
    mcpServers: [],
    languageServers: [],
    themes: [],
    languages: [],
    views: [],
    uiViews: [],
    webviews: [],
    ...partial,
  }
}

function viewList(enabled: boolean): PluginList {
  return {
    plugins: [
      plugin({
        id: 'demo.tree',
        name: 'Tree Demo',
        enabled,
        views: [{ id: 'demo-tree', name: 'Demo Tree' }],
      }),
    ],
    dir: '~/.lite-code/extensions',
    nodeAvailable: true,
  }
}

function setDemoNodes() {
  extState.nodes.set('demo.tree>demo-tree>', [
    { id: 'group', label: 'Group', icon: 'folder' },
    { id: 'run', label: 'Run', command: 'demo.tree.run', junk: 1 },
  ])
  extState.nodes.set('demo.tree>demo-tree>group', [
    { id: 'leaf', label: 'Leaf', tooltip: 'a leaf' },
  ])
}

beforeEach(() => {
  setDemoNodes()
})

afterEach(() => {
  extState.list = null
  extState.nodes.clear()
  extState.failCommands.clear()
  extState.commandCalls.length = 0
  vi.clearAllMocks()
  resetPluginViews()
})

describe('节点归一化', () => {
  it('保留 id/label 与可空字符串字段，丢弃其余键', () => {
    expect(
      normalizeViewNode({ id: 'a', label: 'A', icon: 'file', command: 'x', tooltip: 't', extra: 1 }),
    ).toEqual({ id: 'a', label: 'A', icon: 'file', command: 'x', tooltip: 't' })
    expect(normalizeViewNode({ id: 'a', label: 'A' })).toEqual({ id: 'a', label: 'A' })
    expect(normalizeViewNode({ id: 'a', label: 'A', icon: 42, command: null })).toEqual({
      id: 'a',
      label: 'A',
    })
  })

  it('id/label 必填非空字符串，否则返回 null', () => {
    expect(normalizeViewNode(null)).toBeNull()
    expect(normalizeViewNode('x')).toBeNull()
    expect(normalizeViewNode([])).toBeNull()
    expect(normalizeViewNode({ label: 'A' })).toBeNull()
    expect(normalizeViewNode({ id: '', label: 'A' })).toBeNull()
    expect(normalizeViewNode({ id: 'a' })).toBeNull()
    expect(normalizeViewNode({ id: 'a', label: '' })).toBeNull()
    expect(normalizeViewNode({ id: 'a', label: 7 })).toBeNull()
  })

  it('数组归一：任一节非法或载荷非数组返回 null；空数组合法', () => {
    expect(normalizeViewNodes([{ id: 'a', label: 'A' }])).toEqual([{ id: 'a', label: 'A' }])
    expect(normalizeViewNodes([])).toEqual([])
    expect(normalizeViewNodes([{ id: 'a', label: 'A' }, { id: 'b' }])).toBeNull()
    expect(normalizeViewNodes({ id: 'a' })).toBeNull()
    expect(normalizeViewNodes('x')).toBeNull()
  })
})

describe('declaredViews 收集', () => {
  it('只收集启用且无 error 插件的视图声明', () => {
    const list = viewList(true)
    list.plugins.push(
      plugin({ id: 'demo.off', enabled: false, views: [{ id: 'off-tree', name: 'Off' }] }),
      plugin({ id: 'demo.broken', error: '清单无效', views: [{ id: 'broken-tree', name: 'B' }] }),
    )
    const sections = declaredViews(list)
    expect(sections).toEqual([
      {
        key: 'demo.tree:demo-tree',
        pluginId: 'demo.tree',
        pluginName: 'Tree Demo',
        viewId: 'demo-tree',
        viewName: 'Demo Tree',
      },
    ])
  })
})

describe('pluginViews 同步与展开', () => {
  it('syncPluginViews：声明 → 分区；展开分区拉根节点', async () => {
    extState.list = viewList(true)
    const list = await syncPluginViews()
    expect(list?.plugins).toHaveLength(1)
    expect(getPluginViewsState().sections).toHaveLength(1)
    expect(getPluginViewsState().sections[0].collapsed).toBe(true)

    await togglePluginViewSection('demo.tree:demo-tree')
    const section = getPluginViewsState().sections[0]
    expect(section.collapsed).toBe(false)
    expect(section.slots[''].status).toBe('ready')
    expect(section.slots[''].nodes).toHaveLength(2)
    expect(extensionsViewChildren).toHaveBeenCalledWith('demo.tree', 'demo-tree', undefined)
  })

  it('syncPluginViews 失败返回 null 且保持原状', async () => {
    extState.list = viewList(true)
    await syncPluginViews()
    extState.list = null
    const failed = await syncPluginViews()
    expect(failed).toBeNull()
    expect(getPluginViewsState().sections).toHaveLength(1)
  })

  it('重复同步保留展开状态与缓存', async () => {
    extState.list = viewList(true)
    await syncPluginViews()
    await togglePluginViewSection('demo.tree:demo-tree')
    await applyPluginViewList(extState.list)
    const section = getPluginViewsState().sections[0]
    expect(section.collapsed).toBe(false)
    expect(section.slots[''].status).toBe('ready')
  })

  it('声明消失（插件禁用）分区移除', async () => {
    extState.list = viewList(true)
    await syncPluginViews()
    extState.list = viewList(false)
    await syncPluginViews()
    expect(getPluginViewsState().sections).toHaveLength(0)
  })

  it('节点展开拉 children（nodeId=节点 id），摊平行随展开更新', async () => {
    extState.list = viewList(true)
    await syncPluginViews()
    await togglePluginViewSection('demo.tree:demo-tree')
    const key = 'demo.tree:demo-tree'

    let rows = visibleViewRows(getPluginViewsState().sections[0])
    expect(rows.map(row => (row.kind === 'node' ? `${row.depth}:${row.node.id}` : row.kind))).toEqual([
      '0:group',
      '0:run',
    ])

    await togglePluginViewNode(key, 'group', 'group')
    expect(extensionsViewChildren).toHaveBeenCalledWith('demo.tree', 'demo-tree', 'group')
    rows = visibleViewRows(getPluginViewsState().sections[0])
    expect(
      rows.map(row => (row.kind === 'node' ? `${row.depth}:${row.node.id}` : `${row.kind}@${row.depth}`)),
    ).toEqual(['0:group', '1:leaf', '0:run'])

    // 折叠后子行消失，缓存保留（再展开不重复拉取）。
    const callsBefore = vi.mocked(extensionsViewChildren).mock.calls.length
    await togglePluginViewNode(key, 'group', 'group')
    expect(visibleViewRows(getPluginViewsState().sections[0])).toHaveLength(2)
    await togglePluginViewNode(key, 'group', 'group')
    expect(vi.mocked(extensionsViewChildren).mock.calls.length).toBe(callsBefore)
    expect(visibleViewRows(getPluginViewsState().sections[0])).toHaveLength(3)
  })

  it('读失败/插件禁用 → 错误态，重试恢复', async () => {
    extState.list = viewList(true)
    await syncPluginViews()
    extState.nodes.clear()
    await togglePluginViewSection('demo.tree:demo-tree')
    let section = getPluginViewsState().sections[0]
    expect(section.slots[''].status).toBe('error')
    expect(section.slots[''].error?.code).toBe('internal')

    setDemoNodes()
    await retryPluginViewSlot('demo.tree:demo-tree', '', null)
    section = getPluginViewsState().sections[0]
    expect(section.slots[''].status).toBe('ready')
    expect(section.slots[''].nodes).toHaveLength(2)
  })

  it('节点形状非法 → 槽位错误态（可重试）', async () => {
    extState.list = viewList(true)
    await syncPluginViews()
    extState.nodes.set('demo.tree>demo-tree>', [{ id: 'ok', label: 'OK' }, { id: 'bad' }])
    await togglePluginViewSection('demo.tree:demo-tree')
    const section = getPluginViewsState().sections[0]
    expect(section.slots[''].status).toBe('error')
    expect(section.slots[''].error?.message).toContain('非法节点')
  })

  it('命令路由：带 command 节点点击走 extensionsExecuteCommand', async () => {
    const ok = await runPluginViewCommand('demo.tree.run')
    expect(ok).toBe(true)
    expect(extState.commandCalls).toEqual(['demo.tree.run'])

    extState.failCommands.add('demo.tree.run')
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const failed = await runPluginViewCommand('demo.tree.run')
    expect(failed).toBe(false)
    expect(warnSpy).toHaveBeenCalled()
    warnSpy.mockRestore()
  })
})
