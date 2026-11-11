import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { renderMarkdown } from './markdown'
import type { PluginInfo, PluginList } from './extensions'
import { extensionsExecuteCommand } from './extensions'
import {
  UI_TAB_PREFIX,
  closeUiView,
  coerceUiFormValues,
  declaredUiViews,
  ensureUiSectionLoaded,
  findUiView,
  getPluginUiState,
  isUiSection,
  isUiView,
  loadUiSectionValues,
  normalizeUiSection,
  normalizeUiValues,
  normalizeUiView,
  openPluginUiView,
  openPluginUiViewByCommand,
  parseUiTabId,
  resetPluginUi,
  resolveUiOpenCommand,
  retryUiSection,
  runUiRowCommand,
  submitUiForm,
  subscribePluginUi,
  syncPluginUiViews,
  uiSectionDataKey,
  uiSectionFingerprint,
  uiTabId,
} from './pluginUi'
import { INITIAL_TABS_STATE, tabsStore } from './tabs'

// vi.mock 提升：用可变状态驱动 mock 的列表、命令执行与 ui/values 回读；
// gate 用于在途请求用例（第一次调用被闸门挂起，测试控制放行时机）。
const extState = vi.hoisted(() => ({
  list: null as PluginList | null,
  commandCalls: [] as Array<{ command: string; args: unknown }>,
  commandGate: null as Promise<void> | null,
  uiValuesCalls: [] as string[],
  uiValuesPayload: { rows: [{ title: '任务一', state: 'open' }] } as unknown,
  uiValuesPayloads: [] as unknown[],
  uiValuesError: null as { code: string; message: string; detail: null } | null,
  uiValuesGate: null as Promise<void> | null,
}))

function internalError(message: string) {
  return { code: 'internal' as const, message, detail: null }
}

type BridgeResult<T> =
  | { ok: true; data: T; fallback: boolean }
  | {
      ok: false
      error: { code: string; message: string; detail: null }
      fallback: boolean
    }

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
    extensionsExecuteCommand: vi.fn(
      async (command: string, args?: unknown): Promise<BridgeResult<unknown>> => {
        extState.commandCalls.push({ command, args: args ?? null })
        if (extState.commandGate) await extState.commandGate
        return { ok: true, data: { result: null }, fallback: false }
      },
    ),
    extensionsUiValues: vi.fn(
      async (
        pluginId: string,
        viewId: string,
        sectionId: string,
      ): Promise<BridgeResult<{ values: unknown }>> => {
        extState.uiValuesCalls.push(`${pluginId}>${viewId}>${sectionId}`)
        if (extState.uiValuesError) {
          return { ok: false, error: extState.uiValuesError, fallback: false }
        }
        // 先取本次载荷再挂闸门：保证载荷与调用一一对应（gated 调用取队首）。
        const payload =
          extState.uiValuesPayloads.length > 0
            ? extState.uiValuesPayloads.shift()
            : extState.uiValuesPayload
        if (extState.uiValuesGate !== null && extState.uiValuesCalls.length === 1) {
          await extState.uiValuesGate
        }
        return { ok: true, data: { values: payload }, fallback: false }
      },
    ),
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

/** M1 夹具：markdown + 静态 kv（无 source 不回读）。 */
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
              { id: 'intro', kind: 'markdown', text: '# 说明' },
              { id: 'stats', kind: 'kv', title: '统计', items: [{ key: 'a', value: '1' }] },
            ],
          },
        ],
      }),
    ],
    dir: '~/.lite-code/extensions',
    nodeAvailable: true,
  }
}

/** M2-R 交互夹具：稳定节 id；form（全 field type）+ list（source:"provider"
 * + rowCommand）+ kv（source:"provider" + 静态 items 并存）。 */
