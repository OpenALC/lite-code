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
  ApprovalDecision,
  CommandResult,
  CoreEvent,
  ErrorBody,
  ErrorCode,
  GitCommit,
  GitFileStatus,
  GitStatus,
  LspCompletionItem,
  LspHover,
  LspLocation,
  SessionInfo,
  SessionMode,
} from '@lc-proto'

import {
  isDbConnectionInfo,
  isDbExecResult,
  isDbExplainResult,
  isDbQueryResult,
  isDbSchemaNode,
  type DbConnectionConfig,
  type DbConnectionInfo,
  type DbExecResult,
  type DbExplainResult,
  type DbQueryResult,
  type DbSchemaNode,
  type UiCoreEvent,
} from './dbTypes'
import {
  isChangeStatus,
  isFileChangeDto,
  isTaskCheck,
  isTaskInfo,
  isTaskStatus,
  isTaskStepState,
  type FileChangeDto,
  type TaskInfo,
} from './taskTypes'

/** 按 `kind` 窄化的契约结果类型（直接从 codegen 产物派生，不复制定义）。 */
export type DirListingResult = Extract<CommandResult, { kind: 'dir_listing' }>
export type FileContentResult = Extract<CommandResult, { kind: 'file_content' }>
export type NodeStatusResult = Extract<CommandResult, { kind: 'node_status' }>
export type WorkspaceRootResult = Extract<CommandResult, { kind: 'workspace_root' }>
export type SessionResult = Extract<CommandResult, { kind: 'session' }>
export type AckResult = Extract<CommandResult, { kind: 'ack' }>
export type GitStatusResult = Extract<CommandResult, { kind: 'git_status' }>
export type GitDiffResult = Extract<CommandResult, { kind: 'git_diff' }>
export type GitLogResult = Extract<CommandResult, { kind: 'git_log' }>

// Task 结果类型用 taskTypes 的镜像 DTO 表达：契约 d.ts 的 CommandResult
// 引用了尚未导出的 TaskInfo，直接 Extract 会退化为 any（见 taskTypes.ts 头注释）。
export interface TaskListResult {
  kind: 'tasks'
  tasks: TaskInfo[]
}
export interface TaskResult {
  kind: 'task'
  task: TaskInfo
}
export interface TaskDiffResult {
  kind: 'task_diff'
  task_id: string
  patch: string
  files: FileChangeDto[]
}

// 数据库命令结果（契约 v0.6）。本地镜像原因见 dbTypes.ts 头注释。
export interface DbListResult {
  kind: 'connections'
  connections: DbConnectionInfo[]
}
export interface DbConnectionResult {
  kind: 'connection'
  connection: DbConnectionInfo
}
export interface DbSchemaResult {
  kind: 'db_schema'
  connection_id: string
  nodes: DbSchemaNode[]
}
export interface DbQueryCommandResult {
  kind: 'db_query'
  result: DbQueryResult
}
export interface DbExecCommandResult {
  kind: 'db_exec'
  result: DbExecResult
}
export interface DbExplainCommandResult {
  kind: 'db_explain'
  result: DbExplainResult
}

// 工作区全文搜索（fs_search）。不是契约 CommandResult：搜索结果是 UI 专用载荷，
// 由薄壳直接返回、前端镜像同一形状（后端 `rename_all = "camelCase"`）。
export interface SearchMatch {
  line: number
  column: number
  length: number
  preview: string
  matchStart: number
}

export interface SearchFileResult {
  path: string
  matches: SearchMatch[]
}

export interface SearchResult {
  files: SearchFileResult[]
  totalMatches: number
  truncated: boolean
  scannedFiles: number
}

export interface SearchRequestOptions {
  caseSensitive?: boolean
  regex?: boolean
  maxResults?: number
}

// 供应商管理（providers_load / providers_save）。密钥只以 hasApiKey/末四位提示
// 回传；保存时空白 apiKey 表示保留旧值。
export type ProviderKind = 'openai-responses' | 'anthropic-messages'

export interface ProviderView {
  id: string
  kind: ProviderKind
  label: string
  baseUrl: string
  enabled: boolean
  apiKeyRef: string
  hasApiKey: boolean
  apiKeyHint: string | null
}

export interface ProviderModelConfig {
  id: string
  provider: string
  modelId: string
  thinking: string[]
  contextK: number
  compressAtK: number
  capabilities: string[]
}

export interface ProvidersView {
  providers: ProviderView[]
  models: ProviderModelConfig[]
  defaultModel: string | null
  defaultThinking: string | null
  subagentModel: string | null
  subagentThinking: string | null
}

export interface ProviderSaveInput {
  id: string
  kind: ProviderKind
  label: string
  baseUrl: string
  enabled: boolean
  /** 新输入的密钥；留空表示保留旧值 */
  apiKey?: string
  /** 环境变量引用（兼容旧配置） */
  apiKeyRef?: string
  clearApiKey?: boolean
}

export interface ProvidersSaveInput {
  providers: ProviderSaveInput[]
  models: ProviderModelConfig[]
  defaultModel: string | null
  defaultThinking: string | null
  subagentModel: string | null
  subagentThinking: string | null
}

/** Tauri webview 注入的 IPC 内部入口（@tauri-apps/api 的 invoke 底层）。 */
interface TauriInternals {
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>
  transformCallback?: (callback: (response: unknown) => void, once?: boolean) => number
}

