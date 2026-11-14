// 终端面板状态、DTO 守卫与控制器（docs/MILESTONES.md P5 issue #63）。
// 框架无关：不可变 reducer + 依赖注入的控制器（UI.md §8），与 dbStore/taskStore 同构。
//
// Rust 契约（camelCase custom DTO，非 core:event 契约信封）：
// - 命令：pty_open / pty_write / pty_resize / pty_close / pty_list / pty_replay
// - 事件：pty:data { sessionId, data } / pty:state { sessionId, state } / pty:exit { sessionId, code }
// 数据通道均为 base64 字符串；wire 数据不可信，入 store 前先过 DTO 守卫。

import type { ErrorCode, ErrorBody } from '@lc-proto'

// ==== DTO 与守卫 ====

export type PtySessionKind = 'local' | 'ssh'
export type PtySessionState = 'connected' | 'reconnecting' | 'closed'

export interface PtySessionInfo {
  sessionId: string
  kind: PtySessionKind
  state: PtySessionState
  title: string
}

export interface PtyOpenResult {
  sessionId: string
}

export interface PtyListResult {
  sessions: PtySessionInfo[]
}

/** pty_replay 结果：base64 的最近输出（Rust 侧 ≤256 KiB 缓冲）。 */
export interface PtyReplayResult {
  data: string
}

export interface PtyDataEvent {
  type: 'pty:data'
  sessionId: string
  /** base64 编码的输出字节。 */
  data: string
}

export interface PtyStateEvent {
  type: 'pty:state'
  sessionId: string
  state: PtySessionState
}

export interface PtyExitEvent {
  type: 'pty:exit'
  sessionId: string
  code: number | null
}

export type PtyEvent = PtyDataEvent | PtyStateEvent | PtyExitEvent

export interface PtyOpenLocal {
  kind: 'local'
  cols: number
  rows: number
}

export interface PtyOpenSsh {
  kind: 'ssh'
  host: string
  /** 省略时 Rust 侧默认 22。 */
  port?: number
  /** 可空：回落远端登录用户。 */
  user?: string | null
  cols: number
  rows: number
}

export type PtyOpenRequest = PtyOpenLocal | PtyOpenSsh

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function isString(value: unknown): value is string {
  return typeof value === 'string'
}

/** base64 字符串守卫：允许空串与标准字母表（含 padding）。 */
function isBase64(value: unknown): value is string {
  if (!isString(value)) return false
  return value.length === 0 || /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value)
}

function isPtySessionKind(value: unknown): value is PtySessionKind {
  return value === 'local' || value === 'ssh'
}

function isPtySessionState(value: unknown): value is PtySessionState {
  return value === 'connected' || value === 'reconnecting' || value === 'closed'
}

export function isPtySessionInfo(value: unknown): value is PtySessionInfo {
  if (!isRecord(value)) return false
  return (
    isString(value.sessionId) &&
    value.sessionId.length > 0 &&
    isPtySessionKind(value.kind) &&
    isPtySessionState(value.state) &&
    isString(value.title)
  )
}

export function isPtyOpenResult(value: unknown): value is PtyOpenResult {
  if (!isRecord(value)) return false
  return isString(value.sessionId) && value.sessionId.length > 0
}

export function isPtyListResult(value: unknown): value is PtyListResult {
  if (!isRecord(value)) return false
  return Array.isArray(value.sessions) && value.sessions.every(isPtySessionInfo)
}

export function isPtyReplayResult(value: unknown): value is PtyReplayResult {
  return isRecord(value) && isBase64(value.data)
}

export function isPtyEvent(value: unknown): value is PtyEvent {
  if (!isRecord(value) || !isString(value.sessionId) || value.sessionId.length === 0) {
    return false
  }
  switch (value.type) {
    case 'pty:data':
      return isBase64(value.data)
    case 'pty:state':
      return isPtySessionState(value.state)
    case 'pty:exit':
      return value.code === null || (typeof value.code === 'number' && Number.isFinite(value.code))
    default:
      return false
  }
}

