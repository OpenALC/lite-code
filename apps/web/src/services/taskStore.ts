// Task 工作台状态、reducer 与控制器（docs/TASKS.md §2–§4；P4 task-ui lane）。
// 框架无关：不可变 reducer + 依赖注入的 bridge 控制器（UI.md §8）。
//
// 事件来源是 core:event 的 task:* 事件。命令结果同时携带 TaskInfo，
// 控制器用它做乐观合并；事件流负责多任务并行时的增量更新。
import type { CoreEvent, ErrorCode } from '@lc-proto'

import type {
  AckResult,
  BridgeResult,
  TaskDiffResult,
  TaskListResult,
  TaskResult,
} from './bridge'
import {
  isTaskStatus,
  type FileChangeDto,
  type TaskCheck,
  type TaskInfo,
  type TaskStatus,
  type TaskStep,
} from './taskTypes'

/** 同时运行任务数上限（docs/TASKS.md §4；硬上限由 Rust 侧再钳制到 4）。 */
export const MAX_RUNNING_TASKS = 2

/** 工作台标签；按任务记忆，切换任务时恢复（P4 出口 #4）。 */
export type WorkbenchTab = 'changes' | 'checks' | 'diff' | 'preview'

export const WORKBENCH_TABS = ['changes', 'checks', 'diff', 'preview'] as const

export const DEFAULT_WORKBENCH_TAB: WorkbenchTab = 'changes'

export interface TaskState {
  tasks: TaskInfo[]
  activeTaskId: string | null
  /** 状态栏「N 待确认」计数；与 needsInputTaskIds 同源，避免重复事件重复计数。 */
  needsInputCount: number
  /** 需要用户输入的任务（按首次事件到达顺序），状态栏点击时取第一个。 */
  needsInputTaskIds: string[]
  lastMerged: { taskId: string; commitSha: string } | null
  lastDiscarded: string | null
  error: { code: ErrorCode; message: string } | null
  /** 每个任务上次停留的工作台标签；缺省按 DEFAULT_WORKBENCH_TAB 处理。 */
  workbenchTabByTask: Record<string, WorkbenchTab>
  /** 每个任务预览的 URL；缺省/空串表示未配置。 */
  previewUrlByTask: Record<string, string>
}

export function createTaskState(): TaskState {
  return {
    tasks: [],
    activeTaskId: null,
    needsInputCount: 0,
    needsInputTaskIds: [],
    lastMerged: null,
    lastDiscarded: null,
    error: null,
    workbenchTabByTask: {},
    previewUrlByTask: {},
  }
}

function taskIndex(tasks: readonly TaskInfo[], taskId: string): number {
  return tasks.findIndex(task => task.task_id === taskId)
}

function upsertTask(tasks: readonly TaskInfo[], task: TaskInfo): TaskInfo[] {
  const index = taskIndex(tasks, task.task_id)
  if (index < 0) return [...tasks, task]
  return tasks.map((item, i) => (i === index ? task : item))
}

function placeholderTask(taskId: string, title: string, status: TaskStatus): TaskInfo {
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
    // 事件没有创建时间；占位值不参与排序，后续 task:updated 会补全。
    created_at: 0,
  }
}

/** 对指定任务做不可变补丁；任务不存在时原样返回。 */
function patchTask(
  state: TaskState,
  taskId: string,
  patch: (task: TaskInfo) => TaskInfo,
): TaskState {
  const index = taskIndex(state.tasks, taskId)
  if (index < 0) return state
  const tasks = state.tasks.map((task, i) => (i === index ? patch(task) : task))
  return { ...state, tasks }
}

/** 确保任务存在（事件可能先于 task_list/task:updated 到达）。 */
function ensureTask(state: TaskState, taskId: string, status: TaskStatus): TaskInfo[] {
  return taskIndex(state.tasks, taskId) < 0
    ? [...state.tasks, placeholderTask(taskId, taskId, status)]
    : state.tasks
}

/** 维护待确认集合；计数与 id 列表始终一致。 */
function withNeedsInput(state: TaskState, taskId: string, on: boolean): TaskState {
  const present = state.needsInputTaskIds.includes(taskId)
  if (present === on) return state
  const needsInputTaskIds = on
    ? [...state.needsInputTaskIds, taskId]
    : state.needsInputTaskIds.filter(id => id !== taskId)
  return {
    ...state,
    needsInputTaskIds,
    needsInputCount: needsInputTaskIds.length,
  }
}