interface TauriEventInternals {
  unregisterListener?: (event: string, eventId: number) => void
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

/** 承载前端的壳。`ohos` 由 tauriShim 注入的只读外壳标记识别。 */
export type ShellKind = 'tauri' | 'ohos' | 'browser'

export function shellKind(): ShellKind {
  if (typeof window === 'undefined') return 'browser'
  const marked = (window as unknown as { __LC_OHOS__?: boolean }).__LC_OHOS__
  if (marked === true) return 'ohos'
  return internals() !== null ? 'tauri' : 'browser'
}

/** 鸿蒙首版是「只读 + 对话」外壳：所有写操作入口必须关闭。 */
export function isReadOnly(): boolean {
  return shellKind() === 'ohos'
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
  throw new Error(`命令 ${command} 返回了非契约载荷`)
}

// ==== 命令包装：签名与契约 CoreCommand 对应；root 由传输层注入（见 src-tauri workspace_root） ====

/** 原生文件夹选择器；取消时返回当前根。浏览器 dev 返回 fallback。 */
export async function pickWorkspace(): Promise<BridgeResult<WorkspaceRootResult>> {
  if (!isTauri()) {
    return { ok: true, data: { kind: 'workspace_root', root: null }, fallback: true }
  }
  try {
    const data = await invokeContract('workspace_pick', {})
    if (data.kind !== 'workspace_root') {
      return { ok: false, error: internalError('workspace_pick 返回了非 workspace_root 载荷'), fallback: false }
    }
    return { ok: true, data, fallback: false }
  } catch (error) {
    return { ok: false, error: asErrorBody(error), fallback: false }
  }
}

/** 当前工作区根；浏览器 dev 无真实工作区，返回 null 由视图层回退。 */
export async function workspaceRoot(): Promise<BridgeResult<WorkspaceRootResult>> {
  if (!isTauri()) {
    return { ok: true, data: { kind: 'workspace_root', root: null }, fallback: true }
  }
  const res = await call('workspace_root', {})
  if (!res.ok) return res
  if (res.data.kind !== 'workspace_root') {
    return { ok: false, error: internalError(`workspace_root 返回 kind=${String(res.data.kind)}`), fallback: false }
  }
  return { ok: true, data: res.data, fallback: false }
}

/** 打开目录作为工作区根；Rust 侧校验存在且为目录，并持久化。 */
export async function openWorkspace(path: string): Promise<BridgeResult<WorkspaceRootResult>> {
  if (!isTauri()) {
    return { ok: true, data: { kind: 'workspace_root', root: path }, fallback: true }
  }
  const res = await call('workspace_open', { path })
  if (!res.ok) return res
  if (res.data.kind !== 'workspace_root') {
    return { ok: false, error: internalError(`workspace_open 返回 kind=${String(res.data.kind)}`), fallback: false }
  }
  return { ok: true, data: res.data, fallback: false }
}

export async function readDir(root: string, path: string): Promise<BridgeResult<DirListingResult>> {
  if (!isTauri()) {
    return { ok: true, data: placeholderListing(root, path), fallback: true }
  }
  const res = await call('fs_read_dir', { path })
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
  const res = await call('fs_read_file', { path })
  if (!res.ok) return res
  if (res.data.kind !== 'file_content') {
    return { ok: false, error: internalError(`fs_read_file 返回 kind=${String(res.data.kind)}`), fallback: false }
  }
  return { ok: true, data: res.data, fallback: false }
}

/**
 * 工作区全文搜索。默认大小写不敏感、字面量匹配；`regex` 开启按正则解释。
 * 结果由 Rust 侧遍历（尊重 .gitignore/.ignore），浏览器 dev 返回空结果并标注 fallback。
 */
export async function searchWorkspace(
  query: string,
  options: SearchRequestOptions = {},
): Promise<BridgeResult<SearchResult>> {
  if (!isTauri()) {
    return {
      ok: true,
      data: { files: [], totalMatches: 0, truncated: false, scannedFiles: 0 },
      fallback: true,
    }
  }
  const tauri = internals()
  if (!tauri) {
    return { ok: false, error: internalError('searchWorkspace 需要 Tauri 运行时'), fallback: false }
  }
  try {
    const raw = await tauri.invoke<SearchResult>('fs_search', {
      query,
      case_sensitive: options.caseSensitive ?? false,
      regex: options.regex ?? false,
      max_results: options.maxResults ?? 500,
    })
    if (typeof raw !== 'object' || raw === null || !Array.isArray(raw.files)) {
      return { ok: false, error: internalError('fs_search 返回了非预期载荷'), fallback: false }
    }
    return { ok: true, data: raw, fallback: false }
  } catch (error) {
    return { ok: false, error: asErrorBody(error), fallback: false }
  }
}

/** 保存文本文件；真实根由 Rust 传输层注入，浏览器 dev 只返回 fallback ack。 */export async function writeFile(
  path: string,
  content: string,
): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) {
    // 浏览器 dev 不写真实磁盘；fallback 标注让 UI 明确这是演示行为。
    void path
    void content
    return { ok: true, data: { kind: 'ack' }, fallback: true }
  }
  const res = await call('fs_write_file', { path, content })
  if (!res.ok) return res
  if (res.data.kind !== 'ack') {
    return { ok: false, error: internalError(`fs_write_file 返回 kind=${String(res.data.kind)}`), fallback: false }
  }
  return { ok: true, data: res.data, fallback: false }
}

/** 移动/重命名文件或目录；目标由 Rust 侧校验（根内、不覆盖、目录不自嵌套）。 */
export async function movePath(from: string, to: string): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) {
    void from
    void to
    return { ok: true, data: { kind: 'ack' }, fallback: true }
  }
  return asAck(await call('fs_move', { from, to }), 'fs_move')
}

/** 新建空文件或单层目录；父目录必须已存在且落在工作区内。 */
export async function createEntry(path: string, dir: boolean): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) {
    void path
    void dir
    return { ok: true, data: { kind: 'ack' }, fallback: true }
  }
  return asAck(await call('fs_create', { path, dir }), 'fs_create')
}

/** 删除文件或目录（目录递归删除）；根内校验由 Rust 侧完成。 */
export async function deletePath(path: string): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) {
    void path
    return { ok: true, data: { kind: 'ack' }, fallback: true }
  }
  return asAck(await call('fs_delete', { path }), 'fs_delete')
}

/** 重命名（后端复用移动实现）；根内与不覆盖校验同 fs_move。 */
export async function renamePath(from: string, to: string): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) {
    void from
    void to
    return { ok: true, data: { kind: 'ack' }, fallback: true }
  }
  return asAck(await call('fs_rename', { from, to }), 'fs_rename')
}

/** 在系统文件管理器中定位路径（Windows 选中；Linux 打开所在目录）。 */
export async function revealPath(path: string): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) {
    void path
    return { ok: true, data: { kind: 'ack' }, fallback: true }
  }
  return asAck(await call('fs_reveal', { path }), 'fs_reveal')
}

// ==== Agent 会话命令（契约 v0.3） ====

/** 浏览器 dev 会话 id；UI 在非 Tauri 环境禁用发送，这里只是保持返回形状一致。 */
const BROWSER_SESSION_ID = 'browser-demo'

/**
 * 创建会话。`taskId` 让会话绑定到 worktree 任务（契约 session_create.task_id）；
 * 浏览器 dev 返回带 `fallback: true` 的占位会话。
 * 真实 provider 配置与轮次由 Rust 侧决定（docs/AGENT.md §5）。
 */
export interface AgentAttachment {
  name: string
  path: string
  text: string
  bytes: number
}

/** 原生文件选择器：返回 UTF-8 文本附件，由用户显式选择。 */
export async function agentPickFiles(): Promise<BridgeResult<AgentAttachment[]>> {
  if (!isTauri()) return { ok: true, data: [], fallback: true }
  return invokeCustom<AgentAttachment[]>('agent_pick_files')
}

export async function sessionCreate(
  mode: SessionMode,
  taskId: string | null = null,
  model: string | null = null,
  thinking: string | null = null,
): Promise<BridgeResult<SessionResult>> {
  if (!isTauri()) {
    return {
      ok: true,
      data: {
        kind: 'session',
        session: {
          session_id: BROWSER_SESSION_ID,
          mode,
          turn: 0,
          max_turns: 25,
          context_k: 400,
          compress_at_k: 300,
          cancelled: false,
        },
      },
      fallback: true,
    }
  }
  const res = await call('session_create', { mode, task_id: taskId, model, thinking })
  if (!res.ok) return res
  if (res.data.kind !== 'session') {
    return { ok: false, error: internalError(`session_create 返回 kind=${String(res.data.kind)}`), fallback: false }
  }
  // 旧形状（缺上下文预算）不进入 reducer：视图层按无效会话显示错误，而不是用错误的预算。
  if (!isSessionInfo(res.data.session)) {
    return { ok: false, error: internalError('session_create 返回了非法 session 载荷'), fallback: false }
  }
  return { ok: true, data: res.data, fallback: false }
}

/** 发送用户消息；ack 只表示命令已受理，内容通过 agent:* 事件流返回。 */
export async function agentSend(
  sessionId: string,
  text: string,
): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) {
    void sessionId
    void text
    return { ok: true, data: { kind: 'ack' }, fallback: true }
  }
  return asAck(await call('agent_send', { session_id: sessionId, text }), 'agent_send')
}

/** 取消当前轮；Rust 侧通过 CancellationToken 传播到 HTTP 与工具执行。 */
export async function agentCancel(sessionId: string): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) {
    void sessionId
    return { ok: true, data: { kind: 'ack' }, fallback: true }
  }
  return asAck(await call('agent_cancel', { session_id: sessionId }), 'agent_cancel')
}