// ==== base64 编解码（UTF-8 安全，Node/浏览器通用的纯实现） ====

const BASE64_LOOKUP: string[] = (() => {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'
  return alphabet.split('')
})()

export function bytesToBase64(bytes: Uint8Array): string {
  let out = ''
  let i = 0
  for (; i + 3 <= bytes.length; i += 3) {
    const n = (bytes[i]! << 16) | (bytes[i + 1]! << 8) | bytes[i + 2]!
    out += BASE64_LOOKUP[(n >> 18) & 63]! + BASE64_LOOKUP[(n >> 12) & 63]! + BASE64_LOOKUP[(n >> 6) & 63]! + BASE64_LOOKUP[n & 63]!
  }
  const rest = bytes.length - i
  if (rest === 1) {
    const n = bytes[i]! << 16
    out += BASE64_LOOKUP[(n >> 18) & 63]! + BASE64_LOOKUP[(n >> 12) & 63]! + '=='
  } else if (rest === 2) {
    const n = (bytes[i]! << 16) | (bytes[i + 1]! << 8)
    out += BASE64_LOOKUP[(n >> 18) & 63]! + BASE64_LOOKUP[(n >> 12) & 63]! + BASE64_LOOKUP[(n >> 6) & 63]! + '='
  }
  return out
}

/** 非法 base64（错误长度/字母表外字符）返回 null，由调用方丢弃该帧。 */
export function base64ToBytes(value: string): Uint8Array | null {
  const clean = value.replace(/[\s=]/g, '')
  if (!/^[A-Za-z0-9+/]*$/.test(clean)) return null
  if (clean.length % 4 === 1) return null
  const out = new Uint8Array(Math.floor((clean.length * 3) / 4))
  let acc = 0
  let bits = 0
  let p = 0
  for (let i = 0; i < clean.length; i += 1) {
    const v = BASE64_LOOKUP.indexOf(clean[i]!)
    acc = (acc << 6) | v
    bits += 6
    if (bits >= 8) {
      bits -= 8
      out[p] = (acc >> bits) & 0xff
      p += 1
    }
  }
  return out
}

const textEncoder = new TextEncoder()
const textDecoder = new TextDecoder('utf-8', { fatal: false })

/** 输入文本 → base64（UTF-8 编码），pty_write 的 data 载荷。 */
export function utf8ToBase64(text: string): string {
  return bytesToBase64(textEncoder.encode(text))
}

/** base64 → UTF-8 文本；非法 base64 返回 null。 */
export function base64ToUtf8(value: string): string | null {
  const bytes = base64ToBytes(value)
  return bytes === null ? null : textDecoder.decode(bytes)
}

// ==== SSH 表单：草稿 + 校验（纯函数，i18n key 交给视图层翻译） ====

export const SSH_PORT_DEFAULT = 22

export interface SshFormDraft {
  host: string
  /** 端口用字符串承载，空串表示用默认 22。 */
  port: string
  user: string
}

export function createSshDraft(): SshFormDraft {
  return { host: '', port: '', user: '' }
}

/** 解析端口输入：空串回落 22；非法（非数字 / 越界）返回 null。 */
export function parseSshPort(input: string): number | null {
  const trimmed = input.trim()
  if (trimmed.length === 0) return SSH_PORT_DEFAULT
  if (!/^\d+$/.test(trimmed)) return null
  const port = Number(trimmed)
  if (!Number.isInteger(port) || port < 1 || port > 65535) return null
  return port
}

export type SshFormErrorKey = 'terminal.hostRequired' | 'terminal.portInvalid'

export function validateSshDraft(draft: SshFormDraft): SshFormErrorKey | null {
  if (draft.host.trim().length === 0) return 'terminal.hostRequired'
  if (parseSshPort(draft.port) === null) return 'terminal.portInvalid'
  return null
}

