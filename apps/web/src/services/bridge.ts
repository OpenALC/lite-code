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
  SessionInfo,
  SessionMode,
} from '@lc-proto'

import {
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

/** 保存文本文件；真实根由 Rust 传输层注入，浏览器 dev 只返回 fallback ack。 */
export async function writeFile(
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

// ==== Agent 会话命令（契约 v0.3） ====

/** 浏览器 dev 会话 id；UI 在非 Tauri 环境禁用发送，这里只是保持返回形状一致。 */
const BROWSER_SESSION_ID = 'browser-demo'

/**
 * 创建会话。`taskId` 让会话绑定到 worktree 任务（契约 session_create.task_id）；
 * 浏览器 dev 返回带 `fallback: true` 的占位会话。
 * 真实 provider 配置与轮次由 Rust 侧决定（docs/AGENT.md §5）。
 */
export async function sessionCreate(
  mode: SessionMode,
  taskId: string | null = null,
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
  const res = await call('session_create', { mode, task_id: taskId })
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
 * 只转发 agent:*、task:* 与 error 事件；载荷形状不符（非契约数据）直接丢弃。
 */
export function onCoreEvent(handler: (event: CoreEvent) => void): () => void {
  const tauri = internals()
  const transform = tauri?.transformCallback
  if (!tauri || typeof transform !== 'function') return () => {}

  let active = true
  let unlisten: (() => void) | null = null

  const callbackId = transform((raw: unknown) => {
    if (!active) return
    const event = asCoreEvent(raw)
    if (event) handler(event)
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
function isCoreEvent(value: unknown): value is CoreEvent {
  if (!isRecord(value) || !isString(value.type)) return false
  switch (value.type) {
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
 * Tauri 事件回调 → CoreEvent。兼容三种载荷：
 * - `{ event, id, payload }`（Tauri Event）
 * - `{ seq, event }`（契约 EventEnvelope）
 * - 裸 CoreEvent
 */
function asCoreEvent(raw: unknown): CoreEvent | null {
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