/** 提交审批决定；成功后 Rust 侧恢复该轮执行并继续下发 agent:* 事件。 */
export async function resolveApproval(
  sessionId: string,
  approvalId: number,
  decision: ApprovalDecision,
): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) {
    void sessionId
    void approvalId
    void decision
    return { ok: true, data: { kind: 'ack' }, fallback: true }
  }
  return asAck(
    await call('approval_resolve', {
      session_id: sessionId,
      approval_id: approvalId,
      decision,
    }),
    'approval_resolve',
  )
}

function asAck(
  res: BridgeResult<CommandResult>,
  command: string,
): BridgeResult<AckResult> {
  if (!res.ok) return res
  if (res.data.kind !== 'ack') {
    return {
      ok: false,
      error: internalError(`${command} 返回 kind=${String(res.data.kind)}`),
      fallback: false,
    }
  }
  return { ok: true, data: res.data, fallback: false }
}

// ==== Task 工作台命令（契约 v0.5） ====

/**
 * 浏览器 dev 的最小演示闭环：首次 taskList 为空；create/start/cancel/apply
 * 在内存里更新占位 Task，让三栏布局无需 Tauri 也能走通。真实任务数据只在
 * Tauri 环境经 task_* 命令获得。
 */
const browserTasks = new Map<string, TaskInfo>()
let browserTaskSeq = 0

function browserTask(
  taskId: string,
  title: string,
  status: TaskInfo['status'],
): TaskInfo {
  return {
    task_id: taskId,
    title,
    session_id: null,
    status,
    worktree_path: null,
    branch: null,
    port: null,
    base_sha: null,
    steps: [],
    changes: [],
    checks: [],
    created_at: Date.now(),
  }
}

function updateBrowserTask(
  taskId: string,
  patch: Partial<TaskInfo>,
): TaskInfo {
  const current = browserTasks.get(taskId) ?? browserTask(taskId, taskId, 'planning')
  const next = { ...current, ...patch }
  browserTasks.set(taskId, next)
  return next
}

export async function taskList(): Promise<BridgeResult<TaskListResult>> {
  if (!isTauri()) {
    return {
      ok: true,
      data: { kind: 'tasks', tasks: [...browserTasks.values()] },
      fallback: true,
    }
  }
  return asTaskList(await call('task_list', {}), 'task_list')
}

export async function taskCreate(
  title: string,
  sessionId: string | null = null,
): Promise<BridgeResult<TaskResult>> {
  if (!isTauri()) {
    browserTaskSeq += 1
    const task = browserTask(`browser-task-${browserTaskSeq}`, title, 'planning')
    task.session_id = sessionId
    browserTasks.set(task.task_id, task)
    return { ok: true, data: { kind: 'task', task }, fallback: true }
  }
  return asTaskResult(
    await call('task_create', { title, session_id: sessionId }),
    'task_create',
  )
}

export async function taskStart(taskId: string): Promise<BridgeResult<TaskResult>> {
  if (!isTauri()) {
    const task = updateBrowserTask(taskId, { status: 'running', port: 4300 })
    return { ok: true, data: { kind: 'task', task }, fallback: true }
  }
  return asTaskResult(await call('task_start', { task_id: taskId }), 'task_start')
}

export async function taskCancel(taskId: string): Promise<BridgeResult<TaskResult>> {
  if (!isTauri()) {
    const task = updateBrowserTask(taskId, { status: 'failed', port: null })
    return { ok: true, data: { kind: 'task', task }, fallback: true }
  }
  return asTaskResult(await call('task_cancel', { task_id: taskId }), 'task_cancel')
}

export async function taskApply(taskId: string): Promise<BridgeResult<TaskResult>> {
  if (!isTauri()) {
    const current = updateBrowserTask(taskId, { status: 'done' })
    const task = updateBrowserTask(taskId, {
      port: null,
      worktree_path: null,
      branch: null,
      changes: current.changes.map(change => ({ ...change, applied: true })),
    })
    return { ok: true, data: { kind: 'task', task }, fallback: true }
  }
  return asTaskResult(await call('task_apply', { task_id: taskId }), 'task_apply')
}

export async function taskDiscard(taskId: string): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) {
    browserTasks.delete(taskId)
    return { ok: true, data: { kind: 'ack' }, fallback: true }
  }
  return asAck(await call('task_discard', { task_id: taskId }), 'task_discard')
}

export async function taskDiff(taskId: string): Promise<BridgeResult<TaskDiffResult>> {
  if (!isTauri()) {
    const task = browserTasks.get(taskId)
    const files = task?.changes ?? []
    const body = files
      .map(change => `${change.status} ${change.path} (+${change.added}/-${change.removed})`)
      .join('\n')
    return {
      ok: true,
      data: {
        kind: 'task_diff',
        task_id: taskId,
        patch: body.length > 0
          ? `# browser dev placeholder diff\n${body}\n`
          : `# browser dev placeholder diff\ntask ${taskId} has no files\n`,
        files,
      },
      fallback: true,
    }
  }
  return asTaskDiff(await call('task_diff', { task_id: taskId }), 'task_diff')
}

export async function taskBindSession(
  taskId: string,
  sessionId: string | null,
): Promise<BridgeResult<TaskResult>> {
  if (!isTauri()) {
    const task = updateBrowserTask(taskId, { session_id: sessionId })
    return { ok: true, data: { kind: 'task', task }, fallback: true }
  }
  return asTaskResult(
    await call('task_bind_session', { task_id: taskId, session_id: sessionId }),
    'task_bind_session',
  )
}

function asTaskList(
  res: BridgeResult<CommandResult>,
  command: string,
): BridgeResult<TaskListResult> {
  if (!res.ok) return res
  if (res.data.kind !== 'tasks') {
    return {
      ok: false,
      error: internalError(`${command} 返回 kind=${String(res.data.kind)}`),
      fallback: false,
    }
  }
  const tasks: unknown = res.data.tasks
  if (!Array.isArray(tasks) || !tasks.every(isTaskInfo)) {
    return {
      ok: false,
      error: internalError(`${command} 返回了非法 TaskInfo 列表`),
      fallback: false,
    }
  }
  return {
    ok: true,
    data: { kind: 'tasks', tasks: tasks as TaskInfo[] },
    fallback: false,
  }
}

function asTaskResult(
  res: BridgeResult<CommandResult>,
  command: string,
): BridgeResult<TaskResult> {
  if (!res.ok) return res
  if (res.data.kind !== 'task') {
    return {
      ok: false,
      error: internalError(`${command} 返回 kind=${String(res.data.kind)}`),
      fallback: false,
    }
  }
  const task: unknown = res.data.task
  if (!isTaskInfo(task)) {
    return {
      ok: false,
      error: internalError(`${command} 返回了非法 TaskInfo`),
      fallback: false,
    }
  }
  return { ok: true, data: { kind: 'task', task }, fallback: false }
}

function asTaskDiff(
  res: BridgeResult<CommandResult>,
  command: string,
): BridgeResult<TaskDiffResult> {
  if (!res.ok) return res
  if (res.data.kind !== 'task_diff') {
    return {
      ok: false,
      error: internalError(`${command} 返回 kind=${String(res.data.kind)}`),
      fallback: false,
    }
  }
  const files: unknown = res.data.files
  if (
    !isString(res.data.task_id) ||
    !isString(res.data.patch) ||
    !Array.isArray(files) ||
    !files.every(isFileChangeDto)
  ) {
    return {
      ok: false,
      error: internalError(`${command} 返回了非法 task_diff 载荷`),
      fallback: false,
    }
  }
  return {
    ok: true,
    data: {
      kind: 'task_diff',
      task_id: res.data.task_id,
      patch: res.data.patch,
      files: files as FileChangeDto[],
    },
    fallback: false,
  }
}

