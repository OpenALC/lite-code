// 插件管理 IPC（docs/EXTENSIONS.md §3）。
// DTO 由 Rust 侧以 camelCase 序列化；浏览器 dev 返回两个演示插件并标注 fallback。
import type { ErrorBody } from '@lc-proto'

import { isTauri, type AckResult, type BridgeResult } from './bridge'

export type PluginRuntime = 'node' | 'wasm' | 'declarative'

/** 主题基于的基色（docs/EXTENSIONS.md §1）。 */
export type PluginUiTheme = 'dark' | 'light'

export interface PluginCommand {
  command: string
  title: string
}

/** 插件贡献的配色主题（ext_list 携带，JSON 经 ext_read_theme 读取）。 */
export interface PluginTheme {
  label: string
  path: string
  uiTheme: PluginUiTheme
}

/** 插件贡献的语言（ext_list 携带，语法 JSON 经 ext_read_grammar 读取）。 */
export interface PluginLanguage {
  /** Monaco language id（小写字母/数字/线，插件间由 Rust 校验唯一性）。 */
  id: string
  /** 带点扩展名（Rust 侧已转小写、插件间去重）。 */
  extensions: string[]
  aliases: string[]
  /** 清单声明的语法 JSON 相对路径；无声明为 null。 */
  grammar: string | null
}

/** 插件贡献的树视图（ext_list 携带，节点经 ext_view_children 拉取）。 */
export interface PluginView {
  /** 插件内唯一（小写字母/数字/线）。 */
  id: string
  /** 侧栏分区展示名（非空）。 */
  name: string
}

/** 树视图节点（docs/EXTENSIONS.md §4）：id/label 必填，其余可空。 */
export interface ViewNode {
  id: string
  label: string
  icon?: string
  command?: string
  tooltip?: string
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
  themes: PluginTheme[]
  languages: PluginLanguage[]
  views: PluginView[]
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

function isPluginUiTheme(value: unknown): value is PluginUiTheme {
  return value === 'dark' || value === 'light'
}

function isPluginTheme(value: unknown): value is PluginTheme {
  return (
    isRecord(value) &&
    typeof value.label === 'string' &&
    typeof value.path === 'string' &&
    isPluginUiTheme(value.uiTheme)
  )
}

function isPluginLanguage(value: unknown): value is PluginLanguage {
  return (
    isRecord(value) &&
    typeof value.id === 'string' &&
    isStringArray(value.extensions) &&
    isStringArray(value.aliases) &&
    (value.grammar === null || typeof value.grammar === 'string')
  )
}

function isPluginView(value: unknown): value is PluginView {
  return isRecord(value) && typeof value.id === 'string' && typeof value.name === 'string'
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
    isStringArray(value.languageServers) &&
    Array.isArray(value.themes) &&
    value.themes.every(isPluginTheme) &&
    Array.isArray(value.languages) &&
    value.languages.every(isPluginLanguage) &&
    Array.isArray(value.views) &&
    value.views.every(isPluginView)
  )
}

/** PluginList 载荷校验；marketplace.ts 的 install/update/uninstall 复用。 */
export function asPluginList(raw: unknown): PluginList | null {
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
        themes: [
          { label: 'Demo Dusk', path: './themes/dusk.json', uiTheme: 'dark' },
        ],
        languages: [
          {
            id: 'demo-lang',
            extensions: ['.demo'],
            aliases: ['Demo'],
            grammar: './grammars/demo.json',
          },
        ],
        views: [{ id: 'demo-tree', name: 'Demo Tree' }],
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
        themes: [],
        languages: [],
        views: [],
      },
    ],
    dir: DEMO_EXTENSIONS_DIR,
    nodeAvailable: true,
  }
}

// 浏览器 dev 演示主题：形状与 docs/EXTENSIONS.md §1 的主题 JSON 一致。
const DEMO_THEME_FILES: Record<string, unknown> = {
  'demo.hello>./themes/dusk.json': {
    name: 'Demo Dusk',
    colors: {
      '--bg': '#101014',
      '--panel': '#181820',
      '--fg': '#e6e9ef',
      '--accent': '#8ab4ff',
    },
  },
}

// 浏览器 dev 演示语法：形状是 Monarch 子集 { tokenizer }（docs/EXTENSIONS.md §1）。
const DEMO_GRAMMAR_FILES: Record<string, unknown> = {
  'demo.hello>./grammars/demo.json': {
    tokenizer: {
      root: [[/\w+/, 'keyword']],
    },
  },
}