function mergeStep(steps: readonly TaskStep[], step: TaskStep): TaskStep[] {
  const index = steps.findIndex(item => item.index === step.index)
  const next =
    index < 0
      ? [...steps, step]
      : steps.map((item, i) => (i === index ? step : item))
  return [...next].sort((a, b) => a.index - b.index)
}

function mergeChange(
  changes: readonly FileChangeDto[],
  change: FileChangeDto,
): FileChangeDto[] {
  const index = changes.findIndex(item => item.path === change.path)
  if (index < 0) return [...changes, change]
  return changes.map((item, i) => (i === index ? change : item))
}

function mergeCheck(checks: readonly TaskCheck[], check: TaskCheck): TaskCheck[] {
  const index = checks.findIndex(item => item.name === check.name)
  if (index < 0) return [...checks, check]
  return checks.map((item, i) => (i === index ? check : item))
}

/**
 * 应用一条核心事件。非 task:* 事件（agent/error/node/fs…）原样返回，
 * 便于同一事件流同时喂给多个 reducer。所有更新均为不可变更新。
 */
export function applyTaskEvent(state: TaskState, event: CoreEvent): TaskState {
  switch (event.type) {
    case 'task:created': {
      const index = taskIndex(state.tasks, event.task_id)
      if (index >= 0) {
        const tasks = state.tasks.map((task, i) =>
          i === index ? { ...task, title: event.title } : task,
        )
        return { ...state, tasks }
      }
      const task = placeholderTask(event.task_id, event.title, 'planning')
      return {
        ...state,
        tasks: [...state.tasks, task],
        activeTaskId: state.activeTaskId ?? event.task_id,
      }
    }

    case 'task:status': {
      // 契约里 status 是 string；未知状态直接丢弃，避免渲染层没有对应标签。
      if (!isTaskStatus(event.status)) return state
      const status = event.status
      const exists = taskIndex(state.tasks, event.task_id) >= 0
      const tasks = exists
        ? state.tasks.map(task =>
            task.task_id === event.task_id ? { ...task, status } : task,
          )
        : [...state.tasks, placeholderTask(event.task_id, event.task_id, status)]
      let next: TaskState = { ...state, tasks }
      if (status !== 'blocked') next = withNeedsInput(next, event.task_id, false)
      return next
    }

    case 'task:updated': {
      const tasks = upsertTask(state.tasks, event.task)
      const next: TaskState = {
        ...state,
        tasks,
        activeTaskId: state.activeTaskId ?? event.task.task_id,
      }
      // 任务恢复运行/进入验收时，之前的待确认标记随完整快照一起清掉。
      return event.task.status === 'blocked'
        ? next
        : withNeedsInput(next, event.task.task_id, false)
    }

    case 'task:step': {
      return patchTask(state, event.task_id, task => ({
        ...task,
        steps: mergeStep(task.steps, {
          index: event.index,
          title: event.title,
          state: event.state,
        }),
      }))
    }

    case 'task:change': {
      return patchTask(state, event.task_id, task => ({
        ...task,
        changes: mergeChange(task.changes, event.change),
      }))
    }

    case 'task:check': {
      return patchTask(state, event.task_id, task => ({
        ...task,
        checks: mergeCheck(task.checks, event.check),
      }))
    }

    case 'task:needs_input': {
      const tasks = ensureTask(state, event.task_id, 'blocked')
      return withNeedsInput({ ...state, tasks }, event.task_id, true)
    }

    case 'task:merged': {
      const next = patchTask(state, event.task_id, task => ({
        ...task,
        status: 'done',
        port: null,
        worktree_path: null,
        branch: null,
        changes: task.changes.map(change => ({ ...change, applied: true })),
      }))
      const cleared = withNeedsInput(next, event.task_id, false)
      return {
        ...clearWorkbenchPrefs(cleared, event.task_id),
        lastMerged: { taskId: event.task_id, commitSha: event.commit_sha },
      }
    }

    case 'task:discarded': {
      // Rust 侧丢弃后任务保留在列表里（status = failed），worktree 与变更清空。
      const next = patchTask(state, event.task_id, task => ({
        ...task,
        status: 'failed',
        port: null,
        worktree_path: null,
        branch: null,
        changes: [],
      }))
      const cleared = withNeedsInput(next, event.task_id, false)
      return {
        ...clearWorkbenchPrefs(cleared, event.task_id),
        lastDiscarded: event.task_id,
      }
    }

    default:
      return state
  }
}

