import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { renderMarkdown } from './markdown'
import type { PluginInfo, PluginList } from './extensions'
import { extensionsExecuteCommand } from './extensions'
import {
  UI_TAB_PREFIX,
  declaredUiViews,
  findUiView,
  getPluginUiState,
  isUiSection,
  isUiView,
  normalizeUiSection,
  normalizeUiValues,
  normalizeUiView,
  openPluginUiView,
  openPluginUiViewByCommand,
  parseUiTabId,
  resetPluginUi,
  resolveUiOpenCommand,
  syncPluginUiViews,
  uiTabId,
} from './pluginUi'
import { INITIAL_TABS_STATE, tabsStore } from './tabs'

// vi.mock 提升：用可变状态驱动 mock 的列表与命令执行计数。
const extState = vi.hoisted(() => ({
  list: null as PluginList | null,
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
    extensionsExecuteCommand: vi.fn(async (command: string): Promise<BridgeResult<unknown>> => {
      extState.commandCalls.push(command)
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
    ...partial,
  }
}

function uiViewList(enabled: boolean): PluginList {
  return {
    plugins: [
      plugin({
        id: 'demo.tree',
        name: 'Tree Demo',
        enabled,
        commands: [{ command: 'demo.tree.board.open', title: 'Open board' }],
        uiViews: [
          {
            id: 'board',
            name: '任务看板',
            openCommand: 'demo.tree.board.open',
            sections: [
              { kind: 'markdown', text: '# 说明' },
              { kind: 'kv', title: '统计', items: [{ key: 'a', value: '1' }] },
            ],
          },
        ],
      }),
    ],
    dir: '~/.lite-code/extensions',
    nodeAvailable: true,
  }
}

function resetTabs(): void {
  tabsStore.set(JSON.parse(JSON.stringify(INITIAL_TABS_STATE)) as typeof INITIAL_TABS_STATE)
}

beforeEach(() => {
  resetTabs()
})

afterEach(() => {
  extState.list = null
  extState.commandCalls.length = 0
  vi.clearAllMocks()
  resetPluginUi()
  resetTabs()
})

describe('节守卫 isUiSection / normalizeUiSection', () => {
  it('markdown：text 原样保留（含 <script> 载荷），多余键丢弃', () => {
    const text = '# 标题\n\n<script>alert(1)</script>'
    expect(normalizeUiSection({ kind: 'markdown', text, evil: 1 })).toEqual({
      kind: 'markdown',
      text,
    })
    expect(isUiSection({ kind: 'markdown', text })).toBe(true)
  })

  it('form：五种 type 合法；default 限标量；select 必须有非空 options', () => {
    const section = normalizeUiSection({
      kind: 'form',
      title: '筛选',
      command: 'demo.filter',
      fields: [
        { key: 'q', label: '关键词', type: 'text', placeholder: '…', default: '' },
        { key: 'limit', label: '条数', type: 'number', min: 1, max: 200, default: 50 },
        { key: 'done', label: '含已完成', type: 'boolean', default: false },
        { key: 'sort', label: '排序', type: 'select', options: ['new', 'old'], default: 'new' },
        { key: 'token', label: '令牌', type: 'password', junk: 'x' },
      ],
      extra: 'dropped',
    })
    expect(section).not.toBeNull()
    expect(section && section.kind === 'form' ? section.fields.length : 0).toBe(5)
    // default 非标量（object/array/null）→ 节非法。
    expect(
      normalizeUiSection({
        kind: 'form',
        fields: [{ key: 'q', label: 'Q', type: 'text', default: { nested: true } }],
      }),
    ).toBeNull()
    // select 缺 options → 节非法。
    expect(
      normalizeUiSection({ kind: 'form', fields: [{ key: 's', label: 'S', type: 'select' }] }),
    ).toBeNull()
  })

  it('list：列白名单归一（width 缺省 1），列数组非空，多余键丢弃', () => {
    expect(
      normalizeUiSection({
        kind: 'list',
        title: '任务',
        valuesCommand: 'demo.list',
        columns: [
          { key: 'title', label: '标题', width: 3, onclick: 'x' },
          { key: 'state', label: '状态' },
        ],
        rowCommand: 'demo.open',
        junk: 1,
      }),
    ).toEqual({
      kind: 'list',
      title: '任务',
      valuesCommand: 'demo.list',
      columns: [
        { key: 'title', label: '标题', width: 3 },
        { key: 'state', label: '状态', width: 1 },
      ],
      rowCommand: 'demo.open',
    })
    expect(normalizeUiSection({ kind: 'list', columns: [] })).toBeNull()
    expect(
      normalizeUiSection({ kind: 'list', columns: [{ key: 't', label: 'T', width: '3' }] }),
    ).toBeNull()
  })

  it('kv：javascript: 载荷按纯文本保留（渲染层零 URL 面），items 形状非法 → null', () => {
    expect(
      normalizeUiSection({
        kind: 'kv',
        items: [{ key: '链接', value: 'javascript:alert(1)', evil: 1 }],
      }),
    ).toEqual({ kind: 'kv', items: [{ key: '链接', value: 'javascript:alert(1)' }] })
    expect(normalizeUiSection({ kind: 'kv', items: [{ key: '', value: 'v' }] })).toBeNull()
    expect(normalizeUiSection({ kind: 'kv', items: [{ key: 'k', value: 1 }] })).toBeNull()
    expect(normalizeUiSection({ kind: 'kv', items: 'nope' })).toBeNull()
  })

  it('未知 kind / 非对象节 / 形状非法一律 null', () => {
    expect(isUiSection({ kind: 'html', text: 'x' })).toBe(false)
    expect(isUiSection('markdown')).toBe(false)
    expect(isUiSection(null)).toBe(false)
    expect(isUiSection({ kind: 'markdown' })).toBe(false)
    expect(isUiSection({ kind: 'markdown', text: 42 })).toBe(false)
  })
})

describe('视图守卫 isUiView / normalizeUiView', () => {
  const valid = {
    id: 'board',
    name: '看板',
    openCommand: 'demo.open',
    sections: [{ kind: 'markdown', text: 'x' }],
  }

  it('合法视图通过；openCommand 可空', () => {
    expect(isUiView(valid)).toBe(true)
    expect(isUiView({ ...valid, openCommand: null })).toBe(true)
    expect(normalizeUiView({ ...valid, openCommand: null })?.openCommand).toBeNull()
  })

  it('id 缺失/openCommand 类型坏/sections 非数组 → null', () => {
    expect(isUiView({ ...valid, id: '' })).toBe(false)
    expect(isUiView({ ...valid, openCommand: 42 })).toBe(false)
    expect(isUiView({ ...valid, sections: 'nope' })).toBe(false)
    expect(isUiView({ ...valid, sections: [{ kind: 'html' }] })).toBe(false)
  })
})

describe('normalizeUiValues（§3.2 骨架，M1 静态渲染不调用）', () => {
  it('form：values 对象归一，多余键丢弃；values 非对象 → null', () => {
    expect(normalizeUiValues('form', { values: { q: 'x', limit: 50 }, junk: 1 })).toEqual({
      values: { q: 'x', limit: 50 },
    })
    expect(normalizeUiValues('form', { rows: [] })).toBeNull()
    expect(normalizeUiValues('form', null)).toBeNull()
  })

  it('list：行内非标量键丢弃；行非对象 / 超 500 行 → null', () => {
    expect(
      normalizeUiValues('list', {
        rows: [{ title: 'a', state: 1, evil: { nested: true }, more: null }],
      }),
    ).toEqual({ rows: [{ title: 'a', state: 1 }] })
    expect(normalizeUiValues('list', { rows: ['nope'] })).toBeNull()
    expect(normalizeUiValues('list', { rows: Array(501).fill({ a: 1 }) })).toBeNull()
  })

  it('kv：items 白名单归一；条目形状非法 / 超 64 → null', () => {
    expect(
      normalizeUiValues('kv', { items: [{ key: 'a', value: 'javascript:alert(1)' }] }),
    ).toEqual({ items: [{ key: 'a', value: 'javascript:alert(1)' }] })
    expect(normalizeUiValues('kv', { items: [{ key: 'a' }] })).toBeNull()
    expect(normalizeUiValues('kv', { items: Array(65).fill({ key: 'k', value: 'v' }) })).toBeNull()
  })

  it('markdown 节无回读：恒 null', () => {
    expect(normalizeUiValues('markdown', { anything: true })).toBeNull()
  })
})

describe('declaredUiViews 与同步', () => {
  it('只收集启用且无 error 插件的视图；节白名单归一', () => {
    const list = uiViewList(true)
    list.plugins.push(
      plugin({ id: 'demo.off', enabled: false, uiViews: list.plugins[0]!.uiViews }),
      plugin({ id: 'demo.broken', error: '清单无效', uiViews: list.plugins[0]!.uiViews }),
    )
    const views = declaredUiViews(list)
    expect(views).toHaveLength(1)
    expect(views[0]).toMatchObject({
      key: 'demo.tree:board',
      pluginId: 'demo.tree',
      pluginName: 'Tree Demo',
      viewId: 'board',
      viewName: '任务看板',
      openCommand: 'demo.tree.board.open',
    })
    expect(views[0]?.sections).toHaveLength(2)
  })

  it('syncPluginUiViews：声明 → 状态；失败返回 null 且保持原状', async () => {
    extState.list = uiViewList(true)
    const synced = await syncPluginUiViews()
    expect(synced?.plugins).toHaveLength(1)
    expect(getPluginUiState().views).toHaveLength(1)
    expect(findUiView('demo.tree', 'board')?.viewName).toBe('任务看板')

    extState.list = null
    expect(await syncPluginUiViews()).toBeNull()
    expect(getPluginUiState().views).toHaveLength(1)
  })

  it('插件禁用/删除 → 其视图标签关闭，其它标签不动', async () => {
    extState.list = uiViewList(true)
    await syncPluginUiViews()
    expect(openPluginUiView('demo.tree', 'board')).toBe(true)
    expect(openPluginUiView('demo.tree', 'missing')).toBe(false)
    const tabId = uiTabId('demo.tree', 'board')
    expect(tabsStore.get().tabs.some(tab => tab.id === tabId)).toBe(true)
    expect(tabsStore.get().activeId).toBe(tabId)
    // UI 标签是非文件缓冲区（path=null），不会被当成文件路径。
    expect(tabsStore.get().tabs.find(tab => tab.id === tabId)?.path).toBeNull()

    // 禁用 → 视图消失 → 同步关闭标签（样例标签保留）。
    extState.list = uiViewList(false)
    await syncPluginUiViews()
    expect(getPluginUiState().views).toHaveLength(0)
    expect(tabsStore.get().tabs.some(tab => tab.id === tabId)).toBe(false)
    expect(tabsStore.get().tabs.some(tab => tab.id === 'sample')).toBe(true)

    // 重新启用 → 视图恢复，标签需重新打开。
    extState.list = uiViewList(true)
    await syncPluginUiViews()
    expect(tabsStore.get().tabs.some(tab => tab.id === tabId)).toBe(false)
  })

  it('标签 id roundtrip：pluginId 可含点，viewId 取最后一个冒号后', () => {
    expect(uiTabId('demo.tree', 'board')).toBe('__ui__:demo.tree:board')
    expect(parseUiTabId('__ui__:demo.tree:board')).toEqual({
      pluginId: 'demo.tree',
      viewId: 'board',
    })
    expect(parseUiTabId(`${UI_TAB_PREFIX}a:b:c`)).toEqual({ pluginId: 'a:b', viewId: 'c' })
    expect(parseUiTabId('__ui__:noview')).toBeNull()
    expect(parseUiTabId('__ui__:')).toBeNull()
    expect(parseUiTabId('main.rs')).toBeNull()
  })
})

describe('openCommand 别名（纯 web 拦截，零 invoke）', () => {
  it('命中别名 → 打开标签；extensionsExecuteCommand 零调用', async () => {
    extState.list = uiViewList(true)
    await syncPluginUiViews()

    expect(resolveUiOpenCommand('demo.tree.board.open')).toEqual({
      pluginId: 'demo.tree',
      viewId: 'board',
    })
    expect(openPluginUiViewByCommand('demo.tree.board.open')).toBe(true)
    expect(extensionsExecuteCommand).not.toHaveBeenCalled()
    expect(extState.commandCalls).toEqual([])
    expect(
      tabsStore.get().tabs.some(tab => tab.id === '__ui__:demo.tree:board'),
    ).toBe(true)
  })

  it('未命中命令 → 返回 false（调用方走 ext_execute_command）', async () => {
    extState.list = uiViewList(true)
    await syncPluginUiViews()
    expect(openPluginUiViewByCommand('demo.tree.other')).toBe(false)
    expect(resolveUiOpenCommand('')).toBeNull()
    expect(resolveUiOpenCommand('  ')).toBeNull()
    expect(tabsStore.get().tabs.some(tab => tab.id.startsWith(UI_TAB_PREFIX))).toBe(false)
  })

  it('别名视图来自最新同步：禁用的插件不再拦截', async () => {
    extState.list = uiViewList(true)
    await syncPluginUiViews()
    extState.list = uiViewList(false)
    await syncPluginUiViews()
    expect(openPluginUiViewByCommand('demo.tree.board.open')).toBe(false)
    expect(extensionsExecuteCommand).not.toHaveBeenCalled()
  })
})

describe('markdown 节渲染契约（唯一 v-html 面）', () => {
  it('节 text 含 <script>/javascript: 载荷：renderMarkdown 全量转义，零 <a> 面', () => {
    const text = '# 说明\n\n<script>alert(1)</script>\n\n[javascript:](javascript:alert(1))'
    const section = normalizeUiSection({ kind: 'markdown', text })
    expect(section).not.toBeNull()
    const html = renderMarkdown(section && section.kind === 'markdown' ? section.text : '')

    // 复用 markdown.spec.ts 模式：先转义全部文本、再插固定标签。
    expect(html).toContain('&lt;script&gt;alert(1)&lt;/script&gt;')
    expect(html).not.toContain('<script>')
    expect(html).not.toContain('<img')
    // v1 子集无链接语法：输出不含任何 <a>（javascript: 不可能变成 href）。
    expect(html).not.toContain('<a ')
    expect(html).not.toContain('<a>')
  })
})