/** 校验通过的草稿 → pty_open 的 ssh 请求；user 空串归一为 null。 */
export function buildSshRequest(
  draft: SshFormDraft,
  cols: number,
  rows: number,
): PtyOpenSsh | null {
  if (validateSshDraft(draft) !== null) return null
  const user = draft.user.trim()
  return {
    kind: 'ssh',
    host: draft.host.trim(),
    port: parseSshPort(draft.port) ?? SSH_PORT_DEFAULT,
    user: user.length > 0 ? user : null,
    cols,
    rows,
  }
}

// ==== store：不可变 reducer ====

/** 与 Rust 侧对齐的输出缓冲上限（字节）。 */
export const TERMINAL_BUFFER_LIMIT = 256 * 1024

export interface TerminalSession {
  info: PtySessionInfo
  /** 最近输出缓冲（replay 替换 / data 追加），超过上限从头截断。 */
  buffer: Uint8Array
  exitCode: number | null
}

export interface TerminalState {
  sessions: TerminalSession[]
  activeId: string | null
}

export function createTerminalState(): TerminalState {
  return { sessions: [], activeId: null }
}

function findSession(state: TerminalState, sessionId: string): TerminalSession | null {
  return state.sessions.find(session => session.info.sessionId === sessionId) ?? null
}

/** 截断保留尾部 ≤ limit 字节。 */
function clampBuffer(buffer: Uint8Array): Uint8Array {
  if (buffer.length <= TERMINAL_BUFFER_LIMIT) return buffer
  return buffer.slice(buffer.length - TERMINAL_BUFFER_LIMIT)
}

function upsertSession(state: TerminalState, info: PtySessionInfo): TerminalState {
  const existing = findSession(state, info.sessionId)
  const sessions = existing
    ? state.sessions.map(session =>
        session.info.sessionId === info.sessionId
          ? { ...session, info, exitCode: info.state === 'closed' ? (session.exitCode ?? 0) : session.exitCode }
          : session,
      )
    : [...state.sessions, { info, buffer: new Uint8Array(0), exitCode: null }]
  // 新建会话是显式用户意图：始终切换到新标签。
  return { ...state, sessions, activeId: info.sessionId }
}

/** pty_list 恢复：替换整个标签列表（Rust 侧存活的会话为准），保留本地缓冲。 */
export function terminalSessionsLoaded(state: TerminalState, sessions: PtySessionInfo[]): TerminalState {
  let next: TerminalState = { ...state, sessions: [], activeId: null }
  for (const info of sessions) {
    const kept = findSession(state, info.sessionId)
    next = {
      ...next,
      sessions: [
        ...next.sessions,
        { info, buffer: kept?.buffer ?? new Uint8Array(0), exitCode: kept?.exitCode ?? null },
      ],
    }
  }
  const activeId =
    state.activeId !== null && sessions.some(info => info.sessionId === state.activeId)
      ? state.activeId
      : (sessions[0]?.sessionId ?? null)
  return { ...next, activeId }
}

export function terminalSessionOpened(state: TerminalState, info: PtySessionInfo): TerminalState {
  return upsertSession(state, info)
}

export function terminalSessionClosed(state: TerminalState, sessionId: string): TerminalState {
  if (findSession(state, sessionId) === null) return state
  const sessions = state.sessions.filter(session => session.info.sessionId !== sessionId)
  const activeId =
    state.activeId === sessionId ? (sessions[0]?.info.sessionId ?? null) : state.activeId
  return { ...state, sessions, activeId }
}

/** pty:data → 追加到会话缓冲；未知会话忽略。 */
export function terminalDataAppended(state: TerminalState, sessionId: string, bytes: Uint8Array): TerminalState {
  const session = findSession(state, sessionId)
  if (!session || bytes.length === 0) return state
  const merged = clampBuffer(new Uint8Array([...session.buffer, ...bytes]))
  return {
    ...state,
    sessions: state.sessions.map(item =>
      item.info.sessionId === sessionId ? { ...item, buffer: merged } : item,
    ),
  }
}