// ==== 数据库命令（契约 v0.6；DTO 镜像见 dbTypes.ts） ====

/** 浏览器 dev 的内存连接表：不连真实数据库，仅让连接 CRUD 流程可演示。 */
const browserDbConnections = new Map<string, DbConnectionInfo>()

function browserDbConnection(connectionId: string): DbConnectionInfo {
  return (
    browserDbConnections.get(connectionId) ?? {
      connection_id: connectionId,
      name: connectionId,
      kind: 'sqlite',
      read_only: false,
      state: 'disconnected',
      host: null,
      database: null,
      error: null,
    }
  )
}

function browserDbInfoFromConfig(config: DbConnectionConfig): DbConnectionInfo {
  const previous = browserDbConnections.get(config.connection_id)
  return {
    connection_id: config.connection_id,
    name: config.name,
    kind: config.kind,
    read_only: config.read_only,
    state: previous?.state ?? 'disconnected',
    host: config.kind === 'sqlite' ? null : (config.host || null),
    database: config.database || null,
    error: null,
  }
}

export async function dbList(): Promise<BridgeResult<DbListResult>> {
  if (!isTauri()) {
    return {
      ok: true,
      data: { kind: 'connections', connections: [...browserDbConnections.values()] },
      fallback: true,
    }
  }
  return asDbList(await call('db_list', {}), 'db_list')
}

export async function dbConnect(
  connectionId: string,
): Promise<BridgeResult<DbConnectionResult>> {
  if (!isTauri()) {
    const connection: DbConnectionInfo = {
      ...browserDbConnection(connectionId),
      state: 'connected',
      error: null,
    }
    browserDbConnections.set(connectionId, connection)
    return { ok: true, data: { kind: 'connection', connection }, fallback: true }
  }
  return asDbConnection(
    await call('db_connect', { connection_id: connectionId }),
    'db_connect',
  )
}

export async function dbDisconnect(
  connectionId: string,
): Promise<BridgeResult<DbConnectionResult>> {
  if (!isTauri()) {
    const connection: DbConnectionInfo = {
      ...browserDbConnection(connectionId),
      state: 'disconnected',
      error: null,
    }
    browserDbConnections.set(connectionId, connection)
    return { ok: true, data: { kind: 'connection', connection }, fallback: true }
  }
  return asDbConnection(
    await call('db_disconnect', { connection_id: connectionId }),
    'db_disconnect',
  )
}

export async function dbSchema(
  connectionId: string,
): Promise<BridgeResult<DbSchemaResult>> {
  if (!isTauri()) {
    return {
      ok: true,
      data: { kind: 'db_schema', connection_id: connectionId, nodes: [] },
      fallback: true,
    }
  }
  return asDbSchema(
    await call('db_schema', { connection_id: connectionId }),
    'db_schema',
  )
}

export async function dbQuery(
  connectionId: string,
  sql: string,
  limit?: number,
): Promise<BridgeResult<DbQueryCommandResult>> {
  if (!isTauri()) {
    return {
      ok: true,
      data: {
        kind: 'db_query',
        result: {
          columns: [],
          rows: [],
          row_count: 0,
          truncated: false,
          handle: null,
          elapsed_ms: 0,
          untrusted: true,
        },
      },
      fallback: true,
    }
  }
  return asDbQuery(
    await call('db_query', {
      connection_id: connectionId,
      sql,
      limit: limit ?? null,
    }),
    'db_query',
  )
}

export async function dbExecute(
  connectionId: string,
  sql: string,
): Promise<BridgeResult<DbExecCommandResult>> {
  if (!isTauri()) {
    return {
      ok: true,
      data: { kind: 'db_exec', result: { affected_rows: 0, elapsed_ms: 0 } },
      fallback: true,
    }
  }
  return asDbExec(
    await call('db_execute', { connection_id: connectionId, sql }),
    'db_execute',
  )
}

export async function dbExplain(
  connectionId: string,
  sql: string,
): Promise<BridgeResult<DbExplainCommandResult>> {
  if (!isTauri()) {
    return {
      ok: true,
      data: {
        kind: 'db_explain',
        result: { plan: '浏览器演示模式：无真实执行计划', elapsed_ms: 0 },
      },
      fallback: true,
    }
  }
  return asDbExplain(
    await call('db_explain', { connection_id: connectionId, sql }),
    'db_explain',
  )
}

export async function dbSaveConnection(
  connection: DbConnectionConfig,
): Promise<BridgeResult<DbConnectionResult>> {
  if (!isTauri()) {
    const info = browserDbInfoFromConfig(connection)
    browserDbConnections.set(info.connection_id, info)
    return { ok: true, data: { kind: 'connection', connection: info }, fallback: true }
  }
  return asDbConnection(
    await call('db_save_connection', { connection }),
    'db_save_connection',
  )
}

export async function dbDeleteConnection(
  connectionId: string,
): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) {
    browserDbConnections.delete(connectionId)
    return { ok: true, data: { kind: 'ack' }, fallback: true }
  }
  return asAck(
    await call('db_delete_connection', { connection_id: connectionId }),
    'db_delete_connection',
  )
}

function asDbList(
  res: BridgeResult<CommandResult>,
  command: string,
): BridgeResult<DbListResult> {
  if (!res.ok) return res
  const data = res.data as unknown as Record<string, unknown>
  if (data.kind !== 'connections') {
    return {
      ok: false,
      error: internalError(`${command} 返回 kind=${String(data.kind)}`),
      fallback: false,
    }
  }
  const connections: unknown = data.connections
  if (!Array.isArray(connections) || !connections.every(isDbConnectionInfo)) {
    return {
      ok: false,
      error: internalError(`${command} 返回了非法 DbConnectionInfo 列表`),
      fallback: false,
    }
  }
  return {
    ok: true,
    data: { kind: 'connections', connections: connections as DbConnectionInfo[] },
    fallback: false,
  }
}

function asDbConnection(
  res: BridgeResult<CommandResult>,
  command: string,
): BridgeResult<DbConnectionResult> {
  if (!res.ok) return res
  const data = res.data as unknown as Record<string, unknown>
  if (data.kind !== 'connection') {
    return {
      ok: false,
      error: internalError(`${command} 返回 kind=${String(data.kind)}`),
      fallback: false,
    }
  }
  if (!isDbConnectionInfo(data.connection)) {
    return {
      ok: false,
      error: internalError(`${command} 返回了非法 DbConnectionInfo`),
      fallback: false,
    }
  }
  return {
    ok: true,
    data: { kind: 'connection', connection: data.connection },
    fallback: false,
  }
}

function asDbSchema(
  res: BridgeResult<CommandResult>,
  command: string,
): BridgeResult<DbSchemaResult> {
  if (!res.ok) return res
  const data = res.data as unknown as Record<string, unknown>
  if (data.kind !== 'db_schema') {
    return {
      ok: false,
      error: internalError(`${command} 返回 kind=${String(data.kind)}`),
      fallback: false,
    }
  }
  const nodes: unknown = data.nodes
  if (
    !isString(data.connection_id) ||
    !Array.isArray(nodes) ||
    !nodes.every(isDbSchemaNode)
  ) {
    return {
      ok: false,
      error: internalError(`${command} 返回了非法 db_schema 载荷`),
      fallback: false,
    }
  }
  return {
    ok: true,
    data: {
      kind: 'db_schema',
      connection_id: data.connection_id,
      nodes: nodes as DbSchemaNode[],
    },
    fallback: false,
  }
}

