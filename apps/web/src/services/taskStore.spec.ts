import { describe, expect, it, vi } from 'vitest'

import type { CoreEvent, ErrorCode } from '@lc-proto'

import type {
  AckResult,
  BridgeResult,
  TaskDiffResult,
  TaskListResult,
  TaskResult,
} from './bridge'
import {
  applyTaskEvent,
  createTaskController,
  createTaskState,
  firstNeedsInputTaskId,
  hasNeedsInput,
  MAX_RUNNING_TASKS,
  queuedReason,
  selectTask,
  statusTone,
  taskStatusLabel,
  type TaskControllerDeps,
} from './taskStore'
import type { TaskInfo } from './taskTypes'

function task(overrides: Partial<TaskInfo> = {}): TaskInfo {
  return {
    task_id: 't1',
    title: 'Fix login',
    session_id: null,
    status: 'planning',
    worktree_path: null,
    branch: null,
    port: null,
    base_sha: null,
    steps: [],
    changes: [],
    checks: [],
    created_at: 1,
    ...overrides,
  }
}

function okList(tasks: TaskInfo[]): BridgeResult<TaskListResult> {
  return { ok: true, data: { kind: 'tasks', tasks }, fallback: false }
}

function okTask(value: TaskInfo): BridgeResult<TaskResult> {
  return { ok: true, data: { kind: 'task', task: value }, fallback: false }
}

function okAck(): BridgeResult<AckResult> {
  return { ok: true, data: { kind: 'ack' }, fallback: false }
}

function okDiff(taskId: string): BridgeResult<TaskDiffResult> {
  return {
    ok: true,
    data: {
      kind: 'task_diff',
      task_id: taskId,
      patch: '--- a\n+++ b\n',
      files: [],
    },
    fallback: false,
  }
}

function fail<T>(message = 'boom', code: ErrorCode = 'internal'): BridgeResult<T> {
  return { ok: false, error: { code, message, detail: null }, fallback: false }
}