/** pty_replay → 替换会话缓冲；未知会话忽略。 */
export function terminalReplayReplaced(state: TerminalState, sessionId: string, bytes: Uint8Array): TerminalState {
  const session = findSession(state, sessionId)
  if (!session) return state
  const buffer = clampBuffer(bytes)
  if (buffer.length === session.buffer.length && buffer.every((b, i) => b === session.buffer[i])) {
    return state
  }
  return {
    ...state,
    sessions: state.sessions.map(item =>
      item.info.sessionId === sessionId ? { ...item, buffer } : item,
    ),
  }
}

/** pty:state → 迁移会话状态；未知会话忽略。 */
export function terminalStateMigrated(state: TerminalState, sessionId: string, ptyState: PtySessionState): TerminalState {
  const session = findSession(state, sessionId)
  if (!session || session.info.state === ptyState) return state
  return {
    ...state,
    sessions: state.sessions.map(item =>
      item.info.sessionId === sessionId
        ? { ...item, info: { ...item.info, state: ptyState } }
        : item,
    ),
  }
}

/** pty:exit → 记录退出码并置 closed；未知会话忽略。 */
export function terminalExited(state: TerminalState, sessionId: string, code: number | null): TerminalState {
  const session = findSession(state, sessionId)
  if (!session) return state
  return {
    ...state,
    sessions: state.sessions.map(item =>
      item.info.sessionId === sessionId
        ? {
            ...item,
            exitCode: code,
            info: { ...item.info, state: 'closed' satisfies PtySessionState },
          }
        : item,
    ),
  }
}

export function terminalActiveChanged(state: TerminalState, sessionId: string | null): TerminalState {
  if (state.activeId === sessionId) return state
  if (sessionId !== null && findSession(state, sessionId) === null) return state
  return { ...state, activeId: sessionId }
}

/** 应用一条 pty 事件；未知会话的帧原样返回，便于多控制器共用事件流。 */
export function applyPtyEvent(state: TerminalState, event: PtyEvent): TerminalState {
  switch (event.type) {
    case 'pty:data':
      return terminalDataAppended(state, event.sessionId, base64ToBytes(event.data) ?? new Uint8Array(0))
    case 'pty:state':
      return terminalStateMigrated(state, event.sessionId, event.state)
    case 'pty:exit':
      return terminalExited(state, event.sessionId, event.code)
  }
}

// ==== 控制器：命令 + 事件流 → store ====

/** 命令返回归一为 BridgeResult 形状（与 bridge.ts 一致的 error 信封）。 */
export type PtyCallResult<T> =
  | { ok: true; data: T }
  | { ok: false; error: ErrorBody }

function internalError(message: string): ErrorBody {
  return { code: 'internal' satisfies ErrorCode, message, detail: null }
}

export interface TerminalControllerDeps {
  ptyOpen: (request: PtyOpenRequest) => Promise<PtyCallResult<PtyOpenResult>>
  ptyWrite: (sessionId: string, data: string) => Promise<PtyCallResult<null>>
  ptyResize: (sessionId: string, cols: number, rows: number) => Promise<PtyCallResult<null>>
  ptyClose: (sessionId: string) => Promise<PtyCallResult<null>>
  ptyList: () => Promise<PtyCallResult<PtyListResult>>
  ptyReplay: (sessionId: string) => Promise<PtyCallResult<PtyReplayResult>>
  onPtyEvent: (handler: (event: PtyEvent) => void) => () => void
}

