// 插件市场 IPC（docs/plans/plugin-marketplace.md §3.4）。
// DTO 由 Rust 侧以 camelCase 序列化；浏览器 dev 返回演示市场数据并标注 fallback，
// 安装/卸载在内存上演示（刷新搜索即可看到 installedVersion 变化）。
// 鸿蒙 shim 未转发市场命令：invoke reject 的 ErrorBody 原样进入 error 分支，
// 由 UI 显示「市场暂不可用于此平台」空态（同 extensions.ts 的 ohos 约定）。
import type { ErrorBody } from '@lc-proto'

import { isTauri, type BridgeResult } from './bridge'
import { asPluginList, type PluginInfo, type PluginList } from './extensions'

/** 签名状态徽标（§3.4）。`enforced` 为 M3 强制档预留，M1 只产出 signed/unsigned。 */
export type SignatureState = 'signed' | 'unsigned' | 'enforced'

/** 市场搜索结果条目（ext_search 的 results 元素，custom DTO camelCase）。 */
export interface MarketPlugin {
  id: string
  version: string
  description: string
  runtime: string
  /** engines 冗余字段（安装前 fail-closed 预检用；与 manifest 不一致时以 manifest 为准）。 */
  engines: Record<string, string>
  /** 依赖插件 id 列表（M1 只预检不自动安装）。 */
  deps: string[]
  /** 提示用权限列表（Node 插件本质是可信代码，§7）。 */
  permissions: string[]
  /** 命中该条目的源名（多源按 id 去重后 semver 最高的来源）。 */
  source: string
  /** dist.url 为 file:// 时 true——UI 须标「本地源，自行承担信任」。 */
  fileSource: boolean
  signatureState: SignatureState
  /** 本地已安装版本；未安装为 null。 */
  installedVersion: string | null
  /** 已安装且 index 版本严格大于本地版本。 */
  updateAvailable: boolean
}

/** ext_search 返回体：合并去重后的结果 + 暂时不可用的源名（§3.3）。 */
export interface MarketSearchResult {
  results: MarketPlugin[]
  staleSources: string[]
}

/** ext_registry_list 的 sources 元素（custom DTO camelCase）。 */
export interface RegistrySourceInfo {
  name: string
  url: string
  /** 桌面层注入的 builtin 源（不可删除；文件内同名条目可禁用它）。 */
  builtin: boolean
  requireSignatures: boolean
  /** pubkey 非空（UI 据此展示是否 pinned 公钥）。 */
  hasPubkey: boolean
}

export interface RegistryList {
  sources: RegistrySourceInfo[]
}

/** Tauri webview 注入的 IPC 内部入口（与 extensions.ts 的封装一致）。 */
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

function isSignatureState(value: unknown): value is SignatureState {
  return value === 'signed' || value === 'unsigned' || value === 'enforced'
}

function isStringRecord(value: unknown): value is Record<string, string> {
  return (
    isRecord(value) &&
    Object.keys(value).every(key => typeof key === 'string') &&
    Object.values(value).every(entry => typeof entry === 'string')
  )
}

function isMarketPlugin(value: unknown): value is MarketPlugin {
  if (!isRecord(value)) return false
  return (
    typeof value.id === 'string' &&
    typeof value.version === 'string' &&
    typeof value.description === 'string' &&
    typeof value.runtime === 'string' &&
    isStringRecord(value.engines) &&
    isStringArray(value.deps) &&
    isStringArray(value.permissions) &&
    typeof value.source === 'string' &&
    typeof value.fileSource === 'boolean' &&
    isSignatureState(value.signatureState) &&
    (value.installedVersion === null || typeof value.installedVersion === 'string') &&
    typeof value.updateAvailable === 'boolean'
  )
}

function asMarketSearchResult(raw: unknown): MarketSearchResult | null {
  if (!isRecord(raw)) return null
  if (!Array.isArray(raw.results) || !raw.results.every(isMarketPlugin)) return null
  if (!isStringArray(raw.staleSources)) return null
  return { results: raw.results, staleSources: raw.staleSources }
}