function asDbQuery(
  res: BridgeResult<CommandResult>,
  command: string,
): BridgeResult<DbQueryCommandResult> {
  if (!res.ok) return res
  const data = res.data as unknown as Record<string, unknown>
  if (data.kind !== 'db_query') {
    return {
      ok: false,
      error: internalError(`${command} 返回 kind=${String(data.kind)}`),
      fallback: false,
    }
  }
  if (!isDbQueryResult(data.result)) {
    return {
      ok: false,
      error: internalError(`${command} 返回了非法 DbQueryResult`),
      fallback: false,
    }
  }
  return {
    ok: true,
    data: { kind: 'db_query', result: data.result },
    fallback: false,
  }
}

function asDbExec(
  res: BridgeResult<CommandResult>,
  command: string,
): BridgeResult<DbExecCommandResult> {
  if (!res.ok) return res
  const data = res.data as unknown as Record<string, unknown>
  if (data.kind !== 'db_exec') {
    return {
      ok: false,
      error: internalError(`${command} 返回 kind=${String(data.kind)}`),
      fallback: false,
    }
  }
  if (!isDbExecResult(data.result)) {
    return {
      ok: false,
      error: internalError(`${command} 返回了非法 DbExecResult`),
      fallback: false,
    }
  }
  return {
    ok: true,
    data: { kind: 'db_exec', result: data.result },
    fallback: false,
  }
}

function asDbExplain(
  res: BridgeResult<CommandResult>,
  command: string,
): BridgeResult<DbExplainCommandResult> {
  if (!res.ok) return res
  const data = res.data as unknown as Record<string, unknown>
  if (data.kind !== 'db_explain') {
    return {
      ok: false,
      error: internalError(`${command} 返回 kind=${String(data.kind)}`),
      fallback: false,
    }
  }
  if (!isDbExplainResult(data.result)) {
    return {
      ok: false,
      error: internalError(`${command} 返回了非法 DbExplainResult`),
      fallback: false,
    }
  }
  return {
    ok: true,
    data: { kind: 'db_explain', result: data.result },
    fallback: false,
  }
}

// ==== Git 面板命令（契约 v0.7） ====

/** 浏览器 dev 的空仓库状态：不伪造真实工作区，面板显示 clean。 */
function placeholderGitStatus(): GitStatus {
  return {
    branch: 'main',
    upstream: null,
    ahead: 0,
    behind: 0,
    clean: true,
    merge_in_progress: false,
    rebase_in_progress: false,
    files: [],
  }
}

function isGitFileStatus(value: unknown): value is GitFileStatus {
  if (!isRecord(value)) return false
  return (
    isString(value.path) &&
    isChangeStatus(value.status) &&
    typeof value.staged === 'boolean' &&
    isFiniteNumber(value.added) &&
    Number.isInteger(value.added) &&
    value.added >= 0 &&
    isFiniteNumber(value.removed) &&
    Number.isInteger(value.removed) &&
    value.removed >= 0
  )
}

function isGitStatus(value: unknown): value is GitStatus {
  if (!isRecord(value)) return false
  const branch = value.branch
  const upstream = value.upstream
  return (
    (branch === null || branch === undefined || isString(branch)) &&
    (upstream === null || upstream === undefined || isString(upstream)) &&
    isFiniteNumber(value.ahead) &&
    Number.isInteger(value.ahead) &&
    value.ahead >= 0 &&
    isFiniteNumber(value.behind) &&
    Number.isInteger(value.behind) &&
    value.behind >= 0 &&
    typeof value.clean === 'boolean' &&
    typeof value.merge_in_progress === 'boolean' &&
    typeof value.rebase_in_progress === 'boolean' &&
    Array.isArray(value.files) &&
    value.files.every(isGitFileStatus)
  )
}

function isGitCommit(value: unknown): value is GitCommit {
  if (!isRecord(value)) return false
  return (
    isString(value.sha) &&
    isString(value.short_sha) &&
    isString(value.author) &&
    isString(value.message) &&
    isFiniteNumber(value.timestamp) &&
    value.timestamp >= 0
  )
}

/** 工作区状态；浏览器 dev 返回空仓库占位。 */
export async function gitStatus(): Promise<BridgeResult<GitStatusResult>> {
  if (!isTauri()) {
    return {
      ok: true,
      data: { kind: 'git_status', status: placeholderGitStatus() },
      fallback: true,
    }
  }
  const res = await call('git_status', {})
  if (!res.ok) return res
  if (res.data.kind !== 'git_status') {
    return { ok: false, error: internalError(`git_status 返回 kind=${String(res.data.kind)}`), fallback: false }
  }
  if (!isGitStatus(res.data.status)) {
    return { ok: false, error: internalError('git_status 返回了非法 GitStatus'), fallback: false }
  }
  return { ok: true, data: res.data, fallback: false }
}

/** 单文件（或整棵树 path=null）的 unified patch；staged 决定读 index 还是工作区。 */
export async function gitDiff(
  path: string | null,
  staged: boolean,
): Promise<BridgeResult<GitDiffResult>> {
  if (!isTauri()) {
    const target = path ?? '(working tree)'
    return {
      ok: true,
      data: {
        kind: 'git_diff',
        patch: `# 浏览器 dev 占位 diff
# ${target} (${staged ? 'staged' : 'unstaged'})
`,
        files: [],
      },
      fallback: true,
    }
  }
  const res = await call('git_diff', { path, staged })
  if (!res.ok) return res
  if (res.data.kind !== 'git_diff') {
    return { ok: false, error: internalError(`git_diff 返回 kind=${String(res.data.kind)}`), fallback: false }
  }
  if (
    !isString(res.data.patch) ||
    !Array.isArray(res.data.files) ||
    !res.data.files.every(isGitFileStatus)
  ) {
    return { ok: false, error: internalError('git_diff 返回了非法载荷'), fallback: false }
  }
  return {
    ok: true,
    data: {
      kind: 'git_diff',
      patch: res.data.patch,
      files: res.data.files as GitFileStatus[],
    },
    fallback: false,
  }
}

export async function gitStage(paths: string[]): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) return { ok: true, data: { kind: 'ack' }, fallback: true }
  return asAck(await call('git_stage', { paths }), 'git_stage')
}

export async function gitUnstage(paths: string[]): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) return { ok: true, data: { kind: 'ack' }, fallback: true }
  return asAck(await call('git_unstage', { paths }), 'git_unstage')
}

export async function gitCommit(message: string): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) return { ok: true, data: { kind: 'ack' }, fallback: true }
  return asAck(await call('git_commit', { message }), 'git_commit')
}

/** 最近提交；limit=null 表示不设上限（契约 Option<u32>）。 */
export async function gitLog(
  limit: number | null = null,
): Promise<BridgeResult<GitLogResult>> {
  if (!isTauri()) {
    return { ok: true, data: { kind: 'git_log', commits: [] }, fallback: true }
  }
  const res = await call('git_log', { limit })
  if (!res.ok) return res
  if (res.data.kind !== 'git_log') {
    return { ok: false, error: internalError(`git_log 返回 kind=${String(res.data.kind)}`), fallback: false }
  }
  if (!Array.isArray(res.data.commits) || !res.data.commits.every(isGitCommit)) {
    return { ok: false, error: internalError('git_log 返回了非法 GitCommit 列表'), fallback: false }
  }
  return {
    ok: true,
    data: { kind: 'git_log', commits: res.data.commits as GitCommit[] },
    fallback: false,
  }
}