export interface TerminalController {
  getState(): TerminalState
  subscribe(listener: (state: TerminalState) => void): () => void
  /** 面板挂载时恢复 Rust 侧存活的会话标签。 */
  restore(): Promise<boolean>
  openLocal(cols: number, rows: number, title?: string): Promise<string | null>
  openSsh(draft: SshFormDraft, cols: number, rows: number, title?: string): Promise<string | null>
  write(sessionId: string, text: string): Promise<boolean>
  resize(sessionId: string, cols: number, rows: number): Promise<boolean>
  /** 会话最近输出（base64 解码后字节）；失败返回 null。 */
  replay(sessionId: string): Promise<Uint8Array | null>
  /** 用户关闭标签：仅此路径真正 pty_close。 */
  close(sessionId: string): Promise<boolean>
  activate(sessionId: string | null): void
  /** 面板标题回填（pty:state 附带或 Rust list 提供 title 变化时）。 */
  rename(sessionId: string, title: string): void
  dispose(): void
}

export function createTerminalController(deps: TerminalControllerDeps): TerminalController {
  let state = createTerminalState()
  const listeners = new Set<(state: TerminalState) => void>()
  let disposed = false

  function commit(next: TerminalState): void {
    state = next
    if (disposed) return
    for (const listener of [...listeners]) listener(state)
  }

  const offPty = deps.onPtyEvent(event => {
    if (disposed) return
    commit(applyPtyEvent(state, event))
  })

  async function restore(): Promise<boolean> {
    if (disposed) return false
    const res = await deps.ptyList()
    if (disposed) return false
    if (!res.ok) return false
    if (!isPtyListResult(res.data)) return false
    commit(terminalSessionsLoaded(state, res.data.sessions))
    return true
  }

  async function openLocal(cols: number, rows: number, title = 'local'): Promise<string | null> {
    if (disposed) return null
    const request: PtyOpenLocal = { kind: 'local', cols, rows }
    const res = await deps.ptyOpen(request)
    if (disposed || !res.ok) return null
    if (!isPtyOpenResult(res.data)) return null
    commit(
      terminalSessionOpened(state, {
        sessionId: res.data.sessionId,
        kind: 'local',
        state: 'connected',
        title,
      }),
    )
    return res.data.sessionId
  }

  async function openSsh(
    draft: SshFormDraft,
    cols: number,
    rows: number,
    title = 'ssh',
  ): Promise<string | null> {
    const request = buildSshRequest(draft, cols, rows)
    if (request === null || disposed) return null
    const res = await deps.ptyOpen(request)
    if (disposed || !res.ok) return null
    if (!isPtyOpenResult(res.data)) return null
    commit(
      terminalSessionOpened(state, {
        sessionId: res.data.sessionId,
        kind: 'ssh',
        state: 'connected',
        title,
      }),
    )
    return res.data.sessionId
  }

  async function write(sessionId: string, text: string): Promise<boolean> {
    if (disposed) return false
    const res = await deps.ptyWrite(sessionId, utf8ToBase64(text))
    return res.ok
  }

  async function resize(sessionId: string, cols: number, rows: number): Promise<boolean> {
    if (disposed) return false
    const res = await deps.ptyResize(sessionId, cols, rows)
    return res.ok
  }

  async function replay(sessionId: string): Promise<Uint8Array | null> {
    if (disposed) return null
    const res = await deps.ptyReplay(sessionId)
    if (disposed || !res.ok) return null
    if (!isPtyReplayResult(res.data)) return null
    const bytes = base64ToBytes(res.data.data)
    if (bytes === null) return null
    commit(terminalReplayReplaced(state, sessionId, bytes))
    return bytes
  }

  async function close(sessionId: string): Promise<boolean> {
    if (disposed) return false
    const res = await deps.ptyClose(sessionId)
    if (!res.ok) return false
    commit(terminalSessionClosed(state, sessionId))
    return true
  }

  function activate(sessionId: string | null): void {
    if (disposed) return
    commit(terminalActiveChanged(state, sessionId))
  }

  function rename(sessionId: string, title: string): void {
    if (disposed) return
    const session = findSession(state, sessionId)
    if (!session || session.info.title === title) return
    commit({
      ...state,
      sessions: state.sessions.map(item =>
        item.info.sessionId === sessionId ? { ...item, info: { ...item.info, title } } : item,
      ),
    })
  }

  function getState(): TerminalState {
    return state
  }

  function subscribe(listener: (state: TerminalState) => void): () => void {
    listeners.add(listener)
    return () => {
      listeners.delete(listener)
    }
  }

  function dispose(): void {
    if (disposed) return
    disposed = true
    offPty()
    listeners.clear()
  }

  return {
    getState,
    subscribe,
    restore,
    openLocal,
    openSsh,
    write,
    resize,
    replay,
    close,
    activate,
    rename,
    dispose,
  }
}

