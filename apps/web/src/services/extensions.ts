// 插件管理 IPC（docs/EXTENSIONS.md §3）。
// DTO 由 Rust 侧以 camelCase 序列化；浏览器 dev 返回两个演示插件并标注 fallback。
import type { ErrorBody } from '@lc-proto'

import { isTauri, type AckResult, type BridgeResult } from './bridge'

export type PluginRuntime = 'node' | 'wasm' | 'declarative'

export interface PluginCommand {
  command: string
  title: string
}

export interface PluginInfo {
  id: string
  name: string
  version: string
  description: string
  runtime: PluginRuntime
  enabled: boolean
  path: string
  error: string | null
  commands: PluginCommand[]
  mcpServers: string[]
  languageServers: string[]
}

export interface PluginList {
  plugins: PluginInfo[]
  dir: string
  nodeAvailable: boolean
}

export interface ExtCommandResult {
  result: unknown
}

/** Tauri webview 注入的 IPC 内部入口（与 bridge.ts 的 invoke 底层一致）。 */
interface TauriInternals {
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>
}

function tauriInvoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (typeof window === 'undefined') {
    return Promise.reject(new Error(`${command} 需要 Tauri 运行时`))
  }
  const tauri = (window as unknown as { __TAURI_INTERNALS__?: TauriInternals })
    .__TAURI_INTERNALS__
  if (!tauri || typeof tauri.invoke !== 'function') {
    return Promise.reject(new Error(`${command} 需要 Tauri 运行时`))
  }
  return tauri.invoke<T>(command, args)
}

function internalError(message: string): ErrorBody {
  return { code: 'internal', message, detail: null }
}

/** reject 载荷 → ErrorBody：形状不符时归入 internal（不盲信 IPC 对端）。 */
function asErrorBody(raw: unknown): ErrorBody {
  if (typeof raw === 'object' && raw !== null) {
    const candidate = raw as { code?: unknown; message?: unknown; detail?: unknown }
    if (typeof candidate.code === 'string' && typeof candidate.message === 'string') {
      return {
        code: candidate.code as ErrorBody['code'],
        message: candidate.message,
        detail: typeof candidate.detail === 'string' ? candidate.detail : null,
      }
    }
  }
  return internalError(String(raw))
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every(item => typeof item === 'string')
}

function isPluginRuntime(value: unknown): value is PluginRuntime {
  return value === 'node' || value === 'wasm' || value === 'declarative'
}

function isPluginCommand(value: unknown): value is PluginCommand {
  return (
    isRecord(value) &&
    typeof value.command === 'string' &&
    typeof value.title === 'string'
  )
}

function isPluginInfo(value: unknown): value is PluginInfo {
  if (!isRecord(value)) return false
  return (
    typeof value.id === 'string' &&
    typeof value.name === 'string' &&
    typeof value.version === 'string' &&
    typeof value.description === 'string' &&
    isPluginRuntime(value.runtime) &&
    typeof value.enabled === 'boolean' &&
    typeof value.path === 'string' &&
    (value.error === null || typeof value.error === 'string') &&
    Array.isArray(value.commands) &&
    value.commands.every(isPluginCommand) &&
    isStringArray(value.mcpServers) &&
    isStringArray(value.languageServers)
  )
}

function asPluginList(raw: unknown): PluginList | null {
  if (!isRecord(raw)) return null
  if (typeof raw.dir !== 'string' || typeof raw.nodeAvailable !== 'boolean') return null
  const plugins = raw.plugins
  if (!Array.isArray(plugins) || !plugins.every(isPluginInfo)) return null
  return { plugins, dir: raw.dir, nodeAvailable: raw.nodeAvailable }
}

async function invokePluginList(
  command: string,
  args: Record<string, unknown> = {},
): Promise<BridgeResult<PluginList>> {
  try {
    const data = asPluginList(await tauriInvoke<unknown>(command, args))
    if (!data) {
      return {
        ok: false,
        error: internalError(`${command} 返回了非预期载荷`),
        fallback: false,
      }
    }
    return { ok: true, data, fallback: false }
  } catch (error) {
    return { ok: false, error: asErrorBody(error), fallback: false }
  }
}