describe('applyTaskEvent', () => {
  it('task:created 新增 planning 占位并激活第一个任务', () => {
    const state = applyTaskEvent(createTaskState(), {
      type: 'task:created',
      task_id: 't1',
      title: 'Fix login',
    })
    expect(state.tasks).toHaveLength(1)
    expect(state.tasks[0]).toMatchObject({
      task_id: 't1',
      title: 'Fix login',
      status: 'planning',
      session_id: null,
      steps: [],
      changes: [],
      checks: [],
    })
    expect(state.activeTaskId).toBe('t1')
  })

  it('task:created 对已存在任务只合并 title，不覆盖状态', () => {
    const initial = applyTaskEvent(createTaskState(), {
      type: 'task:updated',
      task: task({ status: 'review', title: 'Old' }),
    })
    const state = applyTaskEvent(initial, {
      type: 'task:created',
      task_id: 't1',
      title: 'New title',
    })
    expect(state.tasks).toHaveLength(1)
    expect(state.tasks[0]).toMatchObject({ title: 'New title', status: 'review' })
  })

  it('task:status 更新已有任务并清理待确认', () => {
    let state = applyTaskEvent(createTaskState(), {
      type: 'task:updated',
      task: task({ status: 'blocked' }),
    })
    state = applyTaskEvent(state, {
      type: 'task:needs_input',
      task_id: 't1',
      kind: 'approval',
    })
    expect(state.needsInputCount).toBe(1)

    state = applyTaskEvent(state, { type: 'task:status', task_id: 't1', status: 'running' })
    expect(state.tasks[0].status).toBe('running')
    expect(state.needsInputCount).toBe(0)
    expect(state.needsInputTaskIds).toEqual([])
  })

  it('task:status 未知任务先建占位，未知状态原样返回', () => {
    const state = applyTaskEvent(createTaskState(), {
      type: 'task:status',
      task_id: 't9',
      status: 'queued',
    })
    expect(state.tasks[0]).toMatchObject({ task_id: 't9', title: 't9', status: 'queued' })

    const unchanged = applyTaskEvent(state, {
      type: 'task:status',
      task_id: 't9',
      status: 'weird',
    })
    expect(unchanged).toBe(state)
  })

  it('task:updated 按 task_id 替换而非追加', () => {
    const initial = applyTaskEvent(createTaskState(), {
      type: 'task:updated',
      task: task({ title: 'Old' }),
    })
    const state = applyTaskEvent(initial, {
      type: 'task:updated',
      task: task({ title: 'New', status: 'review' }),
    })
    expect(state.tasks).toHaveLength(1)
    expect(state.tasks[0]).toMatchObject({ title: 'New', status: 'review' })
  })

  it('task:step 按 index 增量更新并保持有序', () => {
    let state = applyTaskEvent(createTaskState(), {
      type: 'task:updated',
      task: task(),
    })
    state = applyTaskEvent(state, {
      type: 'task:step',
      task_id: 't1',
      index: 1,
      title: 'Edit',
      state: 'pending',
    })
    state = applyTaskEvent(state, {
      type: 'task:step',
      task_id: 't1',
      index: 0,
      title: 'Plan',
      state: 'done',
    })
    state = applyTaskEvent(state, {
      type: 'task:step',
      task_id: 't1',
      index: 1,
      title: 'Edit',
      state: 'running',
    })

    expect(state.tasks[0].steps).toEqual([
      { index: 0, title: 'Plan', state: 'done' },
      { index: 1, title: 'Edit', state: 'running' },
    ])
  })

  it('task:step 对未知任务原样返回', () => {
    const state = applyTaskEvent(createTaskState(), {
      type: 'task:step',
      task_id: 'missing',
      index: 0,
      title: 'Plan',
      state: 'pending',
    })
    expect(state.tasks).toEqual([])
  })

  it('task:change 同路径替换、不同路径追加', () => {
    let state = applyTaskEvent(createTaskState(), {
      type: 'task:updated',
      task: task(),
    })
    const modified = {
      path: 'src/a.ts',
      status: 'modified' as const,
      applied: false,
      added: 2,
      removed: 1,
    }
    state = applyTaskEvent(state, { type: 'task:change', task_id: 't1', change: modified })
    state = applyTaskEvent(state, {
      type: 'task:change',
      task_id: 't1',
      change: { ...modified, added: 9 },
    })
    state = applyTaskEvent(state, {
      type: 'task:change',
      task_id: 't1',
      change: { path: 'src/b.ts', status: 'added', applied: false, added: 1, removed: 0 },
    })

    expect(state.tasks[0].changes).toHaveLength(2)
    expect(state.tasks[0].changes[0]).toMatchObject({ path: 'src/a.ts', added: 9 })
    expect(state.tasks[0].changes[1].path).toBe('src/b.ts')
  })

  it('task:check 同名替换、新检查追加', () => {
    let state = applyTaskEvent(createTaskState(), {
      type: 'task:updated',
      task: task(),
    })
    state = applyTaskEvent(state, {
      type: 'task:check',
      task_id: 't1',
      check: { name: 'unit', ok: false, output_tail: 'boom', duration_ms: 10 },
    })
    state = applyTaskEvent(state, {
      type: 'task:check',
      task_id: 't1',
      check: { name: 'unit', ok: true, output_tail: '', duration_ms: 12 },
    })
    state = applyTaskEvent(state, {
      type: 'task:check',
      task_id: 't1',
      check: { name: 'lint', ok: true, output_tail: '', duration_ms: 4 },
    })

    expect(state.tasks[0].checks).toEqual([
      { name: 'unit', ok: true, output_tail: '', duration_ms: 12 },
      { name: 'lint', ok: true, output_tail: '', duration_ms: 4 },
    ])
  })

  it('task:needs_input 按任务去重计数；重复事件不重复计数', () => {
    let state = applyTaskEvent(createTaskState(), {
      type: 'task:updated',
      task: task({ task_id: 't1' }),
    })
    state = applyTaskEvent(state, {
      type: 'task:updated',
      task: task({ task_id: 't2', title: 'Second' }),
    })
    state = applyTaskEvent(state, { type: 'task:needs_input', task_id: 't1', kind: 'approval' })
    state = applyTaskEvent(state, { type: 'task:needs_input', task_id: 't1', kind: 'approval' })
    state = applyTaskEvent(state, { type: 'task:needs_input', task_id: 't2', kind: 'plan' })

    expect(state.needsInputTaskIds).toEqual(['t1', 't2'])
    expect(state.needsInputCount).toBe(2)
    expect(firstNeedsInputTaskId(state)).toBe('t1')
    expect(hasNeedsInput(state, 't2')).toBe(true)
  })

  it('task:merged 标记 done、应用变更、记录合并提交并清理待确认', () => {
    let state = applyTaskEvent(createTaskState(), {
      type: 'task:updated',
      task: task({
        status: 'review',
        worktree_path: 'D:/wt/t1',
        branch: 'lc/t1-fix',
        port: 4300,
        changes: [
          { path: 'a.ts', status: 'modified', applied: false, added: 1, removed: 0 },
        ],
      }),
    })
    state = applyTaskEvent(state, { type: 'task:needs_input', task_id: 't1', kind: 'approval' })
    state = applyTaskEvent(state, {
      type: 'task:merged',
      task_id: 't1',
      commit_sha: 'abc123',
    })

    expect(state.tasks[0]).toMatchObject({
      status: 'done',
      worktree_path: null,
      branch: null,
      port: null,
    })
    expect(state.tasks[0].changes[0].applied).toBe(true)
    expect(state.lastMerged).toEqual({ taskId: 't1', commitSha: 'abc123' })
    expect(state.needsInputCount).toBe(0)
  })

  it('task:discarded 标记 failed、清空 worktree/变更并记录', () => {
    let state = applyTaskEvent(createTaskState(), {
      type: 'task:updated',
      task: task({
        status: 'review',
        worktree_path: 'D:/wt/t1',
        branch: 'lc/t1-fix',
        port: 4300,
        changes: [
          { path: 'a.ts', status: 'modified', applied: false, added: 1, removed: 0 },
        ],
      }),
    })
    state = applyTaskEvent(state, { type: 'task:needs_input', task_id: 't1', kind: 'approval' })
    state = applyTaskEvent(state, { type: 'task:discarded', task_id: 't1' })

    expect(state.tasks[0]).toMatchObject({
      status: 'failed',
      worktree_path: null,
      branch: null,
      port: null,
      changes: [],
    })
    expect(state.lastDiscarded).toBe('t1')
    expect(state.needsInputCount).toBe(0)
  })

  it('非 task 事件原样返回（引用不变）', () => {
    const state = createTaskState()
    const agentEvent: CoreEvent = {
      type: 'agent:usage',
      session_id: 's1',
      input: 1,
      output: 2,
      cached: 0,
    }
    expect(applyTaskEvent(state, agentEvent)).toBe(state)
    expect(
      applyTaskEvent(state, { type: 'error', error: { code: 'io', message: 'boom' } }),
    ).toBe(state)
  })
})

