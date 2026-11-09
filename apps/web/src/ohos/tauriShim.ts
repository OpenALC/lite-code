// 鸿蒙只读外壳的 Tauri IPC 伪装层。
// 目标：让 bridge.ts 的 isTauri() 分支原样工作——命令转发到 ArkTS 注入的
// window.__lcNative，原生事件回灌到 core:event；历史优先走原生落盘，
// 原生桥不可用（或旧宿主未实现该命令）时回退 localStorage。
import type { SessionMode } from '@lc-proto'

import type { HistorySummary } from '../services/bridge'
import type { LcNativeBridge } from './native'

const CORE_EVENT_CHANNEL = 'core:event'
const HISTORY_KEY = 'lite-code.ohos.history'

/** 首版原生命令面（docs/plans/ohos-shell.md §5.4）。其余命令直接拒绝。 */
const FORWARDED = new Set([
  'workspace_root',
  'fs_read_dir',
  'fs_read_file',
  'session_create',
  'agent_send',
  'agent_cancel',
  'approval_resolve',
  'providers_load',
  'providers_save',
  'history_list',
  'history_save',
  'history_load',
  'history_delete',
])

/** 原生宿主实现的 history 命令；其余 history_* 仍走 localStorage。 */
const NATIVE_HISTORY = new Set([
  'history_list',
  'history_save',
  'history_load',
  'history_delete',
])

interface ErrorBodyLike {
  code: string
  message: string
  detail: string | null
}

function errorBody(code: string, message: string): ErrorBodyLike {
  return { code, message, detail: null }
}

/** 原生错误可能是 ErrorBody 对象、JSON 字符串或普通字符串，统一成桥接信封。 */
function asErrorBody(raw: unknown): ErrorBodyLike {
  if (typeof raw === 'string') {
    try {
      return asErrorBody(JSON.parse(raw) as unknown)
    } catch {
      return errorBody('internal', raw)
    }
  }
  if (typeof raw === 'object' && raw !== null) {
    const record = raw as Record<string, unknown>
    if (typeof record.code === 'string' && typeof record.message === 'string') {
      return {
        code: record.code,
        message: record.message,
        detail: typeof record.detail === 'string' ? record.detail : null,
      }
    }
  }
  return errorBody('internal', String(raw))
}

// ==== Tauri 回调与事件插件形状 ====

const callbacks = new Map<number, (payload: unknown) => void>()
/** event 名 → (eventId → transformCallback id) */
const listeners = new Map<string, Map<number, number>>()
let nextCallbackId = 1
let nextEventId = 1

function transformCallback(callback: (response: unknown) => void, once = false): number {
  const id = nextCallbackId++
  callbacks.set(id, payload => {
    if (once) callbacks.delete(id)
    callback(payload)
  })
  return id
}

function listenEvent(args: Record<string, unknown>): number {
  const event = typeof args.event === 'string' ? args.event : ''
  const handler = typeof args.handler === 'number' ? args.handler : Number.NaN
  if (!event || !Number.isFinite(handler)) {
    throw errorBody('invalid_argument', 'plugin:event|listen 需要 event 与 handler')
  }
  const eventId = nextEventId++
  const handlers = listeners.get(event) ?? new Map<number, number>()
  handlers.set(eventId, handler)
  listeners.set(event, handlers)
  return eventId
}

function unlistenEvent(args: Record<string, unknown>): void {
  const event = typeof args.event === 'string' ? args.event : ''
  const eventId = typeof args.eventId === 'number' ? args.eventId : Number.NaN
  removeListener(event, eventId)
}

function removeListener(event: string, eventId: number): void {
  const handlers = listeners.get(event)
  const callbackId = handlers?.get(eventId)
  if (handlers === undefined || callbackId === undefined) return
  handlers.delete(eventId)
  callbacks.delete(callbackId)
}

/** lc-ohos → C++ tsfn → ArkTS → 这里；载荷是 EventEnvelope 的 JSON。 */
function emitEnvelope(envelopeJson: string): void {
  let payload: unknown
  try {
    payload = JSON.parse(envelopeJson) as unknown
  } catch {
    return
  }
  const handlers = listeners.get(CORE_EVENT_CHANNEL)
  if (!handlers) return
  for (const [eventId, callbackId] of handlers) {
    const callback = callbacks.get(callbackId)
    if (callback) callback({ event: CORE_EVENT_CHANNEL, id: eventId, payload })
  }
}

// ==== 历史记录（localStorage，形状对齐 bridge.ts 的 HistorySummary） ====

function readHistory(): Record<string, unknown> {
  try {
    const raw = localStorage.getItem(HISTORY_KEY)
    const parsed: unknown = raw ? (JSON.parse(raw) as unknown) : null
    return parsed !== null && typeof parsed === 'object'
      ? (parsed as Record<string, unknown>)
      : {}
  } catch {
    return {}
  }
}

function writeHistory(store: Record<string, unknown>): void {
  try {
    localStorage.setItem(HISTORY_KEY, JSON.stringify(store))
  } catch {
    // 配额/隐私模式失败不影响对话主流程。
  }
}

