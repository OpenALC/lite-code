// 插件沙箱 webview（docs/plans/plugin-webview.md §3.8 M3）：iframe 容器协议、
// 一次性握手 + 专属 MessagePort 桥、标签与生命周期。分层镜像 pluginUi.ts：
// 本服务管声明收集/守卫归一/标签生命周期/URL 生成/桥身份绑定与 op 分发，
// PluginWebviewView.vue 纯渲染（sandbox iframe，唯一 src 来自 pluginAssetUrl）。
//
// 安全契约（§3.8，与 Rust 层对齐）：
// - 统一 URL：pluginAssetUrl(pluginId, webviewId, asset) 生成
//   /<pluginId>/<webviewId>/<asset>；Linux = lc-plugin://localhost，
//   Windows = http://lc-plugin.localhost（HTTPS 映射由 Rust 侧开启后同生成器输出）。
// - sandbox 精确值 allow-scripts（组件层 :sandbox 绑定，无 allow-same-origin）：
//   opaque origin、无存储、禁父访问；event.origin === "null" 是预期格式，
//   不能用来区分插件——身份只认 (contentWindow, nonce, 代次)。
// - 握手只接受一次：{ source:"lc-plugin", protocol:1, op:"ready", nonce }，
//   校验 event.source === iframe.contentWindow、nonce、代次与帧形状；通过后
//   以 targetOrigin "*" transfer MessagePort，只发初始化帧，不发状态/凭据。
// - 桥 ops 闭集 getState(key)/setState(key,value)/postCommand(command,args)，
//   请求带 seq，响应 { source:"lc-host", seq, ok, ... }，未知 op 拒绝。
//   state 键 ≤64 字符、单值 ≤64 KiB（web 预校验；每插件 ≤256 KiB 由 Rust 拒绝）；
//   postCommand 以绑定插件身份重新授权（web 侧按清单命令归属预校验越权，
//   Rust 侧 ext_execute_command 再做归属校验）。
// - iframe 重载/导航/卸载或插件禁用 → 关端口、清 nonce、丢弃在途响应；
//   重载重新握手，旧端口不复用；握手后的 load 事件 = 导航 → 桥失效。
import {
  extensionsExecuteCommand,
  extensionsList,
  extensionsWebviewAssetUrl,
  extensionsWebviewStateGet,
  extensionsWebviewStateSet,
  type PluginList,
  type PluginWebview,
} from './extensions'
import { closeTab, openTab, tabsStore } from './tabs'
import type { BridgeResult } from './bridge'

export type { PluginWebview }
// URL 生成的单一来源在 extensions.ts（纯函数 + ext_webview_asset_url 命令
// 包装）；这里原样再导出，组件/测试从本服务取全部 webview 面。
export {
  LC_PLUGIN_BASE_LINUX,
  LC_PLUGIN_BASE_WINDOWS,
  lcPluginAssetBase,
  pluginAssetUrlWithBase,
} from './extensions'

// ==== 声明守卫与归一（纯函数；不信任 IPC 对端，同 extensions.ts 守卫风格） ====

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

/** 归一后的 webview 声明：只有身份与入口（多余键丢弃；assets 留给 Rust handler）。 */
export interface NormalizedWebview {
  webviewId: string
  /** 入口资产（插件目录内相对路径；iframe src 由 pluginAssetUrl 生成）。 */
  entry: string
}

/** 渲染条目 = 归一声明 ⊕ 插件归属。 */
export interface WebviewEntry extends NormalizedWebview {
  /** 状态键：`${pluginId}:${webviewId}`。 */
  key: string
  pluginId: string
  pluginName: string
}

/** webview 声明白名单归一：id/entry 非空字符串（assets 是 Rust handler 的
 * 白名单输入，web 渲染不需要，丢弃）；形状非法 → null。 */
export function normalizeWebview(raw: unknown): NormalizedWebview | null {
  if (!isRecord(raw)) return null
  if (typeof raw.id !== 'string' || raw.id.length === 0) return null
  if (typeof raw.entry !== 'string' || raw.entry.length === 0) return null
  return { webviewId: raw.id, entry: raw.entry }
}

/** 声明形状守卫。 */
export function isWebviewDeclaration(raw: unknown): raw is PluginWebview {
  return normalizeWebview(raw) !== null
}

