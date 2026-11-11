// M3 webview 桥与生命周期（docs/plans/plugin-webview.md §3.8/§5 M3）。
// 测试环境是 node（vite.config.ts test.environment）：无 DOM，握手/桥全部经
// 注入的 messageSink 与假 contentWindow 驱动；MessagePort 用 Node 全局
// MessageChannel 做真实往返。sandbox 属性与零 v-html 是组件渲染层红线，
// node 环境无法挂载组件，用源码断言把守（属性精确值/唯一 iframe/无 v-html）。
import { readFileSync } from 'node:fs'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { PluginInfo, PluginList } from './extensions'
import { extensionsExecuteCommand } from './extensions'
import {
  LC_PLUGIN_BASE_LINUX,
  LC_PLUGIN_BASE_WINDOWS,
  WEBVIEW_BRIDGE_OPS,
  WEBVIEW_STATE_KEY_MAX_CHARS,
  WEBVIEW_STATE_VALUE_MAX_BYTES,
  WEBVIEW_TAB_PREFIX,
  applyPluginWebviewList,
  createWebviewBridge,
  declaredWebviews,
  findWebview,
  getPluginWebviewState,
  isWebviewBridgeRequest,
  isWebviewDeclaration,
  isWebviewReadyFrame,
  lcPluginAssetBase,
  normalizeWebview,
  openPluginWebview,
  parseWebviewTabId,
  pluginAssetUrl,
  pluginAssetUrlWithBase,
  resetPluginWebview,
  syncPluginWebviews,
  webviewTabId,
  type WebviewBridge,
  type WebviewChildWindow,
} from './pluginWebview'
import { INITIAL_TABS_STATE, tabsStore } from './tabs'

// vi.mock 提升：可变状态驱动 mock 的列表、命令执行与 webview state 读写；
// stateGate 用于在途响应丢弃用例（调用挂起，测试控制放行时机）。
const extState = vi.hoisted(() => ({
  list: null as PluginList | null,
  commandCalls: [] as Array<{ command: string; args: unknown; pluginId?: string }>,
  assetUrlCalls: [] as Array<{ pluginId: string; webviewId: string; asset: string }>,
  stateCalls: [] as Array<{ pluginId: string; key: string; value?: unknown }>,
  stateValues: new Map<string, unknown>(),
  stateError: null as { code: string; message: string } | null,
  stateGate: null as Promise<void> | null,
}))

function internalError(message: string) {
  return { code: 'internal' as const, message, detail: null }
}

type BridgeResult<T> =
  | { ok: true; data: T; fallback: boolean }
  | { ok: false; error: { code: string; message: string; detail: null }; fallback: false }

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
      async (
        command: string,
        args?: unknown,
        pluginId?: string,
      ): Promise<BridgeResult<{ result: unknown }>> => {
        extState.commandCalls.push({ command, args: args ?? null, pluginId })
        return { ok: true, data: { result: { echoed: command } }, fallback: false }
      },
    ),
    extensionsWebviewAssetUrl: vi.fn(
      async (
        pluginId: string,
        webviewId: string,
        asset: string,
      ): Promise<BridgeResult<{ url: string }>> => {
        extState.assetUrlCalls.push({ pluginId, webviewId, asset })
        return {
          ok: true,
          data: {
            url: actual.pluginAssetUrlWithBase(actual.LC_PLUGIN_BASE_LINUX, pluginId, webviewId, asset),
          },
          fallback: false,
        }
      },
    ),
    extensionsWebviewStateGet: vi.fn(
      async (pluginId: string, key: string): Promise<BridgeResult<{ value: unknown }>> => {
        extState.stateCalls.push({ pluginId, key })
        if (extState.stateGate) await extState.stateGate
        if (extState.stateError) {
          return { ok: false, error: { ...extState.stateError, detail: null }, fallback: false }
        }
        return {
          ok: true,
          data: { value: extState.stateValues.get(`${pluginId}:${key}`) ?? null },
          fallback: false,
        }
      },
    ),
    extensionsWebviewStateSet: vi.fn(
      async (
        pluginId: string,
        key: string,
        value: unknown,
      ): Promise<BridgeResult<{ kind: 'ack' }>> => {
        extState.stateCalls.push({ pluginId, key, value })
        if (extState.stateGate) await extState.stateGate
        if (extState.stateError) {
          return { ok: false, error: { ...extState.stateError, detail: null }, fallback: false }
        }
        extState.stateValues.set(`${pluginId}:${key}`, value)
        return { ok: true, data: { kind: 'ack' }, fallback: false }
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
    webviews: [],
    ...partial,
  }
}