function uiViewListM2(enabled: boolean): PluginList {
  return {
    plugins: [
      plugin({
        id: 'demo.tree',
        name: 'Tree Demo',
        enabled,
        commands: [
          { command: 'demo.tree.filter', title: 'Filter' },
          { command: 'demo.tree.list', title: 'List' },
          { command: 'demo.tree.open', title: 'Open' },
          { command: 'demo.tree.stats', title: 'Stats' },
        ],
        uiViews: [
          {
            id: 'board',
            name: '任务看板',
            openCommand: null,
            sections: [
              {
                id: 'filter',
                kind: 'form',
                title: '筛选',
                command: 'demo.tree.filter',
                fields: [
                  { key: 'q', label: '关键词', type: 'text', default: '' },
                  { key: 'limit', label: '条数', type: 'number', min: 1, max: 200, default: 50 },
                  { key: 'done', label: '含已完成', type: 'boolean', default: false },
                  {
                    key: 'sort',
                    label: '排序',
                    type: 'select',
                    options: ['new', 'old'],
                    default: 'new',
                  },
                  { key: 'token', label: '令牌', type: 'password' },
                ],
              },
              {
                id: 'tasks',
                kind: 'list',
                title: '任务',
                source: 'provider',
                columns: [
                  { key: 'title', label: '标题', width: 3 },
                  { key: 'state', label: '状态', width: 1 },
                ],
                rowCommand: 'demo.tree.open',
              },
              {
                id: 'stats',
                kind: 'kv',
                title: '统计',
                source: 'provider',
                items: [{ key: '静态', value: 'items' }],
              },
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
  extState.commandGate = null
  extState.uiValuesCalls.length = 0
  extState.uiValuesPayload = { rows: [{ title: '任务一', state: 'open' }] }
  extState.uiValuesPayloads.length = 0
  extState.uiValuesError = null
  extState.uiValuesGate = null
  vi.clearAllMocks()
  resetPluginUi()
  resetTabs()
})

describe('节守卫 isUiSection / normalizeUiSection（M2-R DTO）', () => {
  it('markdown：稳定 id 必填，text 原样保留（含 <script> 载荷），多余键丢弃', () => {
    const text = '# 标题\n\n<script>alert(1)</script>'
    expect(normalizeUiSection({ id: 'intro', kind: 'markdown', text, evil: 1 })).toEqual({
      id: 'intro',
      kind: 'markdown',
      text,
    })
    expect(isUiSection({ id: 'intro', kind: 'markdown', text })).toBe(true)
    // M2-R：缺稳定 id → 节非法（不再以数组序号当身份）。
    expect(normalizeUiSection({ kind: 'markdown', text })).toBeNull()
    expect(normalizeUiSection({ id: '', kind: 'markdown', text })).toBeNull()
  })

  it('form：五种 type 合法；default 限标量；select 必须有非空 options', () => {
    const section = normalizeUiSection({
      id: 'filter',
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
    expect(section?.id).toBe('filter')
    // default 非标量（object/array/null）→ 节非法。
    expect(
      normalizeUiSection({
        id: 'f',
        kind: 'form',
        fields: [{ key: 'q', label: 'Q', type: 'text', default: { nested: true } }],
      }),
    ).toBeNull()
    // select 缺 options → 节非法。
    expect(
      normalizeUiSection({
        id: 'f',
        kind: 'form',
        fields: [{ key: 's', label: 'S', type: 'select' }],
      }),
    ).toBeNull()
  })

  it('list：source:"provider" 为回读开关；valuesCommand 字段存在也丢弃（视为无回读）', () => {
    expect(
      normalizeUiSection({
        id: 'tasks',
        kind: 'list',
        title: '任务',
        source: 'provider',
        valuesCommand: 'demo.legacy',
        columns: [
          { key: 'title', label: '标题', width: 3, onclick: 'x' },
          { key: 'state', label: '状态' },
        ],
        rowCommand: 'demo.open',
        junk: 1,
      }),
    ).toEqual({
      id: 'tasks',
      kind: 'list',
      title: '任务',
      source: 'provider',
      columns: [
        { key: 'title', label: '标题', width: 3 },
        { key: 'state', label: '状态', width: 1 },
      ],
      rowCommand: 'demo.open',
    })
    // 旧 valuesCommand 单独存在：不隐式当回读开关（M2-R 不保留两套语义）。
    const legacy = normalizeUiSection({
      id: 'tasks',
      kind: 'list',
      valuesCommand: 'demo.legacy',
      columns: [{ key: 'title', label: '标题' }],
    })
    expect(legacy).not.toBeNull()
    expect(legacy && legacy.kind === 'list' ? legacy.source : 'provider').toBeUndefined()
    expect(normalizeUiSection({ id: 'l', kind: 'list', columns: [] })).toBeNull()
    expect(
      normalizeUiSection({
        id: 'l',
        kind: 'list',
        columns: [{ key: 't', label: 'T', width: '3' }],
      }),
    ).toBeNull()
  })

  it('kv：javascript: 载荷按纯文本保留（渲染层零 URL 面），items 形状非法 → null', () => {
    expect(
      normalizeUiSection({
        id: 'stats',
        kind: 'kv',
        items: [{ key: '链接', value: 'javascript:alert(1)', evil: 1 }],
      }),
    ).toEqual({
      id: 'stats',
      kind: 'kv',
      items: [{ key: '链接', value: 'javascript:alert(1)' }],
    })
    // kv 的 source 可选：有 → 动态回读；无 → 静态。
    const kvDynamic = normalizeUiSection({ id: 's', kind: 'kv', source: 'provider' })
    expect(kvDynamic && kvDynamic.kind === 'kv' ? kvDynamic.source : null).toBe(
      'provider',
    )
    expect(normalizeUiSection({ id: 's', kind: 'kv', valuesCommand: 'legacy' })).toEqual({
      id: 's',
      kind: 'kv',
    })
    expect(normalizeUiSection({ id: 's', kind: 'kv', items: [{ key: '', value: 'v' }] })).toBeNull()
    expect(normalizeUiSection({ id: 's', kind: 'kv', items: [{ key: 'k', value: 1 }] })).toBeNull()
    expect(normalizeUiSection({ id: 's', kind: 'kv', items: 'nope' })).toBeNull()
  })

  it('未知 kind / 非对象节 / 形状非法一律 null', () => {
    expect(isUiSection({ id: 'x', kind: 'html', text: 'x' })).toBe(false)
    expect(isUiSection('markdown')).toBe(false)
    expect(isUiSection(null)).toBe(false)
    expect(isUiSection({ id: 'x', kind: 'markdown' })).toBe(false)
    expect(isUiSection({ id: 'x', kind: 'markdown', text: 42 })).toBe(false)
  })
})

describe('视图守卫 isUiView / normalizeUiView', () => {
  const valid = {
    id: 'board',
    name: '看板',
    openCommand: 'demo.open',
    sections: [{ id: 'intro', kind: 'markdown', text: 'x' }],
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
    expect(isUiView({ ...valid, sections: [{ id: 'x', kind: 'html' }] })).toBe(false)
  })
})

describe('normalizeUiValues（§3.2 双端白名单归一）', () => {
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

  it('markdown/form 节无回读：恒 null（v1 表单只用清单 default）', () => {
    expect(normalizeUiValues('markdown', { anything: true })).toBeNull()
    expect(normalizeUiValues('form', { values: { q: 'x' } })).toBeNull()
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
    const section = normalizeUiSection({ id: 'intro', kind: 'markdown', text })
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

describe('M2-R 节数据回读（ui/values，稳定 sectionId）', () => {
  it('list 节挂载回读：ready 态 + 行数据，sectionId=稳定 id（非序号）', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()

    await ensureUiSectionLoaded('demo.tree', 'board', 'tasks')
    expect(extState.uiValuesCalls).toEqual(['demo.tree>board>tasks'])
    const data = getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'tasks')]
    expect(data?.status).toBe('ready')
    expect(data?.values).toEqual({ rows: [{ title: '任务一', state: 'open' }] })

    // ensure 幂等：ready 后不再发起。
    await ensureUiSectionLoaded('demo.tree', 'board', 'tasks')
    expect(extState.uiValuesCalls).toHaveLength(1)
  })

  it('kv 节回读：动态 items 优先于静态 items', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    extState.uiValuesPayload = { items: [{ key: '动态', value: '值' }] }

    await loadUiSectionValues('demo.tree', 'board', 'stats')
    const data = getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'stats')]
    expect(data?.status).toBe('ready')
    expect(data?.values).toEqual({ items: [{ key: '动态', value: '值' }] })
  })

  it('回读失败 → 节错误态（含错误码）；retryUiSection 重试成功转 ready', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    extState.uiValuesError = { code: 'not_found', message: 'Node 不可用', detail: null }

    await loadUiSectionValues('demo.tree', 'board', 'tasks')
    const key = uiSectionDataKey('demo.tree', 'board', 'tasks')
    const failed = getPluginUiState().sectionData[key]
    expect(failed?.status).toBe('error')
    expect(failed?.error?.code).toBe('not_found')

    // 重试：错误清除后成功。
    extState.uiValuesError = null
    await retryUiSection('demo.tree', 'board', 'tasks')
    const retried = getPluginUiState().sectionData[key]
    expect(retried?.status).toBe('ready')
    expect(retried?.values?.rows).toHaveLength(1)
  })

  it('回归：每次 patch 换根 state 引用（ref.value=next 订阅方靠身份变化触发渲染）', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    const seen: unknown[] = []
    const unsubscribe = subscribePluginUi(next => seen.push(next))
    try {
      await ensureUiSectionLoaded('demo.tree', 'board', 'tasks')
      // loading + ready 至少两次 emit；相邻根引用必须不同，
      // 否则组件 ref.set 判 hasChanged=false 不重渲染（运行时 e2e 真缺陷）。
      expect(seen.length).toBeGreaterThanOrEqual(2)
      for (let i = 1; i < seen.length; i += 1) {
        expect(seen[i]).not.toBe(seen[i - 1])
      }
    } finally {
      unsubscribe()
    }
  })

  it('形状非法回读 → internal 节错误态', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    extState.uiValuesPayload = { rows: 'nope' }

    await loadUiSectionValues('demo.tree', 'board', 'tasks')
    const data = getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'tasks')]
    expect(data?.status).toBe('error')
    expect(data?.error?.code).toBe('internal')
  })

  it('无 source 的节不发起回读（含旧 valuesCommand 载荷）；未声明节安全忽略', async () => {
    extState.list = uiViewList(true)
    await syncPluginUiViews()

    await loadUiSectionValues('demo.tree', 'board', 'stats') // kv 无 source
    await loadUiSectionValues('demo.tree', 'board', 'nope') // 节不存在
    await loadUiSectionValues('demo.tree', 'missing', 'intro') // 视图不存在
    expect(extState.uiValuesCalls).toEqual([])
  })
})