describe('纯函数', () => {
  it('taskStatusLabel 默认返回 i18n key，可注入翻译函数', () => {
    expect(taskStatusLabel('queued')).toBe('task.statuses.queued')
    expect(taskStatusLabel('review', (key, params) => `${key}:${params?.x ?? ''}`)).toBe(
      'task.statuses.review:',
    )
  })

  it('statusTone 覆盖全部状态', () => {
    expect(statusTone('planning')).toBe('muted')
    expect(statusTone('running')).toBe('active')
    expect(statusTone('blocked')).toBe('warn')
    expect(statusTone('review')).toBe('accent')
    expect(statusTone('done')).toBe('success')
    expect(statusTone('failed')).toBe('danger')
    expect(statusTone('queued')).toBe('muted')
  })

  it('queuedReason 带并发上限，默认 2', () => {
    expect(MAX_RUNNING_TASKS).toBe(2)
    expect(queuedReason((key, params) => `${key}:${params?.max}`)).toBe(
      'task.queuedReason:2',
    )
    expect(queuedReason((key, params) => `${key}:${params?.max}`, 4)).toBe(
      'task.queuedReason:4',
    )
  })

  it('selectTask 忽略未知 id，null 可清空选择', () => {
    let state = applyTaskEvent(createTaskState(), {
      type: 'task:updated',
      task: task(),
    })
    expect(selectTask(state, 'nope')).toBe(state)
    state = selectTask(state, 't1')
    expect(state.activeTaskId).toBe('t1')
    state = selectTask(state, null)
    expect(state.activeTaskId).toBeNull()
  })
})

interface HarnessOptions {
  taskList?: TaskControllerDeps['taskList']
  taskCreate?: TaskControllerDeps['taskCreate']
  taskStart?: TaskControllerDeps['taskStart']
  taskCancel?: TaskControllerDeps['taskCancel']
  taskApply?: TaskControllerDeps['taskApply']
  taskDiscard?: TaskControllerDeps['taskDiscard']
  taskDiff?: TaskControllerDeps['taskDiff']
  taskBindSession?: TaskControllerDeps['taskBindSession']
}