// ==== 统一 URL 生成（§3.8：只允许宿主生成器，web 侧不拼字符串） ====

/**
 * 插件 webview 资产 URL（iframe src 唯一合法来源，注入防御红线）。
 * Tauri 环境经 ext_webview_asset_url 命令由 Rust 生成（Windows/Linux 基址、
 * HTTPS 映射、白名单校验全在宿主）；浏览器 dev 回退纯生成器（演示数据）。
 */
export function pluginAssetUrl(
  pluginId: string,
  webviewId: string,
  asset: string,
): Promise<BridgeResult<{ url: string }>> {
  return extensionsWebviewAssetUrl(pluginId, webviewId, asset)
}

// ==== 标签容器（§3.8：照 __ui__ 先例，id `__wvb__:<pluginId>:<webviewId>`） ====

export const WEBVIEW_TAB_PREFIX = '__wvb__:'

export function webviewTabId(pluginId: string, webviewId: string): string {
  return `${WEBVIEW_TAB_PREFIX}${pluginId}:${webviewId}`
}

/** 标签 id → webview 引用。webviewId 经 Rust 校验不含 ':'，从最后一个 ':' 切分。 */
export function parseWebviewTabId(
  tabId: string,
): { pluginId: string; webviewId: string } | null {
  if (!tabId.startsWith(WEBVIEW_TAB_PREFIX)) return null
  const rest = tabId.slice(WEBVIEW_TAB_PREFIX.length)
  const sep = rest.lastIndexOf(':')
  if (sep <= 0 || sep === rest.length - 1) return null
  return { pluginId: rest.slice(0, sep), webviewId: rest.slice(sep + 1) }
}

// ==== 状态 ====

export interface PluginWebviewState {
  entries: WebviewEntry[]
  /** 同步代次：applyPluginWebviewList 每次成功 +1。 */
  generation: number
  /** 启用且无 error 插件的命令 → 插件 id（bridge postCommand 越权预校验）。 */
  commandOwners: ReadonlyMap<string, string>
}

type Listener = (state: PluginWebviewState) => void

const listeners = new Set<Listener>()
let state: PluginWebviewState = {
  entries: [],
  generation: 0,
  commandOwners: new Map(),
}

export function getPluginWebviewState(): PluginWebviewState {
  return state
}

export function subscribePluginWebview(listener: Listener): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

function emit(): void {
  for (const listener of listeners) listener(state)
}

/** 测试用：清空声明状态。 */
export function resetPluginWebview(): void {
  state = { entries: [], generation: 0, commandOwners: new Map() }
  emit()
}

/** 从插件列表收集启用且无 error 插件的 webview（逐个归一，坏条目跳过）。 */
export function declaredWebviews(list: PluginList): WebviewEntry[] {
  const entries: WebviewEntry[] = []
  for (const plugin of list.plugins) {
    if (!plugin.enabled || plugin.error) continue
    for (const webview of plugin.webviews) {
      const normalized = normalizeWebview(webview)
      if (normalized === null) continue
      entries.push({
        key: `${plugin.id}:${normalized.webviewId}`,
        pluginId: plugin.id,
        pluginName: plugin.name,
        webviewId: normalized.webviewId,
        entry: normalized.entry,
      })
    }
  }
  return entries
}

/**
 * 对一份插件列表做同步（测试与复用入口）。每次成功推进代次；插件禁用/删除
 * → 其 webview 标签关闭（force，不进重开栈），组件卸载随之拆桥（§3.8）。
 */
export function applyPluginWebviewList(list: PluginList): void {
  const entries = declaredWebviews(list)
  const alive = new Set(entries.map(entry => entry.key))
  const commandOwners = new Map<string, string>()
  for (const plugin of list.plugins) {
    if (!plugin.enabled || plugin.error) continue
    for (const command of plugin.commands) {
      // 跨插件命令 id 冲突取首个声明者；碰撞场景 Rust 侧全局映射仍为准。
      if (!commandOwners.has(command.command)) {
        commandOwners.set(command.command, plugin.id)
      }
    }
  }
  state = { entries, generation: state.generation + 1, commandOwners }
  emit()
  for (const tab of [...tabsStore.get().tabs]) {
    const ref = parseWebviewTabId(tab.id)
    if (ref && !alive.has(`${ref.pluginId}:${ref.webviewId}`)) {
      closeTab(tab.id, { force: true, recordClosed: false })
    }
  }
}