/** A 插件带 webview 与命令；B 插件带另一个 webview 与其自有命令（越权用例）。 */
function webviewList(enabled = true): PluginList {
  return {
    plugins: [
      plugin({
        id: 'demo.wv',
        name: 'Webview Demo',
        enabled,
        commands: [{ command: 'demo.wv.run', title: 'Run' }],
        webviews: [{ id: 'panel', entry: './webviews/panel/index.html' }],
      }),
      plugin({
        id: 'demo.other',
        name: 'Other Demo',
        enabled,
        commands: [{ command: 'demo.other.run', title: 'Other run' }],
        webviews: [{ id: 'other-panel', entry: './webviews/other/index.html' }],
      }),
      plugin({ id: 'demo.plain', name: 'Plain Demo', enabled }),
    ],
    dir: '~/.lite-code/extensions',
    nodeAvailable: true,
  }
}

/** 桥测试夹具：假 contentWindow + 注入 sink；childPort/responses 接管子侧。 */
interface Harness {
  bridge: WebviewBridge
  fakeWindow: { postMessage: ReturnType<typeof vi.fn> }
  /** 子页向父侧发 window message（握手帧）。 */
  childEmit: (data: unknown, source?: unknown) => void
  /** 子侧端口（握手成功后可用）。 */
  childPort: () => MessagePort
  /** 子侧收到的响应帧。 */
  responses: unknown[]
  /** 子侧经端口发请求。 */
  childRequest: (frame: Record<string, unknown>) => void
  /** 完整握手（默认当前 nonce、正确 source）。 */
  handshake: (nonce?: string) => void
  initFrames: () => unknown[]
  transfers: () => MessagePort[][]
}

const bridges: WebviewBridge[] = []

function harness(options: { pluginId?: string; webviewId?: string; nonce?: string } = {}): Harness {
  const fakeWindow = { postMessage: vi.fn() }
  const responses: unknown[] = []
  const initFrames: unknown[] = []
  const transfers: MessagePort[][] = []
  let emitFn: ((event: { data: unknown; source: unknown }) => void) | null = null
  let port2: MessagePort | null = null
  fakeWindow.postMessage.mockImplementation(
    (message: unknown, _targetOrigin: string, transfer?: Transferable[]) => {
      initFrames.push(message)
      const list = (transfer ?? []) as MessagePort[]
      transfers.push(list)
      port2 = list[0] ?? null
      if (port2) {
        port2.onmessage = event => responses.push((event as MessageEvent).data)
      }
    },
  )
  const bridge = createWebviewBridge({
    pluginId: options.pluginId ?? 'demo.wv',
    webviewId: options.webviewId ?? 'panel',
    contentWindow: fakeWindow as unknown as WebviewChildWindow,
    messageSink: emit => {
      emitFn = emit
      return () => {
        emitFn = null
      }
    },
    nonce: options.nonce,
  })
  bridges.push(bridge)
  const childEmit = (data: unknown, source?: unknown): void => {
    emitFn?.({ data, source: source ?? fakeWindow })
  }
  return {
    bridge,
    fakeWindow,
    childEmit,
    childPort: () => {
      if (!port2) throw new Error('握手尚未完成，无子侧端口')
      return port2
    },
    responses,
    childRequest: frame => {
      port2?.postMessage(frame)
    },
    handshake: nonce => {
      childEmit({
        source: 'lc-plugin',
        protocol: 1,
        op: 'ready',
        nonce: nonce ?? bridge.nonce,
      })
    },
    initFrames: () => initFrames,
    transfers: () => transfers,
  }
}