// ==== Tauri 命令/事件通道封装（复用 bridge.ts 的底层入口模式） ====

/** Tauri webview 注入的 IPC 内部入口（@tauri-apps/api 的 invoke 底层）。 */
interface TauriInternals {
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>
  transformCallback?: (callback: (response: unknown) => void, once?: boolean) => number
}

function internals(): TauriInternals | null {
  if (typeof window === 'undefined') return null
  const candidate = (window as unknown as { __TAURI_INTERNALS__?: TauriInternals }).__TAURI_INTERNALS__
  return typeof candidate?.invoke === 'function' ? candidate : null
}

function asErrorBody(raw: unknown): ErrorBody {
  if (typeof raw === 'object' && raw !== null) {
    const r = raw as { code?: unknown; message?: unknown; detail?: unknown }
    if (typeof r.code === 'string' && typeof r.message === 'string') {
      return {
        code: r.code as ErrorBody['code'],
        message: r.message,
        detail: typeof r.detail === 'string' ? r.detail : null,
      }
    }
  }
  return internalError(String(raw))
}

async function invokePty<T>(command: string, args: Record<string, unknown>): Promise<PtyCallResult<T>> {
  const tauri = internals()
  if (!tauri) {
    return { ok: false, error: internalError(`${command} 需要 Tauri 运行时`) }
  }
  try {
    return { ok: true, data: await tauri.invoke<T>(command, args) }
  } catch (error) {
    return { ok: false, error: asErrorBody(error) }
  }
}

export async function ptyOpen(request: PtyOpenRequest): Promise<PtyCallResult<PtyOpenResult>> {
  return invokePty<PtyOpenResult>('pty_open', { request })
}

export async function ptyWrite(sessionId: string, data: string): Promise<PtyCallResult<null>> {
  return invokePty<null>('pty_write', { sessionId, data })
}

export async function ptyResize(sessionId: string, cols: number, rows: number): Promise<PtyCallResult<null>> {
  return invokePty<null>('pty_resize', { sessionId, cols, rows })
}

export async function ptyClose(sessionId: string): Promise<PtyCallResult<null>> {
  return invokePty<null>('pty_close', { sessionId })
}

export async function ptyList(): Promise<PtyCallResult<PtyListResult>> {
  return invokePty<PtyListResult>('pty_list', {})
}

export async function ptyReplay(sessionId: string): Promise<PtyCallResult<PtyReplayResult>> {
  // 契约面统一为 `{ data }`；Rust 实现曾直接返回 base64 字符串（裸值），
  // 两种形状都接受，避免双端漂移导致回放静默失败。
  const res = await invokePty<unknown>('pty_replay', { sessionId })
  if (!res.ok) return res
  if (typeof res.data === 'string') {
    return { ok: true, data: { data: res.data } }
  }
  if (isPtyReplayResult(res.data)) {
    return { ok: true, data: res.data }
  }
  return { ok: false, error: internalError('pty_replay 返回了非预期载荷') }
}

/**
 * pty 事件通道名（Rust 侧 app.emit 与此一致）。与 core:event 不同，pty:* 是
 * 独立通道（与 lsp:diagnostics 相同的低层 listen 模式），载荷不带契约信封。
 */