function excerptOf(agent: Record<string, unknown>): string {
  const messages = Array.isArray(agent.messages) ? agent.messages : []
  const parts: string[] = []
  for (const message of messages) {
    if (!message || typeof message !== 'object') continue
    const text = (message as Record<string, unknown>).text
    if (typeof text === 'string' && text.trim()) parts.push(text)
  }
  return parts.join('\n').slice(0, 4000)
}

function summaryOf(id: string, value: unknown): HistorySummary | null {
  if (!value || typeof value !== 'object') return null
  const item = value as Record<string, unknown>
  const agent = (
    item.agent && typeof item.agent === 'object' ? item.agent : {}
  ) as Record<string, unknown>
  return {
    id,
    title: typeof item.title === 'string' ? item.title : '',
    savedAt: Number(item.savedAt ?? 0),
    workspace: typeof item.workspace === 'string' ? item.workspace : '',
    mode: (item.mode ?? 'auto') as SessionMode,
    model: typeof item.model === 'string' ? item.model : '',
    turn: Number(agent.turn ?? 0),
    messages: Array.isArray(agent.messages) ? agent.messages.length : 0,
    archived: Boolean(item.archived),
    excerpt: excerptOf(agent),
  }
}

function historyLocalInvoke(cmd: string, args: Record<string, unknown>): unknown {
  const store = readHistory()
  switch (cmd) {
    case 'history_list':
      return Object.entries(store)
        .map(([id, value]) => summaryOf(id, value))
        .filter((item): item is HistorySummary => item !== null)
        .sort((a, b) => b.savedAt - a.savedAt)
    case 'history_load': {
      const value = store[String(args.id ?? '')]
      if (value === undefined) throw errorBody('not_found', '会话不存在')
      return value
    }
    case 'history_save':
      store[String(args.id ?? '')] = args.payload
      writeHistory(store)
      return null
    case 'history_delete':
      delete store[String(args.id ?? '')]
      writeHistory(store)
      return null
    case 'history_migrate_workspace': {
      const workspace = String(args.workspace ?? '')
      let moved = 0
      for (const value of Object.values(store)) {
        if (!value || typeof value !== 'object') continue
        const item = value as Record<string, unknown>
        if (typeof item.workspace === 'string' && item.workspace) continue
        item.workspace = workspace
        moved += 1
      }
      if (moved > 0) writeHistory(store)
      return moved
    }
    case 'history_rename': {
      const value = store[String(args.id ?? '')]
      if (!value || typeof value !== 'object') throw errorBody('not_found', '会话不存在')
      ;(value as Record<string, unknown>).title = String(args.title ?? '')
      writeHistory(store)
      return null
    }
    case 'history_archive': {
      const value = store[String(args.id ?? '')]
      if (!value || typeof value !== 'object') throw errorBody('not_found', '会话不存在')
      ;(value as Record<string, unknown>).archived = args.archived === true
      writeHistory(store)
      return null
    }
    default:
      throw errorBody('not_implemented', `unsupported command on ohos shell: ${cmd}`)
  }
}

/** 原生历史优先；没有原生桥或旧宿主不认识命令时才回退 localStorage。 */
async function historyInvoke<T>(
  cmd: string,
  args: Record<string, unknown>,
): Promise<T> {
  const native: LcNativeBridge | undefined = window.__lcNative
  if (NATIVE_HISTORY.has(cmd) && native && typeof native.invoke === 'function') {
    try {
      const raw = await native.invoke(cmd, JSON.stringify(args))
      return JSON.parse(raw) as T
    } catch (error) {
      const body = asErrorBody(error)
      if (body.code !== 'not_implemented') throw body
    }
  }
  return historyLocalInvoke(cmd, args) as T
}

// ==== invoke ====

async function invoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  const command = String(cmd)
  if (command === 'plugin:event|listen') return listenEvent(args) as T
  if (command === 'plugin:event|unlisten') {
    unlistenEvent(args)
    return null as T
  }
  if (command.startsWith('history_')) return historyInvoke<T>(command, args)
  if (!FORWARDED.has(command)) {
    throw errorBody('not_implemented', `unsupported command on ohos shell: ${command}`)
  }

  const native: LcNativeBridge | undefined = window.__lcNative
  if (!native || typeof native.invoke !== 'function') {
    throw errorBody('internal', 'window.__lcNative 不可用：NativeBridge 未注入')
  }
  try {
    const raw = await native.invoke(command, JSON.stringify(args))
    return JSON.parse(raw) as T
  } catch (error) {
    throw asErrorBody(error)
  }
}

// ==== 注入 ====

const target = window as unknown as {
  __TAURI_INTERNALS__?: unknown
  __TAURI_EVENT_PLUGIN_INTERNALS__?: unknown
  __LC_OHOS__?: boolean
  __lcOhosEmit?: (envelopeJson: string) => void
}

target.__LC_OHOS__ = true
target.__TAURI_INTERNALS__ = { invoke, transformCallback }
target.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: removeListener }
target.__lcOhosEmit = emitEnvelope