async function flush(rounds = 4): Promise<void> {
  for (let i = 0; i < rounds; i += 1) {
    await new Promise(resolve => setTimeout(resolve, 0))
  }
}

beforeEach(() => {
  extState.list = null
  extState.commandCalls = []
  extState.assetUrlCalls = []
  extState.stateCalls = []
  extState.stateValues = new Map()
  extState.stateError = null
  extState.stateGate = null
  tabsStore.set(JSON.parse(JSON.stringify(INITIAL_TABS_STATE)) as typeof INITIAL_TABS_STATE)
  resetPluginWebview()
})

afterEach(() => {
  while (bridges.length > 0) bridges.pop()?.dispose()
})

// ==== §3.8 渲染层红线：sandbox 精确属性 + 零 v-html（node 环境源码断言） ====

describe('渲染层红线（§3.8：sandbox 精确值 / 唯一 iframe / 零 v-html）', () => {
  // 剥注释后匹配：组件注释里复述红线文案（"不带 allow-same-origin"/"零 v-html"），
  // 不参与渲染层断言。
  const rawSource = readFileSync(
    new URL('../components/PluginWebviewView.vue', import.meta.url),
    'utf8',
  )
  const source = rawSource
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/(^|\s)\/\/.*$/gm, '$1')

  it('iframe sandbox 绑定精确值 allow-scripts（不带 allow-same-origin）', () => {
    expect(source).toContain(`:sandbox="'allow-scripts'"`)
    expect(source).not.toContain('allow-same-origin')
  })

  it('全组件零 v-html、只有一个 iframe、src 只经 pluginAssetUrl 生成器', () => {
    expect(source.match(/v-html/g) ?? []).toHaveLength(0)
    expect(source.match(/<iframe/g) ?? []).toHaveLength(1)
    expect(source).toContain('pluginAssetUrl(')
    // 无第二个 URL 来源（无 :src 拼接字符串以外的绑定）。
    expect(source.match(/:src=/g) ?? []).toHaveLength(1)
  })

  it('ExtensionsPanel：webview 入口受声明门控 + 安装确认固定文案键', () => {
    const panel = readFileSync(
      new URL('../components/ExtensionsPanel.vue', import.meta.url),
      'utf8',
    )
    expect(panel).toContain('plugin.webviews.length > 0')
    expect(panel).toContain('marketplace.webviewWarning')
  })
})

// ==== 统一 URL 生成（§3.8：只允许宿主生成器） ====

describe('pluginAssetUrl（§3.8 统一 URL 与路由）', () => {
  it('逻辑路径 /<pluginId>/<webviewId>/<asset>，pluginId/webviewId 不放 authority', () => {
    expect(
      pluginAssetUrlWithBase(LC_PLUGIN_BASE_LINUX, 'demo.wv', 'panel', './webviews/panel/index.html'),
    ).toBe('lc-plugin://localhost/demo.wv/panel/webviews/panel/index.html')
    expect(
      pluginAssetUrlWithBase(LC_PLUGIN_BASE_WINDOWS, 'demo.wv', 'panel', 'index.html'),
    ).toBe('http://lc-plugin.localhost/demo.wv/panel/index.html')
  })

  it('逐段 encodeURIComponent；剥前导斜杠与 ./', () => {
    expect(
      pluginAssetUrlWithBase(LC_PLUGIN_BASE_LINUX, 'demo.wv', 'panel', '/a b/c.png'),
    ).toBe('lc-plugin://localhost/demo.wv/panel/a%20b/c.png')
    expect(
      pluginAssetUrlWithBase(LC_PLUGIN_BASE_LINUX, 'demo.wv', 'panel', '././index.html'),
    ).toBe('lc-plugin://localhost/demo.wv/panel/index.html')
  })

  it('平台基址：Windows UA → http 映射；Linux/未知 → scheme', () => {
    expect(lcPluginAssetBase('Mozilla/5.0 (Windows NT 10.0; Win64; x64)')).toBe(
      LC_PLUGIN_BASE_WINDOWS,
    )
    expect(lcPluginAssetBase('Mozilla/5.0 (X11; Linux x86_64)')).toBe(LC_PLUGIN_BASE_LINUX)
    expect(lcPluginAssetBase('')).toBe(LC_PLUGIN_BASE_LINUX)
  })

  it('pluginAssetUrl（组件用 async 面）：委派 ext_webview_asset_url 命令', async () => {
    const res = await pluginAssetUrl('demo.wv', 'panel', './webviews/panel/index.html')
    expect(extState.assetUrlCalls).toEqual([
      { pluginId: 'demo.wv', webviewId: 'panel', asset: './webviews/panel/index.html' },
    ])
    expect(res).toEqual({
      ok: true,
      data: { url: 'lc-plugin://localhost/demo.wv/panel/webviews/panel/index.html' },
      fallback: false,
    })
  })
})