describe('M2-R 生命周期（§3.6）', () => {
  it('节重排/插入不改变既有节身份：数据保留、不重新回读', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    await loadUiSectionValues('demo.tree', 'board', 'tasks')
    extState.uiValuesPayload = { items: [{ key: '静态', value: 'items' }] }
    await loadUiSectionValues('demo.tree', 'board', 'stats')
    expect(extState.uiValuesCalls).toEqual(['demo.tree>board>tasks', 'demo.tree>board>stats'])

    // 重排：stats 提前 + 头部插入新 markdown 节；tasks/stats 语义未变。
    const reordered = structuredClone(uiViewListM2(true))
    const view = reordered.plugins[0]!.uiViews[0]!
    const sections = view.sections
    view.sections = [
      { id: 'intro', kind: 'markdown', text: '# 新说明' },
      sections[2],
      sections[0],
      sections[1],
    ]
    extState.list = reordered
    await syncPluginUiViews()

    // 状态键按稳定 id：重排后数据仍归属原节。
    expect(
      getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'tasks')]?.status,
    ).toBe('ready')
    expect(
      getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'stats')]?.values,
    ).toEqual({ items: [{ key: '静态', value: 'items' }] })

    // 语义未变 → 不重新回读（幂等）。
    extState.uiValuesCalls.length = 0
    await ensureUiSectionLoaded('demo.tree', 'board', 'tasks')
    await ensureUiSectionLoaded('demo.tree', 'board', 'stats')
    expect(extState.uiValuesCalls).toEqual([])
  })

  it('仅 title/label 变化保留节数据与输入（指纹不含渲染文案）', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    extState.uiValuesPayload = { items: [{ key: '静态', value: 'items' }] }
    await loadUiSectionValues('demo.tree', 'board', 'stats')

    const relabeled = structuredClone(uiViewListM2(true))
    const kv = relabeled.plugins[0]!.uiViews[0]!.sections[2] as { title?: string }
    kv.title = '指标'
    extState.list = relabeled
    await syncPluginUiViews()

    const data = getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'stats')]
    expect(data?.status).toBe('ready')
    expect(data?.values).toEqual({ items: [{ key: '静态', value: 'items' }] })
  })

  it('同 id 节语义变化（列/数据源/default）→ 重置该节并按新声明重新加载', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    await loadUiSectionValues('demo.tree', 'board', 'tasks')
    await submitUiForm('demo.tree', 'board', 'filter', { q: 'x', limit: '5' })
    expect(
      getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'filter')]?.note
        ?.kind,
    ).toBe('success')

    // tasks：columns 语义变化；stats：移除 source（动态 → 静态）；filter：default 变化。
    const changed = structuredClone(uiViewListM2(true))
    const view = changed.plugins[0]!.uiViews[0]!
    const listSection = view.sections[1] as {
      columns: Array<{ key: string; label: string; width: number }>
    }
    listSection.columns = [{ key: 'title', label: '标题', width: 3 }]
    delete (view.sections[2] as { source?: string }).source
    const formSection = view.sections[0] as {
      fields: Array<{ key: string; default?: unknown }>
    }
    formSection.fields.find(field => field.key === 'limit')!.default = 100
    extState.list = changed
    await syncPluginUiViews()

    // 三个语义变化的节全部重置（状态清除，组件回退默认空态）。
    expect(getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'tasks')]).toBeUndefined()
    expect(getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'stats')]).toBeUndefined()
    expect(getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'filter')]).toBeUndefined()

    // 语义变化节重新加载；stats 已无 source → 不回读。
    extState.uiValuesCalls.length = 0
    await ensureUiSectionLoaded('demo.tree', 'board', 'tasks')
    await ensureUiSectionLoaded('demo.tree', 'board', 'stats')
    expect(extState.uiValuesCalls).toEqual(['demo.tree>board>tasks'])
  })

  it('乱序返回只采用最新请求（旧响应丢弃）', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    extState.uiValuesPayloads = [
      { rows: [{ title: '旧响应', state: 'open' }] },
      { rows: [{ title: '新响应', state: 'done' }] },
    ]
    let release!: () => void
    extState.uiValuesGate = new Promise<void>(resolve => {
      release = resolve
    })

    const first = loadUiSectionValues('demo.tree', 'board', 'tasks')
    const second = loadUiSectionValues('demo.tree', 'board', 'tasks')
    release()
    await Promise.all([first, second])

    expect(extState.uiValuesCalls).toHaveLength(2)
    const data = getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'tasks')]
    expect(data?.status).toBe('ready')
    expect(data?.values).toEqual({ rows: [{ title: '新响应', state: 'done' }] })
  })

  it('代次失配丢弃：插件禁用后旧回读不写回、标签关闭、不恢复已删除状态', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    openPluginUiView('demo.tree', 'board')

    let release!: () => void
    extState.uiValuesGate = new Promise<void>(resolve => {
      release = resolve
    })
    const pending = loadUiSectionValues('demo.tree', 'board', 'tasks')

    // 插件禁用 → 同步推进代次、清理运行时状态并关标签。
    extState.list = uiViewListM2(false)
    await syncPluginUiViews()
    expect(
      tabsStore.get().tabs.some(tab => tab.id === uiTabId('demo.tree', 'board')),
    ).toBe(false)

    release()
    await pending
    expect(getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'tasks')]).toBeUndefined()

    // 重新启用也不恢复旧响应：重开后按新代次重新回读。
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    extState.uiValuesCalls.length = 0
    await ensureUiSectionLoaded('demo.tree', 'board', 'tasks')
    expect(extState.uiValuesCalls).toEqual(['demo.tree>board>tasks'])
  })

  it('同步作废旧请求并复位 busy（语义未变）：提交按钮不永久禁用', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()

    let release!: () => void
    extState.commandGate = new Promise<void>(resolve => {
      release = resolve
    })
    const pending = submitUiForm('demo.tree', 'board', 'filter', { q: 'x' })
    expect(
      getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'filter')]?.busy,
    ).toBe(true)

    // 同步（语义未变）→ 代次推进，在途提交作废且 busy 复位。
    await syncPluginUiViews()
    expect(
      getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'filter')]?.busy,
    ).toBe(false)

    release()
    expect(await pending).toBe(false)
    expect(
      getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'filter')]?.note,
    ).toBeNull()

    // busy 已复位：保留的表单可再次提交并收到成功注记。
    extState.commandGate = null
    expect(await submitUiForm('demo.tree', 'board', 'filter', { q: 'y' })).toBe(true)
    expect(
      getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'filter')]?.note
        ?.kind,
    ).toBe('success')
  })

  it('关闭视图清理该视图运行时状态：在途回读丢弃，重开从 default/新数据开始', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    openPluginUiView('demo.tree', 'board')
    await loadUiSectionValues('demo.tree', 'board', 'tasks')
    expect(
      getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'tasks')]?.status,
    ).toBe('ready')

    // 关闭视图（组件卸载触发）：节数据清除、在途请求作废。
    let release!: () => void
    extState.uiValuesGate = new Promise<void>(resolve => {
      release = resolve
    })
    const pending = loadUiSectionValues('demo.tree', 'board', 'tasks')
    closeUiView('demo.tree', 'board')
    expect(
      getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'tasks')],
    ).toBeUndefined()

    release()
    await pending
    // 旧响应不写回（重开也不恢复）。
    expect(
      getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'tasks')],
    ).toBeUndefined()

    // 声明仍在 → 重开按新数据重新回读。
    expect(openPluginUiView('demo.tree', 'board')).toBe(true)
    extState.uiValuesCalls.length = 0
    await ensureUiSectionLoaded('demo.tree', 'board', 'tasks')
    expect(extState.uiValuesCalls).toEqual(['demo.tree>board>tasks'])
    expect(
      getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'tasks')]?.status,
    ).toBe('ready')
  })
})