export const PTY_DATA_CHANNEL = 'pty:data'
export const PTY_STATE_CHANNEL = 'pty:state'
export const PTY_EXIT_CHANNEL = 'pty:exit'

interface TauriEventInternals {
  unregisterListener?: (event: string, eventId: number) => void
}

function unregisterEventCallback(event: string, eventId: number): void {
  if (typeof window === 'undefined') return
  const plugin = (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__?: TauriEventInternals })
    .__TAURI_EVENT_PLUGIN_INTERNALS__
  plugin?.unregisterListener?.(event, eventId)
}

/** 订阅单个 pty 事件通道；非 Tauri 环境返回 no-op（与 onLspDiagnostics 同构）。 */
function onPtyChannel(channel: string, handler: (payload: unknown) => void): () => void {
  const tauri = internals()
  const transform = tauri?.transformCallback
  if (!tauri || typeof transform !== 'function') return () => {}

  let active = true
  let unlisten: (() => void) | null = null

  const callbackId = transform((raw: unknown) => {
    if (!active) return
    // Tauri event 回调载荷是 { event, payload, id }；裸载荷兼容直发。
    const payload = isRecord(raw) && 'payload' in raw ? raw.payload : raw
    handler(payload)
  })

  void tauri
    .invoke<number>('plugin:event|listen', {
      event: channel,
      target: { kind: 'Any' },
      handler: callbackId,
    })
    .then(eventId => {
      const stop = (): void => {
        unregisterEventCallback(channel, eventId)
        void tauri
          .invoke('plugin:event|unlisten', { event: channel, eventId })
          .catch(() => {})
      }
      if (active) unlisten = stop
      else stop()
    })
    .catch(() => {
      // 监听注册失败只影响实时输出；replay 仍可恢复最近缓冲。
    })

  return () => {
    active = false
    if (unlisten) {
      unlisten()
      unlisten = null
    }
  }
}

/**
 * 事件载荷归一：Rust 侧三通道的负载是 `{ sessionId, ... }`（不带 `type`，
 * 通道即是类型）；守卫与 reducer 依赖 `type` 判别。这里按订阅通道注入，
 * 保留对已带 `type` 载荷的兼容（旧实现/直发场景）。
 */
function withChannelType(channel: string, raw: unknown): unknown {
  if (!isRecord(raw) || raw.type !== undefined) return raw
  return { ...raw, type: channel }
}

/** 订阅全部 pty 事件流；载荷过 DTO 守卫，畸形帧直接丢弃。 */
export function onPtyEvent(handler: (event: PtyEvent) => void): () => void {
  const offs = [
    onPtyChannel(PTY_DATA_CHANNEL, raw => {
      const event = withChannelType('pty:data', raw)
      if (isPtyEvent(event) && event.type === 'pty:data') handler(event)
    }),
    onPtyChannel(PTY_STATE_CHANNEL, raw => {
      const event = withChannelType('pty:state', raw)
      if (isPtyEvent(event) && event.type === 'pty:state') handler(event)
    }),
    onPtyChannel(PTY_EXIT_CHANNEL, raw => {
      const event = withChannelType('pty:exit', raw)
      if (isPtyEvent(event) && event.type === 'pty:exit') handler(event)
    }),
  ]
  return () => {
    for (const off of offs) off()
  }
}

/** 与 bridge.ts isTauri 同语义的轻量检查（组件模板用，避免引 bridge 模块图）。 */
export function isTauriLike(): boolean {
  if (typeof window === 'undefined') return false
  const candidate = (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
  return typeof candidate === 'object' && candidate !== null
}

/** App.vue 组装真实控制器的默认依赖。 */
export function defaultTerminalDeps(): TerminalControllerDeps {
  return {
    ptyOpen,
    ptyWrite,
    ptyResize,
    ptyClose,
    ptyList,
    ptyReplay,
    onPtyEvent,
  }
}