// ==== 声明守卫与入口（§3.8 声明 + §5「未启用 webview 的插件无入口」） ====

describe('声明守卫与入口', () => {
  it('normalizeWebview：id/entry 非空字符串；assets 等多余键一律丢弃', () => {
    expect(normalizeWebview({ id: 'panel', entry: './index.html', assets: ['./a.png'] })).toEqual({
      webviewId: 'panel',
      entry: './index.html',
    })
    expect(normalizeWebview({ id: '', entry: './index.html' })).toBeNull()
    expect(normalizeWebview({ id: 'panel', entry: 42 })).toBeNull()
    expect(isWebviewDeclaration({ id: 'panel', entry: './index.html', extra: 'dropped' })).toBe(
      true,
    )
    // assets 形状 fail-closed 在 extensions.ts 列表守卫；本层只归一 web 渲染字段。
    expect(isWebviewDeclaration({ id: 'panel', entry: './index.html', assets: 'nope' })).toBe(
      true,
    )
  })

  it('declaredWebviews：只收集启用且无 error 插件；未声明 webview 的插件无入口', () => {
    const entries = declaredWebviews(webviewList())
    expect(entries.map(entry => entry.key)).toEqual(['demo.wv:panel', 'demo.other:other-panel'])
    // demo.plain 无 webview 声明 → 无入口；禁用/error 插件同样无入口。
    const disabled = webviewList(false)
    disabled.plugins.push(plugin({ id: 'demo.broken', error: '清单无效' }))
    expect(declaredWebviews(disabled)).toEqual([])
  })

  it('openPluginWebview：未声明返回 false；已声明开标签（path=null 非文件缓冲区）', async () => {
    extState.list = webviewList()
    await syncPluginWebviews()
    expect(openPluginWebview('demo.plain', 'panel')).toBe(false)
    expect(openPluginWebview('demo.wv', 'missing')).toBe(false)
    expect(openPluginWebview('demo.wv', 'panel')).toBe(true)
    const tabId = webviewTabId('demo.wv', 'panel')
    const tab = tabsStore.get().tabs.find(item => item.id === tabId)
    expect(tab?.path).toBeNull()
    expect(tab?.title).toBe('panel')
  })

  it('标签 id roundtrip：pluginId 可含点/冒号，webviewId 取最后一个冒号后', () => {
    expect(webviewTabId('demo.wv', 'panel')).toBe('__wvb__:demo.wv:panel')
    expect(parseWebviewTabId('__wvb__:demo.wv:panel')).toEqual({
      pluginId: 'demo.wv',
      webviewId: 'panel',
    })
    expect(parseWebviewTabId(`${WEBVIEW_TAB_PREFIX}a:b:c`)).toEqual({
      pluginId: 'a:b',
      webviewId: 'c',
    })
    expect(parseWebviewTabId('__wvb__:noview')).toBeNull()
    expect(parseWebviewTabId('__wvb__:')).toBeNull()
    expect(parseWebviewTabId('main.rs')).toBeNull()
  })

  it('sync：失败保持原状；插件禁用 → 视图消失且标签被关闭', async () => {
    extState.list = webviewList()
    await syncPluginWebviews()
    expect(getPluginWebviewState().entries).toHaveLength(2)
    expect(findWebview('demo.wv', 'panel')?.entry).toBe('./webviews/panel/index.html')

    openPluginWebview('demo.wv', 'panel')
    const tabId = webviewTabId('demo.wv', 'panel')
    expect(tabsStore.get().tabs.some(tab => tab.id === tabId)).toBe(true)

    extState.list = null
    expect(await syncPluginWebviews()).toBeNull()
    expect(getPluginWebviewState().entries).toHaveLength(2)

    applyPluginWebviewList(webviewList(false))
    expect(getPluginWebviewState().entries).toHaveLength(0)
    expect(tabsStore.get().tabs.some(tab => tab.id === tabId)).toBe(false)
    expect(tabsStore.get().tabs.some(tab => tab.id === 'sample')).toBe(true)
  })
})