describe('M2-R 交互回传（§3.5）', () => {
  it('form 不回读；提交成功仅刷新同视图 provider 节', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()

    await loadUiSectionValues('demo.tree', 'board', 'filter')
    expect(extState.uiValuesCalls).toEqual([])

    const ok = await submitUiForm('demo.tree', 'board', 'filter', {
      q: 'hello',
      limit: '10',
      done: true,
      sort: 'old',
      token: 'secret-value',
    })
    expect(ok).toBe(true)
    expect(extState.commandCalls).toEqual([
      {
        command: 'demo.tree.filter',
        args: {
          values: { q: 'hello', limit: 10, done: true, sort: 'old', token: 'secret-value' },
        },
      },
    ])
    // 成功后刷新同视图全部 provider 节（tasks + stats；form 自身不回读）。
    expect(extState.uiValuesCalls).toEqual(['demo.tree>board>tasks', 'demo.tree>board>stats'])
  })
})

describe('M2 表单提交（args 组装与类型强制）', () => {
  const formSection = normalizeUiSection({
    id: 'filter',
    kind: 'form',
    command: 'demo.tree.filter',
    fields: [
      { key: 'q', label: '关键词', type: 'text', default: '' },
      { key: 'limit', label: '条数', type: 'number', min: 1, max: 200, default: 50 },
      { key: 'done', label: '含已完成', type: 'boolean', default: false },
      { key: 'sort', label: '排序', type: 'select', options: ['new', 'old'], default: 'new' },
      { key: 'token', label: '令牌', type: 'password' },
    ],
  })

  it('coerceUiFormValues：按 field.type 强制转换', () => {
    if (!formSection || formSection.kind !== 'form') throw new Error('form section')

    // number：数字字符串 → number；空输入回落数字 default；非数字省略。
    expect(
      coerceUiFormValues(formSection, {
        q: '关键词',
        limit: '30',
        done: true,
        sort: 'old',
        token: 'secret-value',
      }),
    ).toEqual({ q: '关键词', limit: 30, done: true, sort: 'old', token: 'secret-value' })
    expect(
      coerceUiFormValues(formSection, { q: 'x', limit: '', done: false, sort: 'new', token: '' }),
    ).toEqual({ q: 'x', limit: 50, done: false, sort: 'new', token: '' })
    expect(
      coerceUiFormValues(formSection, {
        q: 'x',
        limit: 'abc',
        done: 'yes',
        sort: 'new',
        token: 1,
      }),
    ).toEqual({ q: 'x', done: false, sort: 'new', token: '1' })
  })

  it('submitUiForm：成功注记', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()

    const ok = await submitUiForm('demo.tree', 'board', 'filter', { q: 'hello' })
    expect(ok).toBe(true)
    const data = getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'filter')]
    expect(data?.note?.kind).toBe('success')
    expect(data?.busy).toBe(false)
  })

  it('submitUiForm：命令失败 → 错误注记（保留 draft，由组件持有）', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    vi.mocked(extensionsExecuteCommand).mockResolvedValueOnce({
      ok: false,
      error: internalError('handler 炸了'),
      fallback: false,
    })

    const ok = await submitUiForm('demo.tree', 'board', 'filter', { q: 'x' })
    expect(ok).toBe(false)
    const data = getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'filter')]
    expect(data?.note?.kind).toBe('error')
    expect(data?.note?.message).toContain('handler 炸了')
  })

  it('表单无 command → 返回 false 且零调用', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    // 把声明里的 command 摘掉（模拟只读表单）。
    const entry = findUiView('demo.tree', 'board')
    const section = entry?.sections.find(item => item?.id === 'filter')
    if (section && section.kind === 'form') delete section.command

    expect(await submitUiForm('demo.tree', 'board', 'filter', { q: 'x' })).toBe(false)
    expect(extState.commandCalls).toEqual([])
  })
})