// ==== 核心事件订阅（Tauri v2 event plugin 的低层入口） ====

/**
 * Rust 侧 `app.emit` 使用的事件名（主 agent 的 Tauri 命令层须与此一致）。
 *
 * docs/ARCHITECTURE.md §4 的 `core://event` 是早期示意名；契约 v0.3 的
 * Agent 视图接线按任务约定统一为 `core:event`。所有 agent:*、task:* 与 error
 * 事件都作为同一个 CoreEvent 载荷下发（也接受 EventEnvelope 包装）。
 */
export const CORE_EVENT_CHANNEL = 'core:event'

/**
 * 订阅核心事件流。只在 Tauri webview 内生效；浏览器 dev 返回 no-op。
 *
 * 实现对齐 @tauri-apps/api/event 的 listen：
 * `transformCallback` 注册回调 → `plugin:event|listen` 拿到 eventId →
 * 取消订阅时 `plugin:event|unlisten`。不引入 @tauri-apps/api 依赖。
 *
 * 只转发 agent:*、task:*、db:status 与 error 事件；载荷形状不符（非契约数据）直接丢弃。
 *
 * db:status 属于契约 v0.6，但仓内 codegen 产物尚未更新（见 dbTypes.ts 头注释），
 * 因此保留 CoreEvent 重载给现有调用方，另加 UiCoreEvent 重载给数据库面板。
 */
export function onCoreEvent(handler: (event: CoreEvent) => void): () => void
export function onCoreEvent(handler: (event: UiCoreEvent) => void): () => void
export function onCoreEvent(
  handler: ((event: CoreEvent) => void) | ((event: UiCoreEvent) => void),
): () => void {
  const dispatch = handler as (event: UiCoreEvent) => void
  const tauri = internals()
  const transform = tauri?.transformCallback
  if (!tauri || typeof transform !== 'function') return () => {}

  let active = true
  let unlisten: (() => void) | null = null

  const callbackId = transform((raw: unknown) => {
    if (!active) return
    const event = asCoreEvent(raw)
    if (event) dispatch(event)
  })

  void tauri
    .invoke<number>('plugin:event|listen', {
      event: CORE_EVENT_CHANNEL,
      target: { kind: 'Any' },
      handler: callbackId,
    })
    .then(eventId => {
      const stop = (): void => {
        unregisterEventCallback(CORE_EVENT_CHANNEL, eventId)
        void tauri
          .invoke('plugin:event|unlisten', { event: CORE_EVENT_CHANNEL, eventId })
          .catch(() => {})
      }
      if (active) unlisten = stop
      else stop()
    })
    .catch(() => {
      // 监听注册失败只影响事件流；命令通道仍可用，UI 会显示错误/空状态。
    })

  return () => {
    active = false
    if (unlisten) {
      unlisten()
      unlisten = null
    }
  }
}

// ==== LSP 命令 ====

export interface LspServerInfo {
  id: string
  languageIds: string[]
}

export interface LspStartFailure {
  id: string
  message: string
}

export interface LspStartReport {
  servers: LspServerInfo[]
  errors: LspStartFailure[]
}

export interface LspServerConfig {
  id: string
  languageIds: string[]
  command: string
  args: string[]
  env: Record<string, string>
}

/** 从启用插件清单读取 languageServers 并启动（Rust 侧完成转译）。 */
export async function lspStartPlugins(): Promise<BridgeResult<LspStartReport>> {
  if (!isTauri()) {
    return { ok: true, data: { servers: [], errors: [] }, fallback: true }
  }
  return invokeCustom<LspStartReport>('lsp_start_plugins')
}

/** 启动语言服务器；不传 configs 时使用 Rust 侧暂存的插件配置。 */
export async function lspStartAll(
  configs?: LspServerConfig[],
): Promise<BridgeResult<LspStartReport>> {
  if (!isTauri()) {
    return { ok: true, data: { servers: [], errors: [] }, fallback: true }
  }
  return invokeCustom<LspStartReport>('lsp_start_all', { configs: configs ?? null })
}

export async function lspDidOpen(
  path: string,
  languageId: string,
  text: string,
): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) return { ok: true, data: { kind: 'ack' }, fallback: true }
  return asAck(await call('lsp_did_open', { path, language_id: languageId, text }), 'lsp_did_open')
}

export async function lspDidChange(
  path: string,
  text: string,
): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) return { ok: true, data: { kind: 'ack' }, fallback: true }
  return asAck(await call('lsp_did_change', { path, text }), 'lsp_did_change')
}

export async function lspDidClose(path: string): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) return { ok: true, data: { kind: 'ack' }, fallback: true }
  return asAck(await call('lsp_did_close', { path }), 'lsp_did_close')
}

export async function lspStopAll(): Promise<BridgeResult<AckResult>> {
  if (!isTauri()) return { ok: true, data: { kind: 'ack' }, fallback: true }
  return asAck(await call('lsp_stop_all', {}), 'lsp_stop_all')
}

/**
 * textDocument/completion。`line` / `column` 为 1-based（与 Monaco 一致），
 * Rust 侧换算成 LSP 的 0-based UTF-16 position。浏览器 dev 返回空列表。
 */
export async function lspCompletion(
  path: string,
  line: number,
  column: number,
): Promise<BridgeResult<LspCompletionItem[]>> {
  if (!isTauri()) return { ok: true, data: [], fallback: true }
  return invokeCustom<LspCompletionItem[]>('lsp_completion', { path, line, column })
}

/** textDocument/hover；该位置无悬停时 `contents` 为空数组。 */
export async function lspHover(
  path: string,
  line: number,
  column: number,
): Promise<BridgeResult<LspHover>> {
  if (!isTauri()) return { ok: true, data: { contents: [] }, fallback: true }
  return invokeCustom<LspHover>('lsp_hover', { path, line, column })
}

/** textDocument/definition；空数组表示无定义。 */
export async function lspDefinition(
  path: string,
  line: number,
  column: number,
): Promise<BridgeResult<LspLocation[]>> {
  if (!isTauri()) return { ok: true, data: [], fallback: true }
  return invokeCustom<LspLocation[]>('lsp_definition', { path, line, column })
}

// ==== LSP 诊断事件 ====

export interface LspDiagnostic {
  severity: number
  message: string
  line: number
  column: number
  endLine: number
  endColumn: number
}

export interface LspDiagnosticsEvent {
  serverId: string
  path: string
  diagnostics: LspDiagnostic[]
}

const LSP_DIAGNOSTICS_CHANNEL = 'lsp:diagnostics'