// ==== 一次性握手（§3.8：source/nonce/代次/帧形状；origin "null" 不参与判定） ====

describe('一次性握手（§3.8）', () => {
  it('正确帧完成握手：status ready + 初始化帧 + "*" transfer 端口', () => {
    const h = harness({ nonce: 'n-1' })
    h.handshake()
    expect(h.bridge.status).toBe('ready')
    expect(h.initFrames()).toEqual([
      { source: 'lc-host', protocol: 1, op: 'init', pluginId: 'demo.wv', webviewId: 'panel' },
    ])
    // opaque 接收方：targetOrigin 必须 "*"；transfer 恰好一个端口。
    expect(h.fakeWindow.postMessage).toHaveBeenCalledWith(h.initFrames()[0], '*', [
      h.childPort(),
    ])
  })

  it('错误 source（非绑定 contentWindow）→ 拒绝，哪怕 nonce 正确', () => {
    const h = harness({ nonce: 'n-1' })
    h.childEmit({ source: 'lc-plugin', protocol: 1, op: 'ready', nonce: 'n-1' }, {})
    expect(h.bridge.status).toBe('awaiting')
    expect(h.fakeWindow.postMessage).not.toHaveBeenCalled()
  })

  it('错误 nonce / 帧形状坏（缺 nonce、protocol 2、op 非 ready）→ 拒绝', () => {
    const h = harness({ nonce: 'n-1' })
    h.childEmit({ source: 'lc-plugin', protocol: 1, op: 'ready', nonce: 'WRONG' })
    h.childEmit({ source: 'lc-plugin', protocol: 1, op: 'ready' })
    h.childEmit({ source: 'lc-plugin', protocol: 2, op: 'ready', nonce: 'n-1' })
    h.childEmit({ source: 'lc-plugin', protocol: 1, op: 'hello', nonce: 'n-1' })
    h.childEmit('garbage')
    expect(h.bridge.status).toBe('awaiting')
    expect(h.fakeWindow.postMessage).not.toHaveBeenCalled()
    // 帧形状守卫（纯函数面）。
    expect(isWebviewReadyFrame({ source: 'lc-plugin', protocol: 1, op: 'ready', nonce: 'x' })).toBe(
      true,
    )
    expect(isWebviewReadyFrame({ source: 'lc-plugin', protocol: 1, op: 'ready' })).toBe(false)
  })

  it('重复握手拒绝：已握手后重放 ready（原 nonce 已一次性清除）不再 transfer', () => {
    const h = harness({ nonce: 'n-1' })
    h.handshake()
    h.fakeWindow.postMessage.mockClear()
    h.handshake('n-1')
    h.handshake('fresh-nonce')
    expect(h.fakeWindow.postMessage).not.toHaveBeenCalled()
    expect(h.transfers()).toHaveLength(1)
  })

  it('代次：重载须重新握手——旧 nonce 拒绝，新 nonce 接受，旧端口不复用', async () => {
    const first = harness({ nonce: 'gen-1' })
    first.handshake()
    const oldPort = first.childPort()
    first.bridge.dispose()

    const second = harness({ nonce: 'gen-2' })
    // 同一 contentWindow（导航后 Window 代理身份不变 → 仍按 source 命中新桥），
    // 但携带旧代次 nonce → 拒绝。
    second.childEmit({ source: 'lc-plugin', protocol: 1, op: 'ready', nonce: 'gen-1' })
    expect(second.bridge.status).toBe('awaiting')
    second.handshake('gen-2')
    expect(second.bridge.status).toBe('ready')

    // 旧端口在新代次上发请求 → 无任何响应（父侧端口已 close）。
    oldPort.postMessage({ source: 'lc-plugin', protocol: 1, op: 'getState', seq: 9, key: 'k' })
    second.childRequest({ source: 'lc-plugin', protocol: 1, op: 'getState', seq: 1, key: 'k' })
    await flush()
    expect(second.responses).toEqual([
      { source: 'lc-host', seq: 1, ok: true, value: null },
    ])
  })

  it('默认 nonce 随机（逐实例不同）；注入 nonce 原样使用', () => {
    const a = harness()
    const b = harness()
    expect(a.bridge.nonce).not.toBe(b.bridge.nonce)
    expect(a.bridge.nonce).toMatch(/^[0-9a-f]{32}$/)
    expect(harness({ nonce: 'fixed' }).bridge.nonce).toBe('fixed')
  })
})