describe('M2 list 行点击（rowCommand）', () => {
  it('整行对象作 args；成功后回读刷新 + 成功注记', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    await ensureUiSectionLoaded('demo.tree', 'board', 'tasks')
    extState.uiValuesCalls.length = 0

    const ok = await runUiRowCommand('demo.tree', 'board', 'tasks', {
      title: '任务一',
      state: 'open',
    })
    expect(ok).toBe(true)
    expect(extState.commandCalls).toEqual([
      { command: 'demo.tree.open', args: { title: '任务一', state: 'open' } },
    ])
    // 成功后回读刷新（又一次 ui/values）。
    expect(extState.uiValuesCalls).toEqual(['demo.tree>board>tasks'])
    const data = getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'tasks')]
    expect(data?.note?.kind).toBe('success')
    expect(data?.status).toBe('ready')
  })

  it('行命令失败 → 错误注记且不回读', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    vi.mocked(extensionsExecuteCommand).mockResolvedValueOnce({
      ok: false,
      error: internalError('行命令失败'),
      fallback: false,
    })

    const ok = await runUiRowCommand('demo.tree', 'board', 'tasks', { title: 'x' })
    expect(ok).toBe(false)
    expect(extState.uiValuesCalls).toEqual([])
    const data = getPluginUiState().sectionData[uiSectionDataKey('demo.tree', 'board', 'tasks')]
    expect(data?.note?.kind).toBe('error')
  })

  it('无 rowCommand → 返回 false 且零调用', async () => {
    extState.list = uiViewListM2(true)
    await syncPluginUiViews()
    const entry = findUiView('demo.tree', 'board')
    const section = entry?.sections.find(item => item?.id === 'tasks')
    if (section && section.kind === 'list') delete section.rowCommand

    expect(await runUiRowCommand('demo.tree', 'board', 'tasks', { title: 'x' })).toBe(false)
    expect(extState.commandCalls).toEqual([])
  })
})