/** 拉取最新插件列表并做同步；失败返回 null（保持原状）。 */
export async function syncPluginWebviews(): Promise<PluginList | null> {
  const res = await extensionsList()
  if (!res.ok) return null
  applyPluginWebviewList(res.data)
  return res.data
}

export function findWebview(pluginId: string, webviewId: string): WebviewEntry | undefined {
  return state.entries.find(
    entry => entry.pluginId === pluginId && entry.webviewId === webviewId,
  )
}

/** 打开（或激活）一个已声明 webview 的编辑器区标签；未声明返回 false。 */
export function openPluginWebview(pluginId: string, webviewId: string): boolean {
  const entry = findWebview(pluginId, webviewId)
  if (!entry) return false
  openTab({
    id: webviewTabId(pluginId, webviewId),
    // §3.8 声明形状无 name 字段，标签标题用 webview id。
    title: entry.webviewId,
    path: null,
    content: '',
  })
  return true
}

// ==== 一次性握手 + MessagePort 桥（§3.8） ====

export const WEBVIEW_BRIDGE_PROTOCOL = 1
/** state 键上限（字符数，§3.8；与 Rust chars() 对齐用码点计数）。 */
export const WEBVIEW_STATE_KEY_MAX_CHARS = 64
/** state 单值上限（UTF-8 字节，§3.8；每插件 ≤256 KiB 由 Rust 侧拒绝）。 */
export const WEBVIEW_STATE_VALUE_MAX_BYTES = 64 * 1024
/** 桥 op 闭集（§3.8）。 */
export const WEBVIEW_BRIDGE_OPS = ['getState', 'setState', 'postCommand'] as const

/** 子页握手帧：{ source:"lc-plugin", protocol:1, op:"ready", nonce }。 */
export interface WebviewReadyFrame {
  source: 'lc-plugin'
  protocol: 1
  op: 'ready'
  nonce: string
}

/** 桥请求帧（port 上）：op 闭集外值在分发期拒绝；seq 非有限数回 null。 */
export interface WebviewBridgeRequest {
  source: 'lc-plugin'
  protocol: 1
  op: string
  seq: number
  key?: unknown
  value?: unknown
  command?: unknown
  args?: unknown
}

export function isWebviewReadyFrame(raw: unknown): raw is WebviewReadyFrame {
  return (
    isRecord(raw) &&
    raw.source === 'lc-plugin' &&
    raw.protocol === WEBVIEW_BRIDGE_PROTOCOL &&
    raw.op === 'ready' &&
    typeof raw.nonce === 'string'
  )
}

export function isWebviewBridgeRequest(raw: unknown): raw is WebviewBridgeRequest {
  return (
    isRecord(raw) &&
    raw.source === 'lc-plugin' &&
    raw.protocol === WEBVIEW_BRIDGE_PROTOCOL &&
    typeof raw.op === 'string'
  )
}

/** 初始化帧（握手后随端口转移发送；只含身份，不含状态/凭据）。 */
export interface WebviewInitFrame {
  source: 'lc-host'
  protocol: 1
  op: 'init'
  pluginId: string
  webviewId: string
}

/** 父侧绑定的子窗口最小面（生产=iframe.contentWindow；测试可注入假窗口）。 */
export interface WebviewChildWindow {
  postMessage(message: unknown, targetOrigin: string, transfer?: Transferable[]): void
}

/** 消息源注入：返回取消函数。生产默认 window message 监听；测试直接驱动 emit。 */
export type WebviewMessageSink = (
  emit: (event: { data: unknown; source: unknown }) => void,
) => () => void

function defaultMessageSink(emit: (event: { data: unknown; source: unknown }) => void): () => void {
  if (typeof window === 'undefined') return () => {}
  const listener = (event: MessageEvent) => {
    emit({ data: event.data, source: event.source })
  }
  window.addEventListener('message', listener)
  return () => window.removeEventListener('message', listener)
}

export type WebviewBridgeStatus = 'awaiting' | 'ready' | 'closed'
export type WebviewBridgeCloseReason = 'dispose' | 'navigation' | 'error'