// ==== MessagePort 桥（§3.8：op 闭集 + seq + 身份重新授权） ====

describe('MessagePort 桥（§3.8 op 闭集）', () => {
  it('WEBVIEW_BRIDGE_OPS 闭集 = getState/setState/postCommand', () => {
    expect(WEBVIEW_BRIDGE_OPS).toEqual(['getState', 'setState', 'postCommand'])
  })

  it('getState 往返：{ source:"lc-host", seq, ok:true, value }；未设置 value=null', async () => {
    applyPluginWebviewList(webviewList())
    const h = harness()
    h.handshake()
    h.childRequest({ source: 'lc-plugin', protocol: 1, op: 'getState', seq: 7, key: 'k1' })
    await flush()
    expect(extState.stateCalls).toEqual([{ pluginId: 'demo.wv', key: 'k1' }])
    expect(h.responses).toEqual([{ source: 'lc-host', seq: 7, ok: true, value: null }])

    extState.stateValues.set('demo.wv:k1', { deep: [1, 2] })
    h.childRequest({ source: 'lc-plugin', protocol: 1, op: 'getState', seq: 8, key: 'k1' })
    await flush()
    expect(h.responses[1]).toEqual({ source: 'lc-host', seq: 8, ok: true, value: { deep: [1, 2] } })
  })

  it('setState 往返：写后经 getState 读回；ack ok:true', async () => {
    applyPluginWebviewList(webviewList())
    const h = harness()
    h.handshake()
    h.childRequest({ source: 'lc-plugin', protocol: 1, op: 'setState', seq: 1, key: 'k', value: 'v' })
    await flush()
    expect(h.responses).toEqual([{ source: 'lc-host', seq: 1, ok: true }])
    expect(extState.stateCalls).toEqual([{ pluginId: 'demo.wv', key: 'k', value: 'v' }])

    h.childRequest({ source: 'lc-plugin', protocol: 1, op: 'getState', seq: 2, key: 'k' })
    await flush()
    expect(h.responses[1]).toEqual({ source: 'lc-host', seq: 2, ok: true, value: 'v' })
  })

  it('postCommand 往返：同插件命令 invoke（携带绑定 pluginId）+ result 回传', async () => {
    applyPluginWebviewList(webviewList())
    const h = harness()
    h.handshake()
    h.childRequest({
      source: 'lc-plugin',
      protocol: 1,
      op: 'postCommand',
      seq: 3,
      command: 'demo.wv.run',
      args: { a: 1 },
    })
    await flush()
    expect(extState.commandCalls).toEqual([
      { command: 'demo.wv.run', args: { a: 1 }, pluginId: 'demo.wv' },
    ])
    expect(h.responses).toEqual([
      { source: 'lc-host', seq: 3, ok: true, result: { echoed: 'demo.wv.run' } },
    ])
  })

  it('跨插件命令拒绝：web 侧按清单归属预拒绝（forbidden，零 invoke）', async () => {
    applyPluginWebviewList(webviewList())
    const h = harness()
    h.handshake()
    h.childRequest({ source: 'lc-plugin', protocol: 1, op: 'postCommand', seq: 1, command: 'demo.other.run' })
    await flush()
    expect(h.responses).toEqual([
      {
        source: 'lc-host',
        seq: 1,
        ok: false,
        error: { code: 'forbidden', message: '命令 demo.other.run 不属于插件 demo.wv', detail: null },
      },
    ])
    expect(extensionsExecuteCommand).not.toHaveBeenCalled()
    expect(extState.commandCalls).toEqual([])
  })

  it('未知 op 拒绝 invalid_argument；seq 非数字回 null；错误帧不进 op 分发', async () => {
    const h = harness()
    h.handshake()
    h.childRequest({ source: 'lc-plugin', protocol: 1, op: 'fsRead', seq: 5 })
    h.childRequest({ source: 'lc-plugin', protocol: 1, op: 'getState', seq: 'NaN', key: 'k' })
    h.childRequest({ source: 'evil', protocol: 1, op: 'getState', seq: 6, key: 'k' })
    h.childRequest({ source: 'lc-plugin', protocol: 1, op: 'getState', seq: 7 })
    await flush()
    expect(h.responses).toEqual([
      {
        source: 'lc-host',
        seq: 5,
        ok: false,
        error: { code: 'invalid_argument', message: '未知 op：fsRead', detail: null },
      },
      {
        source: 'lc-host',
        seq: null,
        ok: false,
        error: { code: 'invalid_argument', message: 'seq 必须是有限数字', detail: null },
      },
      {
        source: 'lc-host',
        seq: 7,
        ok: false,
        error: { code: 'invalid_argument', message: 'state 键必须是字符串', detail: null },
      },
    ])
    expect(isWebviewBridgeRequest({ source: 'evil', protocol: 1, op: 'x', seq: 1 })).toBe(
      false,
    )
    expect(isWebviewBridgeRequest({ source: 'lc-plugin', protocol: 2, op: 'x', seq: 1 })).toBe(
      false,
    )
  })

  it('宿主错误透传：state 命令失败 → ok:false + 原错误码原样回传', async () => {
    const h = harness()
    h.handshake()
    extState.stateError = { code: 'quota_exceeded', message: '插件状态超过 256 KiB 配额' }
    h.childRequest({ source: 'lc-plugin', protocol: 1, op: 'setState', seq: 1, key: 'k', value: 1 })
    await flush()
    expect(h.responses).toEqual([
      {
        source: 'lc-host',
        seq: 1,
        ok: false,
        error: {
          code: 'quota_exceeded',
          message: '插件状态超过 256 KiB 配额',
          detail: null,
        },
      },
    ])
    extState.stateError = null
  })

  it('state 尺寸预校验（§3.8：键 ≤64 字符、值 ≤64 KiB，超尺寸零 invoke）', async () => {
    const h = harness()
    h.handshake()
    const longKey = 'k'.repeat(WEBVIEW_STATE_KEY_MAX_CHARS + 1)
    h.childRequest({ source: 'lc-plugin', protocol: 1, op: 'setState', seq: 1, key: longKey, value: 1 })
    const bigValue = 'x'.repeat(WEBVIEW_STATE_VALUE_MAX_BYTES) // JSON 序列化后必超 64 KiB
    h.childRequest({ source: 'lc-plugin', protocol: 1, op: 'setState', seq: 2, key: 'k', value: bigValue })
    h.childRequest({ source: 'lc-plugin', protocol: 1, op: 'getState', seq: 3, key: longKey })
    await flush()
    expect(h.responses.map(frame => (frame as { ok: boolean }).ok)).toEqual([
      false,
      false,
      false,
    ])
    expect(extState.stateCalls).toEqual([])
    // 边界内：恰好 64 字符键 + 恰好 ≤64 KiB 值 → 放行。
    h.childRequest({
      source: 'lc-plugin',
      protocol: 1,
      op: 'setState',
      seq: 4,
      key: 'k'.repeat(WEBVIEW_STATE_KEY_MAX_CHARS),
      value: 'x'.repeat(WEBVIEW_STATE_VALUE_MAX_BYTES - 2), // + 两个引号 = 64 KiB 整
    })
    await flush()
    expect(h.responses[3]).toEqual({ source: 'lc-host', seq: 4, ok: true })
    expect(extState.stateCalls).toHaveLength(1)
  })
})