// 浏览器 dev 演示树视图节点：键为 `${pluginId}>${viewId}>${nodeId ?? ''}`，
// 未声明节点一律视为空数组（叶子）。
const DEMO_VIEW_NODES: Record<string, unknown[]> = {
  'demo.hello>demo-tree>': [
    { id: 'group', label: 'Group', icon: 'folder', tooltip: '演示分组' },
    { id: 'run', label: 'Run hello', command: 'demo.hello', tooltip: '执行演示命令' },
  ],
  'demo.hello>demo-tree>group': [{ id: 'leaf', label: 'Leaf item' }],
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

export interface ExtThemeResult {
  json: unknown
}

/**
 * 读取插件贡献的主题文件（docs/EXTENSIONS.md §3）。Rust 侧只允许插件目录内
 * 且清单声明过的 path；浏览器 dev 返回演示主题 JSON，ohos shim 未转发该
 * 命令时按 reject 的 ErrorBody 进入 error 分支（同步逻辑会跳过该主题）。
 */
export async function extensionsReadTheme(
  pluginId: string,
  path: string,
): Promise<BridgeResult<ExtThemeResult>> {
  if (!isTauri()) {
    const json = DEMO_THEME_FILES[`${pluginId}>${path}`]
    if (json === undefined) {
      return {
        ok: false,
        error: internalError(`演示插件 ${pluginId} 未声明主题 ${path}`),
        fallback: false,
      }
    }
    return { ok: true, data: { json }, fallback: true }
  }
  try {
    const raw = await tauriInvoke<unknown>('ext_read_theme', { pluginId, path })
    if (!isRecord(raw) || !('json' in raw)) {
      return {
        ok: false,
        error: internalError('ext_read_theme 返回了非预期载荷'),
        fallback: false,
      }
    }
    return { ok: true, data: { json: raw.json }, fallback: false }
  } catch (error) {
    return { ok: false, error: asErrorBody(error), fallback: false }
  }
}

export interface ExtGrammarResult {
  json: unknown
}

/**
 * 读取插件贡献的语言语法文件（docs/EXTENSIONS.md §3）。Rust 侧只允许插件
 * 目录内且清单 `contributes.languages` 声明过的 grammar path；浏览器 dev
 * 返回演示语法 JSON；ohos shim 未转发该命令时按 reject 的 ErrorBody 进入
 * error 分支（同步逻辑会清掉该语言的高亮并保留语言注册）。
 */
export async function extensionsReadGrammar(
  pluginId: string,
  path: string,
): Promise<BridgeResult<ExtGrammarResult>> {
  if (!isTauri()) {
    const json = DEMO_GRAMMAR_FILES[`${pluginId}>${path}`]
    if (json === undefined) {
      return {
        ok: false,
        error: internalError(`演示插件 ${pluginId} 未声明语法 ${path}`),
        fallback: false,
      }
    }
    return { ok: true, data: { json }, fallback: true }
  }
  try {
    const raw = await tauriInvoke<unknown>('ext_read_grammar', { pluginId, path })
    if (!isRecord(raw) || !('json' in raw)) {
      return {
        ok: false,
        error: internalError('ext_read_grammar 返回了非预期载荷'),
        fallback: false,
      }
    }
    return { ok: true, data: { json: raw.json }, fallback: false }
  } catch (error) {
    return { ok: false, error: asErrorBody(error), fallback: false }
  }
}

export interface ExtViewChildrenResult {
  /** 子节点数组（可空）；形状由 pluginViews.ts 白名单归一。 */
  nodes: unknown[]
}

/**
 * 读取插件树视图的子节点（docs/EXTENSIONS.md §3/§4）。Rust 侧校验视图已
 * 声明且插件启用（node runtime），经扩展宿主转发 view/children；浏览器 dev
 * 返回演示节点；ohos shim 未转发该命令时按 reject 的 ErrorBody 进入 error
 * 分支（分区显示错误态，可重试）。
 */
export async function extensionsViewChildren(
  pluginId: string,
  viewId: string,
  nodeId?: string,
): Promise<BridgeResult<ExtViewChildrenResult>> {
  if (!isTauri()) {
    const nodes =
      DEMO_VIEW_NODES[`${pluginId}>${viewId}>${nodeId ?? ''}`] ??
      DEMO_VIEW_NODES[`${pluginId}>${viewId}>`]
    if (nodes === undefined) {
      return {
        ok: false,
        error: internalError(`演示插件 ${pluginId} 未声明视图 ${viewId}`),
        fallback: false,
      }
    }
    return { ok: true, data: { nodes: [...nodes] }, fallback: true }
  }
  try {
    const raw = await tauriInvoke<unknown>('ext_view_children', {
      pluginId,
      viewId,
      nodeId: nodeId ?? null,
    })
    if (!isRecord(raw) || !Array.isArray(raw.nodes)) {
      return {
        ok: false,
        error: internalError('ext_view_children 返回了非预期载荷'),
        fallback: false,
      }
    }
    return { ok: true, data: { nodes: raw.nodes }, fallback: false }
  } catch (error) {
    return { ok: false, error: asErrorBody(error), fallback: false }
  }
}