function harness(options: HarnessOptions = {}) {
  const events: Array<(event: CoreEvent) => void> = []
  const offCore = vi.fn()
  const deps: TaskControllerDeps = {
    taskList: vi.fn(async () => okList([])),
    taskCreate: vi.fn(async (title: string, sessionId: string | null) =>
      okTask(task({ task_id: 't-new', title, session_id: sessionId })),
    ),
    taskStart: vi.fn(async (taskId: string) =>
      okTask(task({ task_id: taskId, status: 'running' })),
    ),
    taskCancel: vi.fn(async (taskId: string) =>
      okTask(task({ task_id: taskId, status: 'failed' })),
    ),
    taskApply: vi.fn(async (taskId: string) =>
      okTask(task({ task_id: taskId, status: 'done' })),
    ),
    taskDiscard: vi.fn(async () => okAck()),
    taskDiff: vi.fn(async (taskId: string) => okDiff(taskId)),
    taskBindSession: vi.fn(async (taskId: string, sessionId: string | null) =>
      okTask(task({ task_id: taskId, session_id: sessionId })),
    ),
    onCoreEvent: (handler: (event: CoreEvent) => void) => {
      events.push(handler)
      return offCore
    },
    ...options,
  }
  return {
    controller: createTaskController(deps),
    deps,
    offCore,
    emit: (event: CoreEvent) => {
      for (const handler of events) handler(event)
    },
  }
}