/** 订阅 LSP 诊断（与 core:event 相同的 Tauri event internals，无额外依赖）。 */
export function onLspDiagnostics(handler: (event: LspDiagnosticsEvent) => void): () => void {
  const tauri = internals()
  const transform = tauri?.transformCallback
  if (!tauri || typeof transform !== 'function') return () => {}

  let active = true
  let unlisten: (() => void) | null = null

  const callbackId = transform((raw: unknown) => {
    if (!active || typeof raw !== 'object' || raw === null) return
    // Tauri event 回调载荷是 { event, payload, id }；与 onCoreEvent 解包一致。
    const payload = (raw as { payload?: unknown }).payload
    const value = (payload ?? raw) as Partial<LspDiagnosticsEvent>
    if (
      typeof value.serverId !== 'string' ||
      typeof value.path !== 'string' ||
      !Array.isArray(value.diagnostics)
    ) {
      return
    }
    const diagnostics = value.diagnostics.filter(
      (item): item is LspDiagnostic =>
        typeof item === 'object' &&
        item !== null &&
        typeof item.message === 'string' &&
        [item.severity, item.line, item.column, item.endLine, item.endColumn].every(
          number => typeof number === 'number' && Number.isFinite(number),
        ),
    )
    handler({ serverId: value.serverId, path: value.path, diagnostics })
  })

  void tauri
    .invoke<number>('plugin:event|listen', {
      event: LSP_DIAGNOSTICS_CHANNEL,
      target: { kind: 'Any' },
      handler: callbackId,
    })
    .then(eventId => {
      const stop = (): void => {
        unregisterEventCallback(LSP_DIAGNOSTICS_CHANNEL, eventId)
        void tauri
          .invoke('plugin:event|unlisten', { event: LSP_DIAGNOSTICS_CHANNEL, eventId })
          .catch(() => {})
      }
      if (active) unlisten = stop
      else stop()
    })
    .catch(() => {})

  return () => {
    active = false
    if (unlisten) {
      unlisten()
      unlisten = null
    }
  }
}

function unregisterEventCallback(event: string, eventId: number): void {
  if (typeof window === 'undefined') return
  const plugin = (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__?: TauriEventInternals })
    .__TAURI_EVENT_PLUGIN_INTERNALS__
  plugin?.unregisterListener?.(event, eventId)
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function isString(value: unknown): value is string {
  return typeof value === 'string'
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value)
}

function isSessionMode(value: unknown): value is SessionMode {
  return value === 'plan' || value === 'chat' || value === 'auto' || value === 'yolo'
}

function isSessionInfo(value: unknown): value is SessionInfo {
  if (!isRecord(value)) return false
  return (
    isString(value.session_id) &&
    isSessionMode(value.mode) &&
    isFiniteNumber(value.turn) &&
    isFiniteNumber(value.max_turns) &&
    isFiniteNumber(value.context_k) &&
    value.context_k >= 0 &&
    isFiniteNumber(value.compress_at_k) &&
    value.compress_at_k >= 0 &&
    typeof value.cancelled === 'boolean'
  )
}

function isApprovalPrompt(value: unknown): boolean {
  if (!isRecord(value)) return false
  const command = value.command
  return (
    isFiniteNumber(value.approval_id) &&
    isString(value.tool) &&
    (command === undefined || command === null || typeof command === 'string') &&
    isString(value.source) &&
    isString(value.reason)
  )
}

function isErrorBodyLike(value: unknown): boolean {
  return isRecord(value) && isString(value.code) && isString(value.message)
}

/**
 * 事件载荷校验：只放行本 UI 消费的 agent:*、task:* 与 error。
 * 校验字段是 reducer 会直接读取的那些（如 approval.approval_id），
 * 防止跨 IPC 的畸形数据在 reducer 里抛错。
 */
function isCoreEvent(value: unknown): value is UiCoreEvent {
  if (!isRecord(value) || !isString(value.type)) return false
  switch (value.type) {
    case 'db:status':
      return isDbConnectionInfo(value.connection)
    case 'task:created':
      return isString(value.task_id) && isString(value.title)
    case 'task:status':
      // 契约里是 string，但 UI 的状态徽标只认识 TaskStatus 枚举。
      return isString(value.task_id) && isTaskStatus(value.status)
    case 'task:updated':
      return isTaskInfo(value.task)
    case 'task:step':
      return (
        isString(value.task_id) &&
        isFiniteNumber(value.index) &&
        isString(value.title) &&
        isTaskStepState(value.state)
      )
    case 'task:change':
      return isString(value.task_id) && isFileChangeDto(value.change)
    case 'task:check':
      return isString(value.task_id) && isTaskCheck(value.check)
    case 'task:needs_input':
      return isString(value.task_id) && isString(value.kind)
    case 'task:merged':
      return isString(value.task_id) && isString(value.commit_sha)
    case 'task:discarded':
      return isString(value.task_id)
    case 'agent:text_delta':
      return isString(value.session_id) && isFiniteNumber(value.index) && isString(value.text)
    case 'agent:thinking_delta':
      return isString(value.session_id) && isString(value.text)
    case 'agent:tool_start':
      return isString(value.session_id) && isString(value.call_id) && isString(value.name)
    case 'agent:tool_result':
      return (
        isString(value.session_id) &&
        isString(value.call_id) &&
        typeof value.ok === 'boolean' &&
        isString(value.text)
      )
    case 'agent:approval_required':
      return isString(value.session_id) && isApprovalPrompt(value.approval)
    case 'agent:session_state':
      return isSessionInfo(value.session)
    case 'agent:usage':
      return (
        isString(value.session_id) &&
        isFiniteNumber(value.input) &&
        isFiniteNumber(value.output) &&
        isFiniteNumber(value.cached)
      )
    case 'agent:compacted':
      return (
        isString(value.session_id) &&
        isFiniteNumber(value.record_id) &&
        isFiniteNumber(value.before_tokens) &&
        isFiniteNumber(value.after_tokens) &&
        isFiniteNumber(value.removed_messages) &&
        isString(value.summary)
      )
    case 'agent:done':
      return isString(value.session_id) && isString(value.stop_reason)
    case 'error':
      return isErrorBodyLike(value.error)
    default:
      return false
  }
}

/**
 * Tauri 事件回调 → UiCoreEvent。兼容三种载荷：
 * - `{ event, id, payload }`（Tauri Event）
 * - `{ seq, event }`（契约 EventEnvelope）
 * - 裸 CoreEvent / db:status
 */
function asCoreEvent(raw: unknown): UiCoreEvent | null {
  const eventPayload = isRecord(raw) && 'payload' in raw ? raw.payload : raw
  const candidate =
    isRecord(eventPayload) && isFiniteNumber(eventPayload.seq) && 'event' in eventPayload
      ? eventPayload.event
      : eventPayload
  return isCoreEvent(candidate) ? candidate : null
}

function browserNodeStatus(): BridgeResult<NodeStatusResult> {
  return {
    ok: true,
    data: { kind: 'node_status', status: { kind: 'need_manual', attempts: [] } },
    fallback: true,
  }
}

function asNodeStatus(
  res: BridgeResult<CommandResult>,
  command: string,
): BridgeResult<NodeStatusResult> {
  if (!res.ok) return res
  if (res.data.kind !== 'node_status') {
    return {
      ok: false,
      error: internalError(`${command} 返回 kind=${String(res.data.kind)}`),
      fallback: false,
    }
  }
  return { ok: true, data: res.data, fallback: false }
}

/** Node 探测（惰性触发由前端控制，docs/RUNTIME.md）。 */
export async function detectNode(): Promise<BridgeResult<NodeStatusResult>> {
  if (!isTauri()) return browserNodeStatus()
  return asNodeStatus(await call('node_detect', {}), 'node_detect')
}

/** 手动指定 node.exe；Rust 侧校验通过才写配置。 */
export async function setNodeManual(path: string): Promise<BridgeResult<NodeStatusResult>> {
  if (!isTauri()) return browserNodeStatus()
  return asNodeStatus(await call('node_set_manual', { path }), 'node_set_manual')
}

/** 下载并安装便携版 Node；version 为空时用基准主版本。 */
export async function downloadNodePortable(
  version?: string,
): Promise<BridgeResult<NodeStatusResult>> {
  if (!isTauri()) return browserNodeStatus()
  return asNodeStatus(
    await call('node_download_portable', { version: version ?? null }),
    'node_download_portable',
  )
}