/** 按 i18n key 返回状态标签；默认返回 key，方便服务层与测试无框架使用。 */
export type Translate = (key: string, params?: Record<string, unknown>) => string

export function taskStatusLabel(
  status: TaskStatus,
  translate: Translate = key => key,
): string {
  return translate(`task.statuses.${status}`)
}

export type TaskTone = 'muted' | 'active' | 'warn' | 'accent' | 'success' | 'danger'

/** 状态徽标色调；视图层映射到 CSS class（tone-*）。 */
export function statusTone(status: TaskStatus): TaskTone {
  switch (status) {
    case 'planning':
      return 'muted'
    case 'running':
      return 'active'
    case 'blocked':
      return 'warn'
    case 'review':
      return 'accent'
    case 'done':
      return 'success'
    case 'failed':
      return 'danger'
    case 'queued':
      return 'muted'
  }
}

/** 排队原因文案（docs/TASKS.md §4：并发上限 2，UI 必须明示原因）。 */
export function queuedReason(
  translate: Translate,
  maxRunning: number = MAX_RUNNING_TASKS,
): string {
  return translate('task.queuedReason', { max: maxRunning })
}

/** 状态栏跳转目标：第一个仍存在且需要输入的 task id。 */
export function firstNeedsInputTaskId(state: TaskState): string | null {
  for (const id of state.needsInputTaskIds) {
    if (taskIndex(state.tasks, id) >= 0) return id
  }
  return null
}

export function hasNeedsInput(state: TaskState, taskId: string): boolean {
  return state.needsInputTaskIds.includes(taskId)
}

/** 选择任务；未知 id 与原状态一致地忽略。 */
export function selectTask(state: TaskState, taskId: string | null): TaskState {
  if (taskId === state.activeTaskId) return state
  if (taskId !== null && taskIndex(state.tasks, taskId) < 0) return state
  return { ...state, activeTaskId: taskId }
}

// ==== 工作台标签与预览（按任务记忆，P4 出口 #4）====

function hasTask(state: TaskState, taskId: string): boolean {
  return taskIndex(state.tasks, taskId) >= 0
}

/** 记住任务的工作台标签；未知任务不落记录，避免产生孤儿条目。 */
export function setWorkbenchTab(
  state: TaskState,
  taskId: string,
  tab: WorkbenchTab,
): TaskState {
  if (!hasTask(state, taskId) || state.workbenchTabByTask[taskId] === tab) {
    return state
  }
  return {
    ...state,
    workbenchTabByTask: { ...state.workbenchTabByTask, [taskId]: tab },
  }
}

/** 记住任务的预览 URL；空白视为清除，避免 iframe 停留在无效地址。 */
export function setPreviewUrl(state: TaskState, taskId: string, url: string): TaskState {
  if (!hasTask(state, taskId)) return state
  const next = url.trim()
  if (next.length === 0) {
    if (!(taskId in state.previewUrlByTask)) return state
    const previewUrlByTask = { ...state.previewUrlByTask }
    delete previewUrlByTask[taskId]
    return { ...state, previewUrlByTask }
  }
  if (state.previewUrlByTask[taskId] === next) return state
  return {
    ...state,
    previewUrlByTask: { ...state.previewUrlByTask, [taskId]: next },
  }
}

/** 激活任务的标签；没有记录（含未选择任务）时回落到默认标签。 */
export function activeWorkbenchTab(state: TaskState): WorkbenchTab {
  if (state.activeTaskId === null) return DEFAULT_WORKBENCH_TAB
  return state.workbenchTabByTask[state.activeTaskId] ?? DEFAULT_WORKBENCH_TAB
}

/** 激活任务的预览 URL；没有记录时返回空串，视图层显示占位提示。 */
export function activePreviewUrl(state: TaskState): string {
  if (state.activeTaskId === null) return ''
  return state.previewUrlByTask[state.activeTaskId] ?? ''
}