export interface CreateWebviewBridgeOptions {
  pluginId: string
  webviewId: string
  /** 绑定的 iframe.contentWindow；event.source 身份校验对象。 */
  contentWindow: WebviewChildWindow
  /** 缺省 window message 监听；注入后可用 emit 驱动握手帧（node 测试）。 */
  messageSink?: WebviewMessageSink
  /** 缺省 crypto.getRandomValues(16) hex；注入用于确定性测试。 */
  nonce?: string
  onStatus?: (status: WebviewBridgeStatus, reason: WebviewBridgeCloseReason | null) => void
}

export interface WebviewBridge {
  readonly pluginId: string
  readonly webviewId: string
  /** 本代次一次性 nonce（组件拼进 src fragment；握手完成后父侧清除预期值）。 */
  readonly nonce: string
  readonly status: WebviewBridgeStatus
  readonly closeReason: WebviewBridgeCloseReason | null
  /** iframe load：握手后的 load = 导航/重载 → 桥失效（§3.8）。 */
  notifyLoaded(): void
  /** iframe error → 桥失效并进入错误态。 */
  notifyFailed(): void
  /** 组件卸载/受控重载：关端口、清 nonce、丢弃在途响应。 */
  dispose(): void
}

function defaultNonce(): string {
  const bytes = new Uint8Array(16)
  crypto.getRandomValues(bytes)
  let out = ''
  for (const byte of bytes) out += byte.toString(16).padStart(2, '0')
  return out
}

function stateKeyError(key: unknown): string | null {
  if (typeof key !== 'string') return 'state 键必须是字符串'
  const chars = [...key].length
  if (chars === 0 || chars > WEBVIEW_STATE_KEY_MAX_CHARS) {
    return `state 键长度必须是 1..${WEBVIEW_STATE_KEY_MAX_CHARS} 字符`
  }
  return null
}

/**
 * 建桥 = 一个加载代次。父侧为每个 iframe 实例生成随机 nonce 并绑定插件、
 * 视图、contentWindow 与本代次；握手只接受一次，重复/旧 nonce/错 source
 * 一律忽略；握手后以 "*" transfer MessagePort，之后请求/响应只走该端口。
 */
