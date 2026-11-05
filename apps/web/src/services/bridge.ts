// Tauri IPC 桥接：isTauri 检测 + invoke 包装（P1 bridge lane）。
//
// services 层禁止 import 框架 API（UI.md §8；eslint no-restricted-imports 把
// `@tauri-apps/*` 与 vue* 一并挡在 services 外）。因此 invoke 走 Tauri 注入的
// `window.__TAURI_INTERNALS__.invoke` 底层入口——@tauri-apps/api/core 的 invoke
// 正是它的一层薄封装，语义一致且不引入框架依赖。
//
// 非 Tauri 环境（浏览器 dev）返回**占位数据**：形状与契约一致，
// `fallback: true` 显式标注，UI 必须当作演示数据而非真实文件系统。
// 命令侧错误：Rust 命令返回 `Result<CommandResult, ErrorBody>`（契约信封），
// invoke 的 reject 载荷即序列化的 `ErrorBody`，这里归一为 BridgeResult 的 error 分支。

import type {
  CommandResult,
  DirListingResult,
  ErrorCode,
  ErrorBody,
  FileContentResult,
  NodeStatusResult,
} from './contract'

/** Tauri webview 注入的 IPC 内部入口（@tauri-apps/api 的 invoke 底层）。 */
interface TauriInternals {
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>
}

function internals(): TauriInternals | null {
  if (typeof window === 'undefined') return null
  const candidate = (window as unknown as { __TAURI_INTERNALS__?: TauriInternals })
    .__TAURI_INTERNALS__
  return typeof candidate?.invoke === 'function' ? candidate : null
}

/** true = 运行在 Tauri webview 内；false = 浏览器 dev，桥接函数返回占位数据。 */
export function isTauri(): boolean {
  return internals() !== null
}

/** 桥接结果。ok 分支携带契约数据；error 分支携带契约 ErrorBody（错误码供 tooltip/日志，UI.md §4）。 */
export type BridgeResult<T> =
  | { ok: true; data: T; fallback: boolean }
  | { ok: false; error: ErrorBody; fallback: false }

function internalError(message: string): ErrorBody {
  return { code: 'internal', message, detail: null }
}

/** reject 载荷 → ErrorBody：形状不符（非本应用命令的错误）时归入 internal。 */
function asErrorBody(raw: unknown): ErrorBody {
  if (typeof raw === 'object' && raw !== null) {
    const r = raw as { code?: unknown; message?: unknown; detail?: unknown }
    if (typeof r.code === 'string' && typeof r.message === 'string') {
      return {
        code: r.code as ErrorCode,
        message: r.message,
        detail: typeof r.detail === 'string' ? r.detail : null,
      }
    }
  }
  return internalError(String(raw))
}

/** invoke → CommandResult，附带轻量形状校验（跨 IPC 边界不盲信类型）。 */
async function invokeContract(command: string, args: Record<string, unknown>): Promise<CommandResult> {
  const tauri = internals()
  if (!tauri) throw new Error(`invokeContract(${command}) 只应在 Tauri webview 内调用`)
  const raw: unknown = await tauri.invoke(command, args)
  if (typeof raw === 'object' && raw !== null && 'kind' in raw) {
    return raw as CommandResult
  }
  throw new Error(`fs 命令 ${command} 返回了非契约载荷`)
}

// ==== 命令包装：签名与契约 CoreCommand 对应；root 由传输层注入（见 src-tauri fs_read_dir） ====

export async function readDir(root: string, path: string): Promise<BridgeResult<DirListingResult>> {
  if (!isTauri()) {
    return { ok: true, data: placeholderListing(root, path), fallback: true }
  }
  const res = await call('fs_read_dir', { root, path })
  if (!res.ok) return res
  if (res.data.kind !== 'dir_listing') {
    return { ok: false, error: internalError(`fs_read_dir 返回 kind=${String(res.data.kind)}`), fallback: false }
  }
  return { ok: true, data: res.data, fallback: false }
}

export async function readFile(root: string, path: string): Promise<BridgeResult<FileContentResult>> {
  if (!isTauri()) {
    return { ok: true, data: placeholderFile(root, path), fallback: true }
  }
  const res = await call('fs_read_file', { root, path })
  if (!res.ok) return res
  if (res.data.kind !== 'file_content') {
    return { ok: false, error: internalError(`fs_read_file 返回 kind=${String(res.data.kind)}`), fallback: false }
  }
  return { ok: true, data: res.data, fallback: false }
}

/** Node 探测（惰性触发由前端控制，docs/RUNTIME.md）。 */
export async function detectNode(): Promise<BridgeResult<NodeStatusResult>> {
  if (!isTauri()) {
    return { ok: true, data: { kind: 'node_status', status: { kind: 'need_manual', attempts: [] } }, fallback: true }
  }
  const res = await call('node_detect', {})
  if (!res.ok) return res
  if (res.data.kind !== 'node_status') {
    return { ok: false, error: internalError(`node_detect 返回 kind=${String(res.data.kind)}`), fallback: false }
  }
  return { ok: true, data: res.data, fallback: false }
}

async function call(command: string, args: Record<string, unknown>): Promise<BridgeResult<CommandResult>> {
  try {
    return { ok: true, data: await invokeContract(command, args), fallback: false }
  } catch (raw) {
    return { ok: false, error: asErrorBody(raw), fallback: false }
  }
}

// ==== 浏览器 dev 占位数据（fallback === true 时返回） ====
// 形状与契约 DirListingResult / FileContentResult 一致；键为 root 相对伪路径。
const PLACEHOLDER_TREE: Record<string, DirListingResult['entries']> = {
  '': [
    { name: 'docs', path: 'docs', kind: 'dir', size: null },
    { name: 'src', path: 'src', kind: 'dir', size: null },
    { name: 'README.md', path: 'README.md', kind: 'file', size: 1024 },
    { name: 'main.rs', path: 'main.rs', kind: 'file', size: 512 },
  ],
  docs: [
    { name: 'ARCHITECTURE.md', path: 'docs/ARCHITECTURE.md', kind: 'file', size: 8192 },
    { name: 'UI.md', path: 'docs/UI.md', kind: 'file', size: 4096 },
  ],
  src: [
    { name: 'lib.rs', path: 'src/lib.rs', kind: 'file', size: 256 },
    { name: 'main.rs', path: 'src/main.rs', kind: 'file', size: 512 },
  ],
}

/** 占位路径归一：剥掉 root 前缀得到伪路径键（浏览器模式无真实 fs，root 仅用于展示）。 */
function placeholderKey(root: string, path: string): string {
  if (root.length > 0 && path.startsWith(root)) {
    return path.slice(root.length).replace(/^[/\\]/, '')
  }
  return path
}

function placeholderListing(root: string, path: string): DirListingResult {
  return {
    kind: 'dir_listing',
    path,
    entries: PLACEHOLDER_TREE[placeholderKey(root, path)] ?? [],
    generation: 0,
  }
}

function placeholderFile(root: string, path: string): FileContentResult {
  const display = placeholderKey(root, path)
  return {
    kind: 'file_content',
    path,
    text: `// 浏览器 dev 占位内容（fallback）：${display}\n// 真实文件内容需在 Tauri 环境经 fs_read_file 读取。\n`,
  }
}