/** 任务结束（合并/丢弃）后清除其标签与预览记录，避免长期运行时泄漏。 */
export function clearWorkbenchPrefs(state: TaskState, taskId: string): TaskState {
  const hasTab = taskId in state.workbenchTabByTask
  const hasUrl = taskId in state.previewUrlByTask
  if (!hasTab && !hasUrl) return state
  const workbenchTabByTask = { ...state.workbenchTabByTask }
  const previewUrlByTask = { ...state.previewUrlByTask }
  delete workbenchTabByTask[taskId]
  delete previewUrlByTask[taskId]
  return { ...state, workbenchTabByTask, previewUrlByTask }
}

// ==== 控制器：命令 + 事件流 → store ====

export interface TaskControllerDeps {
  taskList: () => Promise<BridgeResult<TaskListResult>>
  taskCreate: (
    title: string,
    sessionId: string | null,
  ) => Promise<BridgeResult<TaskResult>>
  taskStart: (taskId: string) => Promise<BridgeResult<TaskResult>>
  taskCancel: (taskId: string) => Promise<BridgeResult<TaskResult>>
  taskApply: (taskId: string) => Promise<BridgeResult<TaskResult>>
  taskDiscard: (taskId: string) => Promise<BridgeResult<AckResult>>
  taskDiff: (taskId: string) => Promise<BridgeResult<TaskDiffResult>>
  taskBindSession: (
    taskId: string,
    sessionId: string | null,
  ) => Promise<BridgeResult<TaskResult>>
  onCoreEvent: (handler: (event: CoreEvent) => void) => () => void
}

export interface TaskController {
  getState(): TaskState
  subscribe(listener: (state: TaskState) => void): () => void
  refresh(): Promise<boolean>
  create(title: string, sessionId?: string | null): Promise<TaskInfo | null>
  start(taskId: string): Promise<boolean>
  cancel(taskId: string): Promise<boolean>
  apply(taskId: string): Promise<boolean>
  discard(taskId: string): Promise<boolean>
  diff(taskId: string): Promise<BridgeResult<TaskDiffResult>>
  bindSession(taskId: string, sessionId: string | null): Promise<boolean>
  setActive(taskId: string | null): void
  /** 记住任务的工作台标签（切换任务时恢复）。 */
  setWorkbenchTab(taskId: string, tab: WorkbenchTab): void
  /** 记住任务的预览 URL；空串清除记录。 */
  setPreviewUrl(taskId: string, url: string): void
  /** 状态栏点击：切到第一个 needs_input 任务并返回其 id。 */
  activateFirstNeedsInput(): string | null
  clearError(): void
  dispose(): void
}

function errorOf(error: { code: ErrorCode; message: string }): TaskState['error'] {
  return { code: error.code, message: error.message }
}