describe('节语义指纹 uiSectionFingerprint', () => {
  it('kind/fields/columns/command/source/default 变化改变指纹；title/label 变化不变', () => {
    const base = normalizeUiSection({
      id: 's',
      kind: 'list',
      title: '任务',
      source: 'provider',
      columns: [{ key: 'title', label: '标题', width: 3 }],
      rowCommand: 'demo.open',
    })
    if (!base || base.kind !== 'list') throw new Error('list section')
    const same = normalizeUiSection({
      id: 's',
      kind: 'list',
      title: '看板行',
      source: 'provider',
      columns: [{ key: 'title', label: '标题（改）', width: 3 }],
      rowCommand: 'demo.open',
    })
    expect(uiSectionFingerprint(base)).toBe(uiSectionFingerprint(same!))

    // 列集合变化 → 指纹变化。
    const fewer = normalizeUiSection({
      id: 's',
      kind: 'list',
      title: '任务',
      source: 'provider',
      columns: [
        { key: 'title', label: '标题', width: 3 },
        { key: 'state', label: '状态', width: 1 },
      ],
      rowCommand: 'demo.open',
    })
    expect(uiSectionFingerprint(base)).not.toBe(uiSectionFingerprint(fewer!))
    // width 是渲染权重：变化不重置数据。
    const wider = normalizeUiSection({
      id: 's',
      kind: 'list',
      title: '任务',
      source: 'provider',
      columns: [{ key: 'title', label: '标题', width: 5 }],
      rowCommand: 'demo.open',
    })
    expect(uiSectionFingerprint(base)).toBe(uiSectionFingerprint(wider!))
    // source/rowCommand 变化 → 指纹变化。
    const staticList = normalizeUiSection({
      id: 's',
      kind: 'list',
      title: '任务',
      columns: [{ key: 'title', label: '标题', width: 3 }],
      rowCommand: 'demo.open',
    })
    expect(uiSectionFingerprint(base)).not.toBe(uiSectionFingerprint(staticList!))
  })
})