function isRegistrySourceInfo(value: unknown): value is RegistrySourceInfo {
  if (!isRecord(value)) return false
  return (
    typeof value.name === 'string' &&
    typeof value.url === 'string' &&
    typeof value.builtin === 'boolean' &&
    typeof value.requireSignatures === 'boolean' &&
    typeof value.hasPubkey === 'boolean'
  )
}

function asRegistryList(raw: unknown): RegistryList | null {
  if (!isRecord(raw)) return null
  if (!Array.isArray(raw.sources) || !raw.sources.every(isRegistrySourceInfo)) return null
  return { sources: raw.sources }
}

/** 返回 PluginList 的市场命令（install/update/uninstall）共用封装。 */
async function invokePluginListCommand(
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

// 演示安装态：id → version。安装/卸载在内存上生效，刷新搜索即可看到标记变化。
const demoInstalled = new Map<string, string>()

function demoMarketPlugins(): MarketPlugin[] {
  return [
    {
      id: 'demo.hello',
      version: '0.1.0',
      description: '演示 Node 插件：带一个示例命令与权限声明。',
      runtime: 'node',
      engines: { 'lite-code': '>=0.6.1' },
      deps: [],
      permissions: ['filesystem.read'],
      source: 'demo',
      fileSource: false,
      signatureState: 'signed',
      installedVersion: demoInstalled.get('demo.hello') ?? null,
      updateAvailable: false,
    },
    {
      id: 'demo.local',
      version: '0.2.0',
      description: '演示 file:// 本地源插件（未签名，徽标可见）。',
      runtime: 'declarative',
      engines: {},
      deps: [],
      permissions: [],
      source: 'demo-local',
      fileSource: true,
      signatureState: 'unsigned',
      installedVersion: demoInstalled.get('demo.local') ?? null,
      updateAvailable: false,
    },
  ]
}

function demoPluginList(): PluginList {
  const plugins: PluginInfo[] = [...demoInstalled].map(([id, version]) => ({
    id,
    name: id,
    version,
    description: '浏览器演示插件',
    runtime: 'node',
    enabled: true,
    path: `${DEMO_EXTENSIONS_DIR}/${id}`,
    error: null,
    commands: [],
    mcpServers: [],
    languageServers: [],
    themes: [],
    languages: [],
    views: [],
  }))
  return { plugins, dir: DEMO_EXTENSIONS_DIR, nodeAvailable: true }
}

// ==== 命令包装 ====

/** 列出全部插件源（builtin 在首位）；浏览器 dev 返回演示源。 */
export async function marketplaceListRegistries(): Promise<BridgeResult<RegistryList>> {
  if (!isTauri()) {
    return {
      ok: true,
      data: {
        sources: [
          {
            name: 'demo',
            url: 'https://plugins.example.com/index.json',
            builtin: true,
            requireSignatures: false,
            hasPubkey: false,
          },
          {
            name: 'demo-local',
            url: 'file:///tmp/demo-plugins/index.json',
            builtin: false,
            requireSignatures: false,
            hasPubkey: false,
          },
        ],
      },
      fallback: true,
    }
  }
  try {
    const data = asRegistryList(await tauriInvoke<unknown>('ext_registry_list'))
    if (!data) {
      return {
        ok: false,
        error: internalError('ext_registry_list 返回了非预期载荷'),
        fallback: false,
      }
    }
    return { ok: true, data, fallback: false }
  } catch (error) {
    return { ok: false, error: asErrorBody(error), fallback: false }
  }
}

/** 新增插件源。URL 走 Rust 侧白名单校验（https / 回环 http / file://）。 */
export async function marketplaceAddRegistry(
  name: string,
  url: string,
  options: { requireSignatures?: boolean; pubkey?: string } = {},
): Promise<BridgeResult<RegistryList>> {
  if (!isTauri()) {
    void name
    void url
    void options
    const res = await marketplaceListRegistries()
    if (!res.ok) return res
    return { ok: true, data: res.data, fallback: true }
  }
  try {
    const data = asRegistryList(
      await tauriInvoke<unknown>('ext_registry_add', {
        name,
        url,
        requireSignatures: options.requireSignatures ?? null,
        pubkey: options.pubkey ?? null,
      }),
    )
    if (!data) {
      return {
        ok: false,
        error: internalError('ext_registry_add 返回了非预期载荷'),
        fallback: false,
      }
    }
    return { ok: true, data, fallback: false }
  } catch (error) {
    return { ok: false, error: asErrorBody(error), fallback: false }
  }
}

/** 删除插件源；builtin 源由 Rust 侧拒绝。 */
export async function marketplaceRemoveRegistry(
  name: string,
): Promise<BridgeResult<RegistryList>> {
  if (!isTauri()) {
    void name
    const res = await marketplaceListRegistries()
    if (!res.ok) return res
    return { ok: true, data: res.data, fallback: true }
  }
  try {
    const data = asRegistryList(
      await tauriInvoke<unknown>('ext_registry_remove', { name }),
    )
    if (!data) {
      return {
        ok: false,
        error: internalError('ext_registry_remove 返回了非预期载荷'),
        fallback: false,
      }
    }
    return { ok: true, data, fallback: false }
  } catch (error) {
    return { ok: false, error: asErrorBody(error), fallback: false }
  }
}

/**
 * 搜索市场（多源合并、按 id 去重取 semver 最高）；`refresh=true` 跳过 5 分钟缓存。
 * 浏览器 dev 返回演示条目并按 query 本地过滤；ohos shim 未转发时按 reject 进 error 分支。
 */
export async function marketplaceSearch(
  query?: string,
  refresh?: boolean,
): Promise<BridgeResult<MarketSearchResult>> {
  if (!isTauri()) {
    const text = (query ?? '').trim().toLowerCase()
    const results = demoMarketPlugins().filter(
      plugin =>
        text === '' ||
        plugin.id.toLowerCase().includes(text) ||
        plugin.description.toLowerCase().includes(text),
    )
    return { ok: true, data: { results, staleSources: [] }, fallback: true }
  }
  try {
    const data = asMarketSearchResult(
      await tauriInvoke<unknown>('ext_search', {
        query: query ?? null,
        refresh: refresh ?? null,
      }),
    )
    if (!data) {
      return {
        ok: false,
        error: internalError('ext_search 返回了非预期载荷'),
        fallback: false,
      }
    }
    return { ok: true, data, fallback: false }
  } catch (error) {
    return { ok: false, error: asErrorBody(error), fallback: false }
  }
}

/** 安装市场插件。engines/deps 预检失败返回 invalid_argument，UI 确认后可 force 重试（§3.5）。 */
export async function marketplaceInstall(
  id: string,
  options: { version?: string; source?: string; force?: boolean } = {},
): Promise<BridgeResult<PluginList>> {
  if (!isTauri()) {
    demoInstalled.set(id, options.version ?? '0.0.0')
    return { ok: true, data: demoPluginList(), fallback: true }
  }
  return invokePluginListCommand('ext_install', {
    id,
    version: options.version ?? null,
    source: options.source ?? null,
    force: options.force ?? null,
  })
}

/** 更新插件；id 缺省 = 全部可更新项。保留 disabled 状态（§3.5）。 */
export async function marketplaceUpdate(
  id?: string,
  force?: boolean,
): Promise<BridgeResult<PluginList>> {
  if (!isTauri()) {
    void id
    void force
    return { ok: true, data: demoPluginList(), fallback: true }
  }
  return invokePluginListCommand('ext_update', {
    id: id ?? null,
    force: force ?? null,
  })
}

/** 卸载市场安装的插件（路径白名单 + disabled 清理由 Rust 侧保证）。 */
export async function marketplaceUninstall(
  id: string,
): Promise<BridgeResult<PluginList>> {
  if (!isTauri()) {
    demoInstalled.delete(id)
    return { ok: true, data: demoPluginList(), fallback: true }
  }
  return invokePluginListCommand('ext_uninstall', { id })
}