export function createTaskController(deps: TaskControllerDeps): TaskController {
  let state = createTaskState()
  const listeners = new Set<(state: TaskState) => void>()
  let disposed = false

  function commit(next: TaskState): void {
    state = next
    if (disposed) return
    for (const listener of [...listeners]) listener(state)
  }

  function patch(next: Partial<TaskState>): void {
    commit({ ...state, ...next })
  }

  const offCore = deps.onCoreEvent(event => {
    if (disposed) return
    const next = applyTaskEvent(state, event)
    if (next !== state) commit(next)
  })

  async function refresh(): Promise<boolean> {
    if (disposed) return false
    const res = await deps.taskList()
    if (disposed) return false
    if (!res.ok) {
      patch({ error: errorOf(res.error) })
      return false
    }
    const tasks = res.data.tasks
    const activeTaskId =
      state.activeTaskId !== null && taskIndex(tasks, state.activeTaskId) >= 0
        ? state.activeTaskId
        : (tasks[0]?.task_id ?? null)
    const needsInputTaskIds = state.needsInputTaskIds.filter(
      id => taskIndex(tasks, id) >= 0,
    )
    commit({
      ...state,
      tasks,
      activeTaskId,
      needsInputTaskIds,
      needsInputCount: needsInputTaskIds.length,
      error: null,
    })
    return true
  }

  async function create(
    title: string,
    sessionId: string | null = null,
  ): Promise<TaskInfo | null> {
    const trimmed = title.trim()
    if (disposed || trimmed.length === 0) return null
    const res = await deps.taskCreate(trimmed, sessionId)
    if (disposed) return null
    if (!res.ok) {
      patch({ error: errorOf(res.error) })
      return null
    }
    const task = res.data.task
    commit({
      ...state,
      tasks: upsertTask(state.tasks, task),
      activeTaskId: task.task_id,
      error: null,
    })
    return task
  }

  function acceptTask(res: BridgeResult<TaskResult>): boolean {
    if (disposed) return false
    if (!res.ok) {
      patch({ error: errorOf(res.error) })
      return false
    }
    commit({
      ...state,
      tasks: upsertTask(state.tasks, res.data.task),
      error: null,
    })
    return true
  }

  async function start(taskId: string): Promise<boolean> {
    if (disposed) return false
    return acceptTask(await deps.taskStart(taskId))
  }

  async function cancel(taskId: string): Promise<boolean> {
    if (disposed) return false
    return acceptTask(await deps.taskCancel(taskId))
  }

  async function apply(taskId: string): Promise<boolean> {
    if (disposed) return false
    const res = await deps.taskApply(taskId)
    if (!acceptTask(res)) return false
    // 合并即任务结束：清掉该任务的标签/预览记录，避免 iframe 停留在已结束任务。
    commit(clearWorkbenchPrefs(state, taskId))
    // 合并后主工作区/基线变化，Rust 会重写任务状态与变更 applied 标记。
    // 浏览器 fallback 没有真实列表（返回空），此时保留本地结果用于演示。
    if (!res.fallback) await refresh()
    return true
  }

  async function discard(taskId: string): Promise<boolean> {
    if (disposed) return false
    const res = await deps.taskDiscard(taskId)
    if (disposed) return false
    if (!res.ok) {
      patch({ error: errorOf(res.error) })
      return false
    }
    // 契约 ack 不带 TaskInfo：按 Rust discard 语义标记失败并清空 worktree 字段。
    const tasks = state.tasks.map(task =>
      task.task_id === taskId
        ? {
            ...task,
            status: 'failed' as const,
            port: null,
            worktree_path: null,
            branch: null,
            changes: [],
          }
        : task,
    )
    const cleared = withNeedsInput(
      { ...state, tasks, lastDiscarded: taskId, error: null },
      taskId,
      false,
    )
    commit(clearWorkbenchPrefs(cleared, taskId))
    return true
  }

  async function diff(taskId: string): Promise<BridgeResult<TaskDiffResult>> {
    if (disposed) {
      return {
        ok: false,
        error: { code: 'internal', message: 'task controller disposed', detail: null },
        fallback: false,
      }
    }
    const res = await deps.taskDiff(taskId)
    if (disposed) return res
    if (!res.ok) patch({ error: errorOf(res.error) })
    else if (state.error) patch({ error: null })
    return res
  }

  async function bindSession(
    taskId: string,
    sessionId: string | null,
  ): Promise<boolean> {
    if (disposed) return false
    return acceptTask(await deps.taskBindSession(taskId, sessionId))
  }

  function setActive(taskId: string | null): void {
    if (disposed) return
    const next = selectTask(state, taskId)
    if (next !== state) commit(next)
  }

  function setWorkbenchTabForTask(taskId: string, tab: WorkbenchTab): void {
    if (disposed) return
    const next = setWorkbenchTab(state, taskId, tab)
    if (next !== state) commit(next)
  }

  function setPreviewUrlForTask(taskId: string, url: string): void {
    if (disposed) return
    const next = setPreviewUrl(state, taskId, url)
    if (next !== state) commit(next)
  }

  function activateFirstNeedsInput(): string | null {
    if (disposed) return null
    const taskId = firstNeedsInputTaskId(state)
    if (taskId === null) return null
    setActive(taskId)
    return taskId
  }

  function clearError(): void {
    if (disposed || state.error === null) return
    patch({ error: null })
  }

  function subscribe(listener: (state: TaskState) => void): () => void {
    listeners.add(listener)
    return () => {
      listeners.delete(listener)
    }
  }

  function dispose(): void {
    if (disposed) return
    disposed = true
    offCore()
    listeners.clear()
  }

  return {
    getState: () => state,
    subscribe,
    refresh,
    create,
    start,
    cancel,
    apply,
    discard,
    diff,
    bindSession,
    setActive,
    setWorkbenchTab: setWorkbenchTabForTask,
    setPreviewUrl: setPreviewUrlForTask,
    activateFirstNeedsInput,
    clearError,
    dispose,
  }
}