export function createWebviewBridge(options: CreateWebviewBridgeOptions): WebviewBridge {
  const { pluginId, webviewId, contentWindow, onStatus } = options
  const sink = options.messageSink ?? defaultMessageSink
  let status: WebviewBridgeStatus = 'awaiting'
  let closeReason: WebviewBridgeCloseReason | null = null
  // 预期 nonce：握手成功后置 null——重放同一 nonce 即失效（一次性）。
  let expectedNonce: string | null = options.nonce ?? defaultNonce()
  const bridgeNonce = expectedNonce
  let port: MessagePort | null = null
  // 每代次首次 load 是初始文档自身完成（子页解析期即发 ready，握手先于
  // load）——登记但不杀桥；第二次起的 load 才是子 frame 导航（§3.8）。
  let initialLoadSettled = false

  const notify = (): void => onStatus?.(status, closeReason)

  const respond = (target: MessagePort, frame: Record<string, unknown>): void => {
    // 在途响应防护：桥已关/已换代（port 身份不符）→ 丢弃（§3.8）。
    if (status !== 'ready' || port !== target) return
    target.postMessage(frame)
  }

  const reject = (
    target: MessagePort,
    seq: number | null,
    code: string,
    message: string,
  ): void => {
    respond(target, {
      source: 'lc-host',
      seq,
      ok: false,
      error: { code, message, detail: null },
    })
  }

  const handleRequest = async (target: MessagePort, raw: WebviewBridgeRequest): Promise<void> => {
    const seq = typeof raw.seq === 'number' && Number.isFinite(raw.seq) ? raw.seq : null
    if (seq === null) {
      return reject(target, null, 'invalid_argument', 'seq 必须是有限数字')
    }
    switch (raw.op) {
      case 'getState': {
        const keyError = stateKeyError(raw.key)
        if (keyError !== null) return reject(target, seq, 'invalid_argument', keyError)
        const res = await extensionsWebviewStateGet(pluginId, raw.key as string)
        if (!res.ok) return reject(target, seq, res.error.code, res.error.message)
        respond(target, { source: 'lc-host', seq, ok: true, value: res.data.value })
        return
      }
      case 'setState': {
        const keyError = stateKeyError(raw.key)
        if (keyError !== null) return reject(target, seq, 'invalid_argument', keyError)
        if (raw.value === undefined) {
          return reject(target, seq, 'invalid_argument', 'setState 缺少 value')
        }
        let serialized: string
        try {
          serialized = JSON.stringify(raw.value) as string
        } catch {
          return reject(target, seq, 'invalid_argument', 'state 值必须是可序列化 JSON')
        }
        if (new TextEncoder().encode(serialized).length > WEBVIEW_STATE_VALUE_MAX_BYTES) {
          return reject(target, seq, 'invalid_argument', 'state 值超过 64 KiB')
        }
        const res = await extensionsWebviewStateSet(pluginId, raw.key as string, raw.value)
        if (!res.ok) return reject(target, seq, res.error.code, res.error.message)
        respond(target, { source: 'lc-host', seq, ok: true })
        return
      }
      case 'postCommand': {
        if (typeof raw.command !== 'string' || raw.command.trim().length === 0) {
          return reject(target, seq, 'invalid_argument', 'postCommand 缺少 command')
        }
        // 父侧绑定的插件身份重新授权：跨插件命令 web 侧预拒绝（零 invoke），
        // 同插件/未知命令交 Rust 侧 ext_execute_command 归属校验（§3.8——
        // invoke 携带 plugin_id，宿主 execute_webview_command 二次校验）。
        const owner = state.commandOwners.get(raw.command)
        if (owner !== undefined && owner !== pluginId) {
          return reject(target, seq, 'forbidden', `命令 ${raw.command} 不属于插件 ${pluginId}`)
        }
        const res = await extensionsExecuteCommand(
          raw.command,
          raw.args === undefined ? undefined : raw.args,
          pluginId,
        )
        if (!res.ok) return reject(target, seq, res.error.code, res.error.message)
        respond(target, { source: 'lc-host', seq, ok: true, result: res.data.result })
        return
      }
      default:
        return reject(target, seq, 'invalid_argument', `未知 op：${raw.op}`)
    }
  }

  const handleWindowMessage = (event: { data: unknown; source: unknown }): void => {
    if (status === 'closed') return
    // 身份：只认绑定的 contentWindow（origin "null" 是预期格式，不参与判定）。
    if (event.source !== contentWindow) return
    // 握手只接受一次：已建端口后重复 ready 一律忽略（nonce 已清，双重失效）。
    if (port !== null) return
    if (!isWebviewReadyFrame(event.data)) return
    // nonce/代次：与当前代次预期值不符（重放/旧代次/错插件）→ 拒绝。
    if (event.data.nonce !== expectedNonce) return
    expectedNonce = null
    const channel = new MessageChannel()
    port = channel.port1
    port.onmessage = message => {
      if (status !== 'ready') return
      const raw: unknown = (message as MessageEvent).data
      if (!isWebviewBridgeRequest(raw)) return
      void handleRequest(port as MessagePort, raw)
    }
    status = 'ready'
    const initFrame: WebviewInitFrame = {
      source: 'lc-host',
      protocol: WEBVIEW_BRIDGE_PROTOCOL,
      op: 'init',
      pluginId,
      webviewId,
    }
    contentWindow.postMessage(initFrame, '*', [channel.port2])
    notify()
  }

  const unsubscribe = sink(handleWindowMessage)

  const teardown = (reason: WebviewBridgeCloseReason): void => {
    if (status === 'closed') return
    status = 'closed'
    closeReason = reason
    expectedNonce = null
    const deadPort = port
    port = null
    deadPort?.close()
    unsubscribe()
    notify()
  }

  notify()

  return {
    pluginId,
    webviewId,
    nonce: bridgeNonce,
    get status() {
      return status
    },
    get closeReason() {
      return closeReason
    },
    notifyLoaded() {
      // iframe 每次 load 触发。每代次第一次 load 是 src 导航进来的初始文档
      // 自身完成加载——子页脚本在解析期就发 ready（VS Code webview 同款
      // 惯例），握手完成先于 load 到达，不能把这次 load 误判为导航；只有
      // 第二次起的 load 才可能是子 frame 离开批准入口（§3.8 导航/重载 →
      // 桥失效）。握手前的首次 load 同理只作初始加载登记。
      if (!initialLoadSettled) {
        initialLoadSettled = true
        return
      }
      if (status !== 'ready') return
      teardown('navigation')
    },
    notifyFailed() {
      teardown('error')
    },
    dispose() {
      teardown('dispose')
    },
  }
}