// ==== 浏览器 dev 演示数据 ====

const DEMO_EXTENSIONS_DIR = '~/.lite-code/extensions'
const demoDisabled = new Set<string>()

function demoPluginList(): PluginList {
  return {
    plugins: [
      {
        id: 'demo.hello',
        name: 'Hello Demo',
        version: '0.1.0',
        description: 'Demo Node plugin with a sample command.',
        runtime: 'node',
        enabled: !demoDisabled.has('demo.hello'),
        path: `${DEMO_EXTENSIONS_DIR}/demo.hello`,
        error: null,
        commands: [
          { command: 'demo.hello', title: 'Say hello' },
          { command: 'demo.hello.time', title: 'Show demo time' },
        ],
        mcpServers: [],
        languageServers: [],
      },
      {
        id: 'demo.notes',
        name: 'Notes Declarative',
        version: '0.2.1',
        description: 'Demo declarative plugin that contributes a command.',
        runtime: 'declarative',
        enabled: !demoDisabled.has('demo.notes'),
        path: `${DEMO_EXTENSIONS_DIR}/demo.notes`,
        error: null,
        commands: [{ command: 'demo.notes.open', title: 'Open demo notes' }],
        mcpServers: ['filesystem'],
        languageServers: [],
      },
    ],
    dir: DEMO_EXTENSIONS_DIR,
    nodeAvailable: true,
  }
}

// ==== 命令包装 ====

/** 列出已安装插件；浏览器 dev 返回两个演示插件。 */
export async function extensionsList(): Promise<BridgeResult<PluginList>> {
  if (!isTauri()) {
    return { ok: true, data: demoPluginList(), fallback: true }
  }
  return invokePluginList('ext_list')
}

/** 启用/禁用插件；返回更新后的完整列表。 */
export async function extensionsSetEnabled(
  id: string,
  enabled: boolean,
): Promise<BridgeResult<PluginList>> {
  if (!isTauri()) {
    if (enabled) demoDisabled.delete(id)
    else demoDisabled.add(id)
    return { ok: true, data: demoPluginList(), fallback: true }
  }
  return invokePluginList('ext_set_enabled', { id, enabled })
}

/** 让扩展宿主重新加载插件；返回更新后的完整列表。 */
export async function extensionsReload(): Promise<BridgeResult<PluginList>> {
  if (!isTauri()) {
    return { ok: true, data: demoPluginList(), fallback: true }
  }
  return invokePluginList('ext_reload')
}

/** 在系统文件管理器中打开扩展目录。 */
export async function extensionsOpenDir(): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) {
    return { ok: true, data: { kind: 'ack' }, fallback: true }
  }
  try {
    const raw = await tauriInvoke<unknown>('ext_open_dir')
    if (!isRecord(raw) || raw.kind !== 'ack') {
      return {
        ok: false,
        error: internalError('ext_open_dir 返回了非预期载荷'),
        fallback: false,
      }
    }
    return { ok: true, data: { kind: 'ack' }, fallback: false }
  } catch (error) {
    return { ok: false, error: asErrorBody(error), fallback: false }
  }
}

/** 执行插件贡献的命令；args 原样透传为 JSON。 */
export async function extensionsExecuteCommand(
  command: string,
  args?: unknown,
): Promise<BridgeResult<ExtCommandResult>> {
  if (!isTauri()) {
    return {
      ok: true,
      data: {
        result: {
          demo: true,
          executed: false,
          command,
          args: args ?? null,
        },
      },
      fallback: true,
    }
  }
  try {
    const raw = await tauriInvoke<unknown>('ext_execute_command', {
      command,
      args: args ?? null,
    })
    if (!isRecord(raw) || !('result' in raw)) {
      return {
        ok: false,
        error: internalError('ext_execute_command 返回了非预期载荷'),
        fallback: false,
      }
    }
    return { ok: true, data: { result: raw.result }, fallback: false }
  } catch (error) {
    return { ok: false, error: asErrorBody(error), fallback: false }
  }
}