// ==== 生命周期（§3.8：导航/重载/卸载 → 桥失效；在途响应丢弃） ====

describe('桥生命周期（§3.8）', () => {
  it('快脚本（握手先于 load，真实子页惯例）：首次 load 不杀桥、请求可用；第二次 load（导航）才失效', async () => {
    const h = harness()
    h.handshake()
    expect(h.bridge.status).toBe('ready')

    // 子页解析期握手 → 本代次文档的 load 在握手之后到达：不得误判为导航。
    h.bridge.notifyLoaded()
    expect(h.bridge.status).toBe('ready')
    h.childRequest({ source: 'lc-plugin', protocol: 1, op: 'getState', seq: 1, key: 'k' })
    await flush()
    expect(h.responses).toEqual([
      { source: 'lc-host', seq: 1, ok: true, value: null },
    ])

    // 第二次 load 只能是子 frame 离开批准入口（§3.8）→ 桥失效。
    h.bridge.notifyLoaded()
    expect(h.bridge.status).toBe('closed')
    expect(h.bridge.closeReason).toBe('navigation')

    h.childRequest({ source: 'lc-plugin', protocol: 1, op: 'getState', seq: 2, key: 'k' })
    await flush()
    expect(h.responses).toHaveLength(1)
  })

  it('慢脚本（load 先于握手）：首次 load 只登记；握手后第二次 load 仍是导航失效', () => {
    const h = harness()
    h.bridge.notifyLoaded()
    expect(h.bridge.status).toBe('awaiting')
    h.handshake()
    expect(h.bridge.status).toBe('ready')

    h.bridge.notifyLoaded()
    expect(h.bridge.status).toBe('closed')
    expect(h.bridge.closeReason).toBe('navigation')
  })

  it('iframe error → closed(error)；dispose 后一切消息静默', () => {
    const h = harness()
    h.bridge.notifyFailed()
    expect(h.bridge.status).toBe('closed')
    expect(h.bridge.closeReason).toBe('error')

    const d = harness({ nonce: 'nd' })
    d.handshake()
    d.bridge.dispose()
    expect(d.bridge.status).toBe('closed')
    expect(d.bridge.closeReason).toBe('dispose')
    d.childEmit({ source: 'lc-plugin', protocol: 1, op: 'ready', nonce: 'nd' })
    d.childRequest({ source: 'lc-plugin', protocol: 1, op: 'getState', seq: 1, key: 'k' })
    expect(d.fakeWindow.postMessage).toHaveBeenCalledTimes(1) // 仅握手那一次
  })

  it('在途响应丢弃：state 命令挂起期间拆桥，放行后旧端口收不到响应', async () => {
    let release = (): void => {}
    extState.stateGate = new Promise<void>(resolve => {
      release = resolve
    })
    const h = harness()
    h.handshake()
    h.childRequest({ source: 'lc-plugin', protocol: 1, op: 'getState', seq: 1, key: 'k' })
    await flush(2)
    h.bridge.dispose()
    release()
    await flush()
    expect(h.responses).toEqual([])
    expect(extState.stateCalls).toEqual([{ pluginId: 'demo.wv', key: 'k' }])
  })
})