describe('TaskController', () => {
  it('refresh 填充列表并默认选中第一个任务', async () => {
    const { controller, deps } = harness({
      taskList: vi.fn(async () => okList([task({ task_id: 't1' }), task({ task_id: 't2' })])),
    })
    expect(await controller.refresh()).toBe(true)
    expect(deps.taskList).toHaveBeenCalledTimes(1)
    expect(controller.getState().tasks.map(item => item.task_id)).toEqual(['t1', 't2'])
    expect(controller.getState().activeTaskId).toBe('t1')
  })

  it('refresh 失败写入 error，保留已有任务', async () => {
    const { controller } = harness({
      taskList: vi.fn(async () => fail<TaskListResult>('list failed', 'io')),
    })
    expect(await controller.refresh()).toBe(false)
    expect(controller.getState().error).toEqual({ code: 'io', message: 'list failed' })
  })

  it('create 传 title/session_id，成功后加入列表并激活；空标题不发命令', async () => {
    const { controller, deps } = harness()
    const created = await controller.create('  Fix login  ', 's1')
    expect(created?.title).toBe('Fix login')
    expect(deps.taskCreate).toHaveBeenCalledWith('Fix login', 's1')
    expect(controller.getState().tasks.map(item => item.task_id)).toEqual(['t-new'])
    expect(controller.getState().activeTaskId).toBe('t-new')

    expect(await controller.create('   ')).toBeNull()
    expect(deps.taskCreate).toHaveBeenCalledTimes(1)
  })

  it('create 失败写入 error 并返回 null', async () => {
    const { controller } = harness({
      taskCreate: vi.fn(async () => fail<TaskResult>('no git', 'invalid_argument')),
    })
    expect(await controller.create('Fix')).toBeNull()
    expect(controller.getState().error).toEqual({
      code: 'invalid_argument',
      message: 'no git',
    })
  })

  it('start/cancel 用命令结果更新任务', async () => {
    const { controller, deps } = harness()
    expect(await controller.start('t1')).toBe(true)
    expect(deps.taskStart).toHaveBeenCalledWith('t1')
    expect(controller.getState().tasks[0].status).toBe('running')

    expect(await controller.cancel('t1')).toBe(true)
    expect(deps.taskCancel).toHaveBeenCalledWith('t1')
    expect(controller.getState().tasks[0].status).toBe('failed')
  })

  it('start 失败保留任务并写入 error', async () => {
    const { controller } = harness({
      taskStart: vi.fn(async () => fail<TaskResult>('busy')),
    })
    expect(await controller.start('t1')).toBe(false)
    expect(controller.getState().tasks).toEqual([])
    expect(controller.getState().error?.message).toBe('busy')
  })

  it('apply 成功后刷新列表（非 fallback）', async () => {
    const done = task({ task_id: 't1', status: 'done' })
    const list = vi.fn(async () => okList([done, task({ task_id: 't2', status: 'review' })]))
    const { controller, deps } = harness({
      taskApply: vi.fn(async () => okTask(done)),
      taskList: list,
    })
    expect(await controller.apply('t1')).toBe(true)
    expect(deps.taskApply).toHaveBeenCalledWith('t1')
    expect(list).toHaveBeenCalledTimes(1)
    expect(controller.getState().tasks.map(item => item.task_id)).toEqual(['t1', 't2'])
    expect(controller.getState().activeTaskId).toBe('t1')
  })

  it('apply 浏览器 fallback 不刷新（避免空列表清掉演示任务）', async () => {
    const list = vi.fn(async () => okList([]))
    const { controller } = harness({
      taskApply: vi.fn(async () => ({
        ok: true as const,
        data: { kind: 'task' as const, task: task({ task_id: 't1', status: 'done' }) },
        fallback: true,
      })),
      taskList: list,
    })
    expect(await controller.apply('t1')).toBe(true)
    expect(list).not.toHaveBeenCalled()
    expect(controller.getState().tasks[0].status).toBe('done')
  })

  it('discard 标记 failed、清空 worktree/变更并清理待确认', async () => {
    const { controller, deps, emit } = harness({
      taskList: vi.fn(async () =>
        okList([
          task({
            task_id: 't1',
            status: 'review',
            worktree_path: 'D:/wt/t1',
            branch: 'lc/t1',
            port: 4300,
            changes: [
              { path: 'a.ts', status: 'modified', applied: false, added: 1, removed: 0 },
            ],
          }),
        ]),
      ),
    })
    await controller.refresh()
    emit({ type: 'task:needs_input', task_id: 't1', kind: 'approval' })
    expect(controller.getState().needsInputCount).toBe(1)

    expect(await controller.discard('t1')).toBe(true)
    expect(deps.taskDiscard).toHaveBeenCalledWith('t1')
    expect(controller.getState().tasks[0]).toMatchObject({
      status: 'failed',
      worktree_path: null,
      branch: null,
      port: null,
      changes: [],
    })
    expect(controller.getState().lastDiscarded).toBe('t1')
    expect(controller.getState().needsInputCount).toBe(0)
  })

  it('diff 成功返回载荷，失败写入 error', async () => {
    const { controller } = harness()
    const ok = await controller.diff('t1')
    expect(ok.ok).toBe(true)
    if (ok.ok) expect(ok.data.kind).toBe('task_diff')
    expect(controller.getState().error).toBeNull()

    const failing = harness({ taskDiff: vi.fn(async () => fail<TaskDiffResult>('no diff')) })
    const bad = await failing.controller.diff('t1')
    expect(bad.ok).toBe(false)
    expect(failing.controller.getState().error?.message).toBe('no diff')
  })

  it('bindSession 用命令结果更新 session_id', async () => {
    const { controller, deps } = harness()
    expect(await controller.bindSession('t1', 's5')).toBe(true)
    expect(deps.taskBindSession).toHaveBeenCalledWith('t1', 's5')
    expect(controller.getState().tasks[0].session_id).toBe('s5')
  })

  it('setActive 忽略未知 id', async () => {
    const { controller } = harness({
      taskList: vi.fn(async () => okList([task({ task_id: 't1' }), task({ task_id: 't2' })])),
    })
    await controller.refresh()
    controller.setActive('nope')
    expect(controller.getState().activeTaskId).toBe('t1')
    controller.setActive('t2')
    expect(controller.getState().activeTaskId).toBe('t2')
    controller.setActive(null)
    expect(controller.getState().activeTaskId).toBeNull()
  })

  it('activateFirstNeedsInput 切到第一个 needs_input 任务', async () => {
    const { controller, emit } = harness({
      taskList: vi.fn(async () => okList([task({ task_id: 't1' }), task({ task_id: 't2' })])),
    })
    await controller.refresh()
    emit({ type: 'task:needs_input', task_id: 't2', kind: 'approval' })
    emit({ type: 'task:needs_input', task_id: 't1', kind: 'plan' })
    expect(controller.getState().needsInputCount).toBe(2)
    expect(controller.activateFirstNeedsInput()).toBe('t2')
    expect(controller.getState().activeTaskId).toBe('t2')
  })

  it('事件流驱动状态；取消订阅后不再收到更新', async () => {
    const { controller, emit } = harness()
    const seen: number[] = []
    const unsubscribe = controller.subscribe(state => seen.push(state.tasks.length))
    emit({ type: 'task:created', task_id: 't1', title: 'Fix' })
    expect(seen).toEqual([1])
    unsubscribe()
    emit({ type: 'task:created', task_id: 't2', title: 'Second' })
    expect(seen).toEqual([1])
    expect(controller.getState().tasks).toHaveLength(2)
  })

  it('dispose 解绑事件并清空监听；之后命令不再改状态', async () => {
    const { controller, emit, offCore } = harness()
    controller.dispose()
    expect(offCore).toHaveBeenCalledTimes(1)

    emit({ type: 'task:created', task_id: 't1', title: 'Fix' })
    expect(controller.getState().tasks).toEqual([])
    expect(await controller.refresh()).toBe(false)
    expect(await controller.create('Fix')).toBeNull()
  })
})