/** 非契约命令（providers_* / fs_search）的通用 invoke：原样返回 serde 载荷。 */
async function invokeCustom<T>(
  command: string,
  args: Record<string, unknown> = {},
): Promise<BridgeResult<T>> {
  const tauri = internals()
  if (!tauri) {
    return { ok: false, error: internalError(`${command} 需要 Tauri 运行时`), fallback: false }
  }
  try {
    return { ok: true, data: await tauri.invoke<T>(command, args), fallback: false }
  } catch (error) {
    return { ok: false, error: asErrorBody(error), fallback: false }
  }
}

async function call(command: string, args: Record<string, unknown>): Promise<BridgeResult<CommandResult>> {
  try {
    return { ok: true, data: await invokeContract(command, args), fallback: false }
  } catch (raw) {
    return { ok: false, error: asErrorBody(raw), fallback: false }
  }
}

// ==== Agent 会话历史 ====

export interface HistorySummary {
  id: string
  title: string
  savedAt: number
  workspace: string
  mode: SessionMode
  model: string
  turn: number
  messages: number
  archived: boolean
  /** 消息正文摘要，快速打开按它检索会话内容。 */
  excerpt: string
}

const HISTORY_FALLBACK_KEY = 'lc.agentHistory'

function fallbackHistory(): Record<string, unknown> {
  try {
    const raw = localStorage.getItem(HISTORY_FALLBACK_KEY)
    const parsed = raw ? (JSON.parse(raw) as unknown) : null
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : {}
  } catch {
    return {}
  }
}

function fallbackSaveHistory(): void {
  try {
    localStorage.setItem(HISTORY_FALLBACK_KEY, JSON.stringify(fallbackHistory()))
  } catch {
    // 浏览器演示模式的配额失败不影响主流程。
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

export async function historyList(): Promise<BridgeResult<HistorySummary[]>> {
  if (!isTauri()) {
    const items = Object.entries(fallbackHistory())
      .map(([id, value]) => summaryOf(id, value))
      .filter((item): item is HistorySummary => item !== null)
      .sort((a, b) => b.savedAt - a.savedAt)
    return { ok: true, data: items, fallback: true }
  }
  return invokeCustom<HistorySummary[]>('history_list')
}

export async function historyLoad(id: string): Promise<BridgeResult<unknown>> {
  if (!isTauri()) {
    const value = fallbackHistory()[id]
    if (!value) {
      return {
        ok: false,
        error: { code: 'not_found', message: '会话不存在', detail: null },
        fallback: false,
      }
    }
    return { ok: true, data: value, fallback: true }
  }
  return invokeCustom<unknown>('history_load', { id })
}

export async function historySave(
  id: string,
  payload: unknown,
): Promise<BridgeResult<null>> {
  if (!isTauri()) {
    const store = fallbackHistory()
    store[id] = payload
    fallbackSaveHistory()
    return { ok: true, data: null, fallback: true }
  }
  return invokeCustom<null>('history_save', { id, payload })
}

export async function historyDelete(id: string): Promise<BridgeResult<null>> {
  if (!isTauri()) {
    const store = fallbackHistory()
    delete store[id]
    fallbackSaveHistory()
    return { ok: true, data: null, fallback: true }
  }
  return invokeCustom<null>('history_delete', { id })
}

export async function historyMigrateWorkspace(
  workspace: string,
): Promise<BridgeResult<number>> {
  if (!isTauri()) {
    const store = fallbackHistory()
    let moved = 0
    for (const value of Object.values(store)) {
      if (!value || typeof value !== 'object') continue
      const item = value as Record<string, unknown>
      if (typeof item.workspace === 'string' && item.workspace) continue
      item.workspace = workspace
      moved += 1
    }
    if (moved > 0) fallbackSaveHistory()
    return { ok: true, data: moved, fallback: true }
  }
  return invokeCustom<number>('history_migrate_workspace', { workspace })
}

export async function historyRename(
  id: string,
  title: string,
): Promise<BridgeResult<null>> {
  if (!isTauri()) {
    const store = fallbackHistory()
    const value = store[id]
    if (!value || typeof value !== 'object') {
      return {
        ok: false,
        error: { code: 'not_found', message: '会话不存在', detail: null },
        fallback: false,
      }
    }
    ;(value as Record<string, unknown>).title = title
    fallbackSaveHistory()
    return { ok: true, data: null, fallback: true }
  }
  return invokeCustom<null>('history_rename', { id, title })
}

export async function historyArchive(
  id: string,
  archived: boolean,
): Promise<BridgeResult<null>> {
  if (!isTauri()) {
    const store = fallbackHistory()
    const value = store[id]
    if (!value || typeof value !== 'object') {
      return {
        ok: false,
        error: { code: 'not_found', message: '会话不存在', detail: null },
        fallback: false,
      }
    }
    ;(value as Record<string, unknown>).archived = archived
    fallbackSaveHistory()
    return { ok: true, data: null, fallback: true }
  }
  return invokeCustom<null>('history_archive', { id, archived })
}

// ==== 供应商管理 ====

function demoProvidersView(): ProvidersView {
  return {
    providers: [
      {
        id: 'openai',
        kind: 'openai-responses',
        label: 'OpenAI',
        baseUrl: 'https://api.openai.com/v1',
        enabled: true,
        apiKeyRef: '',
        hasApiKey: false,
        apiKeyHint: null,
      },
    ],
    models: [
      {
        id: 'gpt-5',
        provider: 'openai',
        modelId: 'gpt-5',
        thinking: ['off', 'low', 'medium', 'high'],
        contextK: 400,
        compressAtK: 300,
        capabilities: ['tools'],
      },
    ],
    defaultModel: 'gpt-5',
    defaultThinking: 'medium',
    subagentModel: null,
    subagentThinking: null,
  }
}

/** 读取供应商配置；密钥只返回 hasApiKey 与末四位提示。 */
export async function providersLoad(): Promise<BridgeResult<ProvidersView>> {
  if (!isTauri()) {
    return { ok: true, data: demoProvidersView(), fallback: true }
  }
  const res = await invokeCustom<ProvidersView>('providers_load')
  if (!res.ok) return res
  if (!Array.isArray(res.data.providers) || !Array.isArray(res.data.models)) {
    return { ok: false, error: internalError('providers_load 返回了非预期载荷'), fallback: false }
  }
  return res
}

/** 保存供应商配置。空白 apiKey 表示保留旧值；clearApiKey 显式清除。 */
export async function providersSave(
  input: ProvidersSaveInput,
): Promise<BridgeResult<ProvidersView>> {
  if (!isTauri()) {
    return {
      ok: true,
      data: {
        providers: input.providers.map(provider => ({
          id: provider.id,
          kind: provider.kind,
          label: provider.label,
          baseUrl: provider.baseUrl,
          enabled: provider.enabled,
          apiKeyRef: provider.apiKeyRef ?? '',
          hasApiKey: Boolean(provider.apiKey && provider.apiKey.trim()),
          apiKeyHint: null,
        })),
        models: input.models,
        defaultModel: input.defaultModel,
        defaultThinking: input.defaultThinking,
        subagentModel: input.subagentModel,
        subagentThinking: input.subagentThinking,
      },
      fallback: true,
    }
  }
  const res = await invokeCustom<ProvidersView>('providers_save', { input })
  if (!res.ok) return res
  if (!Array.isArray(res.data.providers) || !Array.isArray(res.data.models)) {
    return { ok: false, error: internalError('providers_save 返回了非预期载荷'), fallback: false }
  }
  return res
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
