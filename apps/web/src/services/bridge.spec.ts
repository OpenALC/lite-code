import { afterEach, describe, expect, it, vi } from 'vitest'

import type { CommandResult, CoreEvent, SessionInfo } from '@lc-proto'

import {
  agentCancel,
  agentSend,
  CORE_EVENT_CHANNEL,
  createEntry,
  dbConnect,
  dbDeleteConnection,
  deletePath,
  movePath,
  renamePath,
  revealPath,
  dbDisconnect,
  dbExecute,
  dbExplain,
  dbList,
  dbQuery,
  dbSaveConnection,
  dbSchema,
  detectNode,
  downloadNodePortable,
  gitCommit,
  gitDiff,
  gitLog,
  gitStage,
  gitStatus,
  gitUnstage,
  isTauri,
  onCoreEvent,
  openWorkspace,
  providersLoad,
  providersSave,
  readFile,
  readDir,
  resolveApproval,
  searchWorkspace,
  sessionCreate,
  setNodeManual,
  taskApply,
  taskBindSession,
  taskCancel,
  taskCreate,
  taskDiff,
  taskDiscard,
  taskList,
  taskStart,
  workspaceRoot,
  writeFile,
  type BridgeResult,
  type DbConnectionResult,
  type DbExecCommandResult,
  type DbExplainCommandResult,
  type DbListResult,
  type DbQueryCommandResult,
  type DbSchemaResult,
  type DirListingResult,
  type GitDiffResult,
  type GitLogResult,
  type GitStatusResult,
  type TaskDiffResult,
  type TaskListResult,
} from './bridge'
import type {
  DbConnectionConfig,
  DbConnectionInfo,
  DbQueryResult,
  UiCoreEvent,
} from './dbTypes'
import type { TaskInfo } from './taskTypes'

// node 环境无 window：默认走 fallback 占位路径；Tauri 路径用注入的假
// __TAURI_INTERNALS__ 驱动（invoke 的入参与 reject 载荷形状都在这里断言）。

interface FakeInternals {
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>
  transformCallback?: (callback: (response: unknown) => void, once?: boolean) => number
}

function installTauri(fake: FakeInternals): void {
  ;(globalThis as unknown as { window: unknown }).window = {
    __TAURI_INTERNALS__: fake,
  }
}

function removeWindow(): void {
  delete (globalThis as unknown as { window?: unknown }).window
}

afterEach(removeWindow)

describe('isTauri', () => {
  it('无 window（node 测试环境）返回 false', () => {
    expect(isTauri()).toBe(false)
  })

  it('有 __TAURI_INTERNALS__.invoke 返回 true', () => {
    installTauri({ invoke: async () => ({}) })
    expect(isTauri()).toBe(true)
  })

  it('internals 缺 invoke 方法视为非 Tauri', () => {
    ;(globalThis as unknown as { window: unknown }).window = { __TAURI_INTERNALS__: {} }
    expect(isTauri()).toBe(false)
  })
})

describe('fallback 占位（浏览器 dev）', () => {
  it('readDir 返回带 fallback 标注的 dir_listing', async () => {
    const res = await readDir('', '')
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) {
      expect(res.data.kind).toBe('dir_listing')
      expect(res.data.entries.length).toBeGreaterThan(0)
      expect(res.data.generation).toBe(0)
    }
  })

  it('readDir 按 root 前缀归一层键，未知层为空目录', async () => {
    const nested = await readDir('D:/proj', 'D:/proj/docs')
    if (nested.ok) expect(nested.data.entries.map(e => e.name)).toContain('ARCHITECTURE.md')
    const unknown = await readDir('D:/proj', 'D:/proj/nope')
    if (unknown.ok) expect(unknown.data.entries).toEqual([])
  })

  it('readFile 返回标注占位的文本', async () => {
    const res = await readFile('', 'README.md')
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) {
      expect(res.data.kind).toBe('file_content')
      expect(res.data.text).toContain('占位')
    }
  })

  it('workspaceRoot 返回 null + fallback（无真实工作区）', async () => {
    const res = await workspaceRoot()
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) expect(res.data).toEqual({ kind: 'workspace_root', root: null })
  })

  it('openWorkspace 在浏览器 dev 返回 fallback 根', async () => {
    const res = await openWorkspace('D:/proj')
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) expect(res.data.root).toBe('D:/proj')
  })

  it('writeFile 返回 ack + fallback（不写真实磁盘）', async () => {
    const res = await writeFile('README.md', 'demo')
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) expect(res.data.kind).toBe('ack')
  })

  it('setNodeManual / downloadNodePortable 在浏览器 dev 返回 fallback', async () => {
    for (const res of [await setNodeManual('C:/node.exe'), await downloadNodePortable('22')]) {
      expect(res.ok).toBe(true)
      expect(res.fallback).toBe(true)
      if (res.ok) expect(res.data.status.kind).toBe('need_manual')
    }
  })

  it('detectNode 返回 need_manual 占位（不伪造可用的 Node）', async () => {
    const res = await detectNode()
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) {
      expect(res.data.kind).toBe('node_status')
      if (res.data.kind === 'node_status') {
        expect(res.data.status.kind).toBe('need_manual')
      }
    }
  })

  it('searchWorkspace 返回空结果并标注 fallback', async () => {
    const res = await searchWorkspace('needle')
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) {
      expect(res.data.files).toEqual([])
      expect(res.data.totalMatches).toBe(0)
      expect(res.data.truncated).toBe(false)
    }
  })
})

describe('供应商管理（providers_*）', () => {
  const view = {
    providers: [
      {
        id: 'openai',
        kind: 'openai-responses' as const,
        label: 'OpenAI',
        baseUrl: 'https://api.openai.com/v1',
        enabled: true,
        apiKeyRef: '',
        hasApiKey: true,
        apiKeyHint: '••••6789',
      },
    ],
    models: [
      {
        id: 'gpt-5',
        provider: 'openai',
        modelId: 'gpt-5',
        thinking: ['off', 'high'],
        contextK: 400,
        compressAtK: 300,
        capabilities: ['tools'],
      },
    ],
    defaultModel: 'gpt-5',
    defaultThinking: 'off',
    subagentModel: 'gpt-5',
    subagentThinking: 'high',
  }

  it('浏览器 dev 返回演示供应商', async () => {
    const res = await providersLoad()
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) {
      expect(res.data.providers[0].label).toBe('OpenAI')
      expect(res.data.models[0].id).toBe('gpt-5')
    }
  })

  it('Tauri 路径调用 providers_load 原样返回脱敏视图', async () => {
    const calls: string[] = []
    installTauri({
      invoke: async cmd => {
        calls.push(cmd)
        return view
      },
    })
    const res = await providersLoad()
    expect(calls).toEqual(['providers_load'])
    expect(res.ok).toBe(true)
    if (res.ok) expect(res.data.providers[0].apiKeyHint).toBe('••••6789')
  })

  it('providers_save 以 input 包装提交并返回新视图', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return view
      },
    })
    const input = {
      providers: [
        {
          id: 'openai',
          kind: 'openai-responses' as const,
          label: 'OpenAI',
          baseUrl: 'https://api.openai.com/v1',
          enabled: true,
          apiKey: 'sk-123456789',
          apiKeyRef: '',
        },
      ],
      models: view.models,
      defaultModel: 'gpt-5',
      defaultThinking: 'off',
      subagentModel: 'gpt-5',
      subagentThinking: 'high',
    }
    const res = await providersSave(input)
    expect(calls[0].cmd).toBe('providers_save')
    expect(calls[0].args).toEqual({ input })
    expect(res.ok).toBe(true)
  })

  it('保存错误转成 ErrorBody 分支', async () => {
    installTauri({
      invoke: async () => {
        throw { code: 'invalid_argument', message: 'compressAtK 必须小于 contextK' }
      },
    })
    const res = await providersSave({
      providers: [],
      models: [],
      defaultModel: null,
      defaultThinking: null,
      subagentModel: null,
      subagentThinking: null,
    })
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.code).toBe('invalid_argument')
  })
})

describe('工作区搜索（fs_search）', () => {
  it('Tauri 路径传 snake_case 参数并原样返回结果', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return {
          files: [
            {
              path: 'D:/proj/a.txt',
              matches: [{ line: 2, column: 3, length: 5, preview: 'xxneedle', matchStart: 2 }],
            },
          ],
          totalMatches: 1,
          truncated: false,
          scannedFiles: 3,
        }
      },
    })
    const res = await searchWorkspace('needle', { caseSensitive: true, regex: true, maxResults: 20 })
    expect(res.ok).toBe(true)
    expect(calls[0].cmd).toBe('fs_search')
    expect(calls[0].args).toEqual({
      query: 'needle',
      case_sensitive: true,
      regex: true,
      max_results: 20,
    })
    if (res.ok) {
      expect(res.data.totalMatches).toBe(1)
      expect(res.data.files[0].matches[0].line).toBe(2)
    }
  })

  it('命令错误转成 ErrorBody 分支', async () => {
    installTauri({
      invoke: async () => {
        throw { code: 'invalid_argument', message: '无效的搜索正则' }
      },
    })
    const res = await searchWorkspace('(')
    expect(res.ok).toBe(false)
    if (!res.ok) {
      expect(res.error.code).toBe('invalid_argument')
    }
  })

  it('畸形载荷视为 internal 错误', async () => {
    installTauri({ invoke: async () => ({ nope: true }) })
    const res = await searchWorkspace('needle')
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.code).toBe('internal')
  })
})

describe('文件写操作命令（移动/新建/删除/重命名/定位）', () => {
  it('浏览器 dev 全部返回 fallback ack（不触碰真实磁盘）', async () => {
    for (const res of [
      await movePath('D:/p/a.rs', 'D:/p/sub/a.rs'),
      await createEntry('D:/p/new.rs', false),
      await deletePath('D:/p/new.rs'),
      await renamePath('D:/p/a.rs', 'D:/p/b.rs'),
      await revealPath('D:/p/a.rs'),
    ]) {
      expect(res).toEqual({ ok: true, data: { kind: 'ack' }, fallback: true })
    }
  })

  it('Tauri：命令名与 snake_case 参数', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { kind: 'ack' } satisfies CommandResult
      },
    })

    await movePath('D:/p/a.rs', 'D:/p/sub/a.rs')
    await createEntry('D:/p/new', true)
    await deletePath('D:/p/new')
    await renamePath('D:/p/a.rs', 'D:/p/b.rs')
    await revealPath('D:/p/b.rs')

    expect(calls).toEqual([
      { cmd: 'fs_move', args: { from: 'D:/p/a.rs', to: 'D:/p/sub/a.rs' } },
      { cmd: 'fs_create', args: { path: 'D:/p/new', dir: true } },
      { cmd: 'fs_delete', args: { path: 'D:/p/new' } },
      { cmd: 'fs_rename', args: { from: 'D:/p/a.rs', to: 'D:/p/b.rs' } },
      { cmd: 'fs_reveal', args: { path: 'D:/p/b.rs' } },
    ])
  })
})

describe('Tauri 路径', () => {
  it('readDir 只把 path 传给 fs_read_dir（root 由传输层注入）', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    const listing: DirListingResult = {
      kind: 'dir_listing',
      path: 'D:/proj',
      entries: [{ name: 'a.rs', path: 'D:/proj/a.rs', kind: 'file', size: 3 }],
      generation: 7,
    }
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return listing
      },
    })
    const res: BridgeResult<DirListingResult> = await readDir('D:/proj', 'D:/proj')
    expect(calls).toEqual([{ cmd: 'fs_read_dir', args: { path: 'D:/proj' } }])
    expect(res).toEqual({ ok: true, data: listing, fallback: false })
  })

  it('命令 Err：reject 的 ErrorBody 原样进入 error 分支', async () => {
    installTauri({
      invoke: async () => {
        throw { code: 'path_outside_root', message: 'path outside project root: x', detail: null }
      },
    })
    const res = await readDir('D:/proj', 'D:/elsewhere')
    expect(res.ok).toBe(false)
    if (!res.ok) {
      expect(res.fallback).toBe(false)
      expect(res.error.code).toBe('path_outside_root')
      expect(res.error.message).toContain('outside')
    }
  })

  it('reject 载荷非 ErrorBody 时归入 internal（不盲信 IPC 对端）', async () => {
    installTauri({ invoke: async () => Promise.reject('boom') })
    const res = await readDir('D:/proj', 'D:/proj')
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.code).toBe('internal')
  })

  it('命令返回非预期 kind 归入 internal（契约形状校验）', async () => {
    installTauri({ invoke: async () => ({ kind: 'ack' }) satisfies CommandResult })
    const res = await readDir('D:/proj', 'D:/proj')
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.code).toBe('internal')
  })

  it('workspace_root 以空参数调用并返回根', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { kind: 'workspace_root', root: 'D:/proj' }
      },
    })
    const res = await workspaceRoot()
    expect(calls).toEqual([{ cmd: 'workspace_root', args: {} }])
    expect(res).toEqual({
      ok: true,
      data: { kind: 'workspace_root', root: 'D:/proj' },
      fallback: false,
    })
  })

  it('workspace_open 以 { path } 调用并返回新根', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { kind: 'workspace_root', root: 'D:/new' }
      },
    })
    const res = await openWorkspace('D:/new')
    expect(calls).toEqual([{ cmd: 'workspace_open', args: { path: 'D:/new' } }])
    expect(res.ok).toBe(true)
    if (res.ok) expect(res.data.root).toBe('D:/new')
  })

  it('writeFile 以 { path, content } 调 fs_write_file', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { kind: 'ack' }
      },
    })
    const res = await writeFile('D:/proj/a.rs', 'fn main() {}')
    expect(calls).toEqual([
      { cmd: 'fs_write_file', args: { path: 'D:/proj/a.rs', content: 'fn main() {}' } },
    ])
    expect(res).toEqual({ ok: true, data: { kind: 'ack' }, fallback: false })
  })

  it('setNodeManual 以 { path } 调用并返回 ready', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return {
          kind: 'node_status',
          status: {
            kind: 'ready',
            runtime: {
              path: 'D:/node.exe',
              version: { major: 22, minor: 14, patch: 0 },
              source: 'manual',
              health: 'supported',
            },
          },
        }
      },
    })
    const res = await setNodeManual('D:/node.exe')
    expect(calls).toEqual([{ cmd: 'node_set_manual', args: { path: 'D:/node.exe' } }])
    expect(res.ok).toBe(true)
    if (res.ok) expect(res.data.status.kind).toBe('ready')
  })

  it('downloadNodePortable 以 { version: null } 调用', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { kind: 'node_status', status: { kind: 'need_manual', attempts: [] } }
      },
    })
    await downloadNodePortable()
    expect(calls).toEqual([{ cmd: 'node_download_portable', args: { version: null } }])
  })

  it('node_detect 以空参数调用', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { kind: 'node_status', status: { kind: 'need_manual', attempts: [] } }
      },
    })
    const res = await detectNode()
    expect(calls).toEqual([{ cmd: 'node_detect', args: {} }])
    expect(res.ok).toBe(true)
  })
})

describe('Agent 会话命令', () => {
  const session: SessionInfo = {
    session_id: 's1',
    mode: 'auto',
    turn: 0,
    max_turns: 25,
    context_k: 400,
    compress_at_k: 300,
    cancelled: false,
  }

  it('浏览器 dev：session 返回 fallback 占位，其余命令返回 fallback ack', async () => {
    const created = await sessionCreate('plan')
    expect(created.ok).toBe(true)
    expect(created.fallback).toBe(true)
    if (created.ok) {
      expect(created.data.kind).toBe('session')
      expect(created.data.session).toMatchObject({
        mode: 'plan',
        turn: 0,
        context_k: 400,
        compress_at_k: 300,
      })
    }

    for (const res of [
      await agentSend('s1', 'hello'),
      await agentCancel('s1'),
      await resolveApproval('s1', 1, 'allow_once'),
    ]) {
      expect(res.ok).toBe(true)
      expect(res.fallback).toBe(true)
      if (res.ok) expect(res.data).toEqual({ kind: 'ack' })
    }
  })

  it('Tauri：命令名与 snake_case 参数与契约 CoreCommand 对应', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        if (cmd === 'session_create') return { kind: 'session', session }
        return { kind: 'ack' }
      },
    })

    const created = await sessionCreate('auto')
    expect(created).toEqual({
      ok: true,
      data: { kind: 'session', session },
      fallback: false,
    })
    expect((await agentSend('s1', 'hello')).ok).toBe(true)
    expect((await agentCancel('s1')).ok).toBe(true)
    expect((await resolveApproval('s1', 3, 'allow_session')).ok).toBe(true)

    expect(calls).toEqual([
      {
        cmd: 'session_create',
        args: { mode: 'auto', task_id: null, model: null, thinking: null },
      },
      { cmd: 'agent_send', args: { session_id: 's1', text: 'hello' } },
      { cmd: 'agent_cancel', args: { session_id: 's1' } },
      {
        cmd: 'approval_resolve',
        args: { session_id: 's1', approval_id: 3, decision: 'allow_session' },
      },
    ])
  })

  it('Tauri：session_create 把 task_id 透传给契约', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { kind: 'session', session }
      },
    })

    expect((await sessionCreate('auto', 'task-7')).ok).toBe(true)
    expect(calls).toEqual([
      {
        cmd: 'session_create',
        args: { mode: 'auto', task_id: 'task-7', model: null, thinking: null },
      },
    ])
  })

  it('Tauri：session_create 透传模型与思考挡位', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { kind: 'session', session }
      },
    })

    expect((await sessionCreate('auto', null, 'gpt-5', 'high')).ok).toBe(true)
    expect(calls).toEqual([
      {
        cmd: 'session_create',
        args: { mode: 'auto', task_id: null, model: 'gpt-5', thinking: 'high' },
      },
    ])
  })

  it('Tauri：返回 kind 与命令不匹配时归入 internal', async () => {
    installTauri({ invoke: async () => ({ kind: 'ack' }) satisfies CommandResult })

    const created = await sessionCreate('auto')
    expect(created.ok).toBe(false)
    if (!created.ok) expect(created.error.code).toBe('internal')
  })

  it('Tauri：旧形状 session（缺 context_k/compress_at_k）被拒绝', async () => {
    const legacy = {
      session_id: 's1',
      mode: 'auto',
      turn: 0,
      max_turns: 25,
      cancelled: false,
    }
    installTauri({
      invoke: async () => ({ kind: 'session', session: legacy }) as unknown as CommandResult,
    })

    const created = await sessionCreate('auto')
    expect(created.ok).toBe(false)
    if (!created.ok) {
      expect(created.error.code).toBe('internal')
      expect(created.error.message).toContain('session')
      expect(created.fallback).toBe(false)
    }
  })

  it('Tauri：ack 命令收到非 ack 载荷时归入 internal', async () => {
    installTauri({
      invoke: async () => ({ kind: 'session', session }) satisfies CommandResult,
    })

    const res = await agentSend('s1', 'hello')
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.code).toBe('internal')
  })
})

describe('Task 工作台命令', () => {
  function taskInfo(overrides: Partial<TaskInfo> = {}): TaskInfo {
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

  it('浏览器 dev：任务列表初始为空，create 返回占位 Task，生命周期命令维持形状', async () => {
    const listed = await taskList()
    expect(listed).toEqual({
      ok: true,
      data: { kind: 'tasks', tasks: [] },
      fallback: true,
    })

    const created = await taskCreate('Demo task', 's1')
    expect(created.ok).toBe(true)
    expect(created.fallback).toBe(true)
    if (!created.ok) return
    expect(created.data.task).toMatchObject({
      title: 'Demo task',
      status: 'planning',
      session_id: 's1',
    })
    const id = created.data.task.task_id

    const started = await taskStart(id)
    expect(started.ok).toBe(true)
    if (started.ok) expect(started.data.task.status).toBe('running')

    const cancelled = await taskCancel(id)
    expect(cancelled.ok).toBe(true)
    if (cancelled.ok) expect(cancelled.data.task.status).toBe('failed')

    const applied = await taskApply(id)
    expect(applied.ok).toBe(true)
    if (applied.ok) expect(applied.data.task.status).toBe('done')

    const diff = await taskDiff(id)
    expect(diff.ok).toBe(true)
    expect(diff.fallback).toBe(true)
    if (diff.ok) expect(diff.data.patch).toContain('browser dev placeholder diff')

    const bound = await taskBindSession(id, 's2')
    expect(bound.ok).toBe(true)
    if (bound.ok) expect(bound.data.task.session_id).toBe('s2')

    expect(await taskDiscard(id)).toEqual({
      ok: true,
      data: { kind: 'ack' },
      fallback: true,
    })
  })

  it('Tauri：task_list 以空参数调用并校验 TaskInfo 列表', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    const payload: TaskListResult = { kind: 'tasks', tasks: [taskInfo()] }
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return payload
      },
    })

    const res = await taskList()
    expect(calls).toEqual([{ cmd: 'task_list', args: {} }])
    expect(res).toEqual({ ok: true, data: payload, fallback: false })
  })

  it('Tauri：task_create 传 title 与 session_id（缺省 null）', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { kind: 'task', task: taskInfo() }
      },
    })

    expect((await taskCreate('Fix login')).ok).toBe(true)
    expect((await taskCreate('Bind me', 's9')).ok).toBe(true)
    expect(calls).toEqual([
      { cmd: 'task_create', args: { title: 'Fix login', session_id: null } },
      { cmd: 'task_create', args: { title: 'Bind me', session_id: 's9' } },
    ])
  })

  it('Tauri：start/cancel/apply/bind/diff/discard 的命令名与参数对应契约', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    const diff: TaskDiffResult = {
      kind: 'task_diff',
      task_id: 't1',
      patch: '--- a\n+++ b\n',
      files: [
        {
          path: 'src/a.ts',
          status: 'modified',
          applied: false,
          added: 2,
          removed: 1,
        },
      ],
    }
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        if (cmd === 'task_discard') return { kind: 'ack' }
        if (cmd === 'task_diff') return diff
        return { kind: 'task', task: taskInfo() }
      },
    })

    expect((await taskStart('t1')).ok).toBe(true)
    expect((await taskCancel('t1')).ok).toBe(true)
    expect((await taskApply('t1')).ok).toBe(true)
    expect((await taskBindSession('t1', 's2')).ok).toBe(true)
    expect((await taskDiff('t1')).ok).toBe(true)
    expect((await taskDiscard('t1')).ok).toBe(true)

    expect(calls).toEqual([
      { cmd: 'task_start', args: { task_id: 't1' } },
      { cmd: 'task_cancel', args: { task_id: 't1' } },
      { cmd: 'task_apply', args: { task_id: 't1' } },
      { cmd: 'task_bind_session', args: { task_id: 't1', session_id: 's2' } },
      { cmd: 'task_diff', args: { task_id: 't1' } },
      { cmd: 'task_discard', args: { task_id: 't1' } },
    ])
  })

  it('Tauri：畸形 TaskInfo / task_diff 载荷归入 internal', async () => {
    installTauri({
      invoke: async () => ({ kind: 'task', task: { task_id: 't1' } }),
    })
    const badTask = await taskCreate('x')
    expect(badTask.ok).toBe(false)
    if (!badTask.ok) {
      expect(badTask.error.code).toBe('internal')
      expect(badTask.error.message).toContain('TaskInfo')
    }

    installTauri({
      invoke: async () => ({
        kind: 'tasks',
        tasks: [{ task_id: 't1', status: 'planning' }],
      }),
    })
    const badList = await taskList()
    expect(badList.ok).toBe(false)
    if (!badList.ok) expect(badList.error.code).toBe('internal')

    installTauri({
      invoke: async () => ({
        kind: 'task_diff',
        task_id: 't1',
        patch: 'x',
        files: [{ path: 'a.ts', status: 'weird' }],
      }),
    })
    const badDiff = await taskDiff('t1')
    expect(badDiff.ok).toBe(false)
    if (!badDiff.ok) expect(badDiff.error.code).toBe('internal')
  })

  it('Tauri：任务命令返回错误 kind 时归入 internal', async () => {
    installTauri({ invoke: async () => ({ kind: 'ack' }) satisfies CommandResult })
    const res = await taskStart('t1')
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.code).toBe('internal')
  })
})

describe('数据库命令', () => {
  function dbConnection(overrides: Partial<DbConnectionInfo> = {}): DbConnectionInfo {
    return {
      connection_id: 'c1',
      name: 'Local',
      kind: 'sqlite',
      read_only: false,
      state: 'disconnected',
      host: null,
      database: 'app.db',
      ...overrides,
    }
  }

  function dbResult(overrides: Partial<DbQueryResult> = {}): DbQueryResult {
    return {
      columns: [{ name: 'id', type_name: 'INTEGER', nullable: false }],
      rows: [['1'], [null]],
      row_count: 2,
      truncated: false,
      handle: null,
      elapsed_ms: 4,
      untrusted: true,
      ...overrides,
    }
  }

  function config(): DbConnectionConfig {
    return {
      connection_id: 'c1',
      name: 'Local',
      kind: 'sqlite',
      host: '',
      port: 0,
      database: 'app.db',
      user: '',
      secret_ref: '',
      read_only: true,
    }
  }

  it('浏览器 dev：连接 CRUD 与查询都返回 fallback 占位，不连真实数据库', async () => {
    const initial = await dbList()
    expect(initial.ok).toBe(true)
    expect(initial.fallback).toBe(true)
    if (initial.ok) expect(initial.data).toEqual({ kind: 'connections', connections: [] })

    const saved = await dbSaveConnection(config())
    expect(saved.ok).toBe(true)
    expect(saved.fallback).toBe(true)
    if (saved.ok) {
      expect(saved.data.connection).toMatchObject({
        connection_id: 'c1',
        state: 'disconnected',
        database: 'app.db',
        read_only: true,
      })
    }

    const list = await dbList()
    if (list.ok) expect(list.data.connections.map(item => item.connection_id)).toEqual(['c1'])

    const connected = await dbConnect('c1')
    if (connected.ok) expect(connected.data.connection.state).toBe('connected')
    const disconnected = await dbDisconnect('c1')
    if (disconnected.ok) expect(disconnected.data.connection.state).toBe('disconnected')

    const schema = await dbSchema('c1')
    expect(schema.ok).toBe(true)
    expect(schema.fallback).toBe(true)
    if (schema.ok) expect(schema.data.nodes).toEqual([])

    const query = await dbQuery('c1', 'SELECT * FROM t')
    if (query.ok) {
      expect(query.data.result).toMatchObject({
        rows: [],
        row_count: 0,
        untrusted: true,
      })
    }
    const exec = await dbExecute('c1', 'DELETE FROM t')
    if (exec.ok) expect(exec.data.result.affected_rows).toBe(0)
    const explain = await dbExplain('c1', 'SELECT * FROM t')
    expect(explain.ok).toBe(true)
    if (explain.ok) expect(explain.data.result.plan).toContain('浏览器演示')

    const deleted = await dbDeleteConnection('c1')
    expect(deleted.ok).toBe(true)
    expect(deleted.fallback).toBe(true)
    const after = await dbList()
    if (after.ok) expect(after.data.connections).toEqual([])
  })

  it('Tauri：9 个命令名与 snake_case 参数与契约 CoreCommand 对应', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    const item = dbConnection()
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        switch (cmd) {
          case 'db_list':
            return { kind: 'connections', connections: [item] }
          case 'db_connect':
          case 'db_disconnect':
          case 'db_save_connection':
            return { kind: 'connection', connection: item }
          case 'db_schema':
            return { kind: 'db_schema', connection_id: 'c1', nodes: [] }
          case 'db_query':
            return { kind: 'db_query', result: dbResult() }
          case 'db_execute':
            return { kind: 'db_exec', result: { affected_rows: 2, elapsed_ms: 3 } }
          case 'db_explain':
            return { kind: 'db_explain', result: { plan: 'SCAN t', elapsed_ms: 1 } }
          default:
            return { kind: 'ack' }
        }
      },
    })

    const list: BridgeResult<DbListResult> = await dbList()
    const connected: BridgeResult<DbConnectionResult> = await dbConnect('c1')
    const disconnected: BridgeResult<DbConnectionResult> = await dbDisconnect('c1')
    const schema: BridgeResult<DbSchemaResult> = await dbSchema('c1')
    const query: BridgeResult<DbQueryCommandResult> = await dbQuery('c1', 'SELECT 1', 50)
    const exec: BridgeResult<DbExecCommandResult> = await dbExecute('c1', 'DELETE FROM t')
    const explain: BridgeResult<DbExplainCommandResult> = await dbExplain('c1', 'SELECT 1')
    const saved: BridgeResult<DbConnectionResult> = await dbSaveConnection(config())
    const deleted = await dbDeleteConnection('c1')

    for (const res of [list, connected, disconnected, schema, query, exec, explain, saved, deleted]) {
      expect(res.ok).toBe(true)
      expect(res.fallback).toBe(false)
    }
    expect(calls).toEqual([
      { cmd: 'db_list', args: {} },
      { cmd: 'db_connect', args: { connection_id: 'c1' } },
      { cmd: 'db_disconnect', args: { connection_id: 'c1' } },
      { cmd: 'db_schema', args: { connection_id: 'c1' } },
      { cmd: 'db_query', args: { connection_id: 'c1', sql: 'SELECT 1', limit: 50 } },
      { cmd: 'db_execute', args: { connection_id: 'c1', sql: 'DELETE FROM t' } },
      { cmd: 'db_explain', args: { connection_id: 'c1', sql: 'SELECT 1' } },
      { cmd: 'db_save_connection', args: { connection: config() } },
      { cmd: 'db_delete_connection', args: { connection_id: 'c1' } },
    ])
  })

  it('Tauri：db_query 缺省 limit 以 null 下发（契约 Option<u32>）', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { kind: 'db_query', result: dbResult() }
      },
    })
    expect((await dbQuery('c1', 'SELECT 1')).ok).toBe(true)
    expect(calls).toEqual([
      { cmd: 'db_query', args: { connection_id: 'c1', sql: 'SELECT 1', limit: null } },
    ])
  })

  it('Tauri：畸形连接列表 / schema 载荷归入 internal', async () => {
    installTauri({
      invoke: async cmd => {
        if (cmd === 'db_list') {
          return {
            kind: 'connections',
            connections: [{ ...dbConnection(), kind: 'oracle' }],
          }
        }
        return {
          kind: 'db_schema',
          connection_id: 'c1',
          nodes: [{ name: 'users', kind: 'table' }],
        }
      },
    })
    const list = await dbList()
    expect(list.ok).toBe(false)
    if (!list.ok) expect(list.error.code).toBe('internal')
    const schema = await dbSchema('c1')
    expect(schema.ok).toBe(false)
    if (!schema.ok) expect(schema.error.message).toContain('db_schema')
  })

  it('Tauri：畸形查询/执行/EXPLAIN 载荷归入 internal', async () => {
    installTauri({
      invoke: async cmd => {
        if (cmd === 'db_query') {
          return { kind: 'db_query', result: { ...dbResult(), rows: [[42]] } }
        }
        if (cmd === 'db_execute') {
          return { kind: 'db_exec', result: { affected_rows: -1, elapsed_ms: 0 } }
        }
        return { kind: 'db_explain', result: { plan: 42, elapsed_ms: 0 } }
      },
    })
    const query = await dbQuery('c1', 'SELECT 1')
    const exec = await dbExecute('c1', 'DELETE FROM t')
    const explain = await dbExplain('c1', 'SELECT 1')
    for (const res of [query, exec, explain]) {
      expect(res.ok).toBe(false)
      if (!res.ok) expect(res.error.code).toBe('internal')
    }
  })

  it('Tauri：连接命令 kind 不匹配或连接载荷非法时归入 internal', async () => {
    installTauri({ invoke: async () => ({ kind: 'ack' }) })
    const wrongKind = await dbConnect('c1')
    expect(wrongKind.ok).toBe(false)
    if (!wrongKind.ok) expect(wrongKind.error.code).toBe('internal')

    installTauri({
      invoke: async () => ({ kind: 'connection', connection: { connection_id: 'c1' } }),
    })
    const badShape = await dbSaveConnection(config())
    expect(badShape.ok).toBe(false)
    if (!badShape.ok) expect(badShape.error.code).toBe('internal')
  })
})

describe('Git 面板命令', () => {
  function gitStatusPayload(overrides: Record<string, unknown> = {}): GitStatusResult {
    return {
      kind: 'git_status',
      status: {
        branch: 'agent/git-ui',
        upstream: 'origin/main',
        ahead: 2,
        behind: 1,
        clean: false,
        merge_in_progress: false,
        rebase_in_progress: false,
        files: [
          {
            path: 'apps/web/src/App.vue',
            status: 'modified',
            staged: true,
            added: 12,
            removed: 3,
          },
        ],
        ...overrides,
      },
    }
  }

  it('浏览器 dev：status/log 为空，diff 为占位 patch，写操作返回 fallback ack', async () => {
    const status = await gitStatus()
    expect(status.ok).toBe(true)
    expect(status.fallback).toBe(true)
    if (status.ok) {
      expect(status.data.kind).toBe('git_status')
      expect(status.data.status.files).toEqual([])
      expect(status.data.status.clean).toBe(true)
    }

    const diff = await gitDiff('src/main.ts', false)
    expect(diff.ok).toBe(true)
    expect(diff.fallback).toBe(true)
    if (diff.ok) {
      expect(diff.data.kind).toBe('git_diff')
      expect(diff.data.patch).toContain('浏览器 dev 占位 diff')
      expect(diff.data.patch).toContain('unstaged')
      expect(diff.data.files).toEqual([])
    }

    const log = await gitLog()
    expect(log.ok).toBe(true)
    expect(log.fallback).toBe(true)
    if (log.ok) expect(log.data.commits).toEqual([])

    for (const res of [
      await gitStage(['a.ts']),
      await gitUnstage(['a.ts']),
      await gitCommit('demo'),
    ]) {
      expect(res.ok).toBe(true)
      expect(res.fallback).toBe(true)
      if (res.ok) expect(res.data.kind).toBe('ack')
    }
  })

  it('Tauri：git_status 以空参数调用并校验 GitStatus', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    const payload = gitStatusPayload()
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return payload
      },
    })
    const res = await gitStatus()
    expect(calls).toEqual([{ cmd: 'git_status', args: {} }])
    expect(res).toEqual({ ok: true, data: payload, fallback: false })
  })

  it('Tauri：git_diff 透传 path/staged 并校验 patch 与文件列表', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    const payload: GitDiffResult = {
      kind: 'git_diff',
      patch: '--- a.ts\n+++ b.ts\n',
      files: [
        { path: 'b.ts', status: 'added', staged: true, added: 1, removed: 0 },
      ],
    }
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return payload
      },
    })
    const res = await gitDiff(null, true)
    expect(calls).toEqual([{ cmd: 'git_diff', args: { path: null, staged: true } }])
    expect(res).toEqual({ ok: true, data: payload, fallback: false })
  })

  it('Tauri：stage/unstage/commit 的命令名与 paths/message 参数对应契约', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { kind: 'ack' } satisfies CommandResult
      },
    })
    await gitStage(['a.ts', 'b.ts'])
    await gitUnstage(['a.ts'])
    await gitCommit('feat: demo')
    expect(calls).toEqual([
      { cmd: 'git_stage', args: { paths: ['a.ts', 'b.ts'] } },
      { cmd: 'git_unstage', args: { paths: ['a.ts'] } },
      { cmd: 'git_commit', args: { message: 'feat: demo' } },
    ])
  })

  it('Tauri：git_log 缺省 limit 以 null 下发，显式 limit 原样透传', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    const payload: GitLogResult = {
      kind: 'git_log',
      commits: [
        {
          sha: 'abcdef1234567890',
          short_sha: 'abcdef1',
          author: 'Dev',
          message: 'feat: demo',
          timestamp: 1_700_000_000,
        },
      ],
    }
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return payload
      },
    })
    const all = await gitLog()
    const limited = await gitLog(5)
    expect(calls).toEqual([
      { cmd: 'git_log', args: { limit: null } },
      { cmd: 'git_log', args: { limit: 5 } },
    ])
    expect(all).toEqual({ ok: true, data: payload, fallback: false })
    expect(limited).toEqual({ ok: true, data: payload, fallback: false })
  })

  it('Tauri：畸形 GitStatus / diff 文件 / commit 列表归入 internal', async () => {
    installTauri({
      invoke: async cmd => {
        if (cmd === 'git_status') {
          return gitStatusPayload({
            files: [{ path: 'x', status: 'bogus', staged: false, added: 0, removed: 0 }],
          })
        }
        if (cmd === 'git_diff') {
          return { kind: 'git_diff', patch: 42, files: [] }
        }
        return {
          kind: 'git_log',
          commits: [{ sha: 'x', short_sha: 'x', author: 'a', message: 'm' }],
        }
      },
    })
    for (const res of [
      await gitStatus(),
      await gitDiff('x', false),
      await gitLog(),
    ]) {
      expect(res.ok).toBe(false)
      if (!res.ok) expect(res.error.code).toBe('internal')
    }
  })

  it('Tauri：Git 命令返回错误 kind 时归入 internal', async () => {
    installTauri({ invoke: async () => ({ kind: 'ack' }) satisfies CommandResult })
    for (const res of [await gitStatus(), await gitDiff('x', false), await gitLog()]) {
      expect(res.ok).toBe(false)
      if (!res.ok) expect(res.error.code).toBe('internal')
    }
  })
})

describe('onCoreEvent', () => {
  interface FakeEventWindow {
    callbacks: Map<number, (response: unknown) => void>
    calls: Array<{ cmd: string; args?: Record<string, unknown> }>
    unregistered: Array<{ event: string; eventId: number }>
  }

  function installEventTauri(listenResult: Promise<number> = Promise.resolve(7)): FakeEventWindow {
    const callbacks = new Map<number, (response: unknown) => void>()
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    const unregistered: Array<{ event: string; eventId: number }> = []
    let nextId = 1
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        if (cmd === 'plugin:event|listen') return listenResult
        return undefined
      },
      transformCallback: callback => {
        const id = nextId++
        callbacks.set(id, callback)
        return id
      },
    })
    ;(
      (globalThis as unknown as { window: { __TAURI_EVENT_PLUGIN_INTERNALS__?: unknown } }).window
    ).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (event: string, eventId: number) => {
        unregistered.push({ event, eventId })
      },
    }
    return { callbacks, calls, unregistered }
  }

  function tauriEvent(payload: unknown): unknown {
    return { event: CORE_EVENT_CHANNEL, id: 1, payload }
  }

  it('浏览器 dev 返回 no-op', () => {
    const off = onCoreEvent(() => {
      throw new Error('handler must not run outside Tauri')
    })
    expect(() => off()).not.toThrow()
  })

  it('注册 core:event 监听并转发契约事件', () => {
    const fake = installEventTauri()
    const seen: CoreEvent[] = []
    onCoreEvent(event => seen.push(event))

    expect(fake.calls[0]).toEqual({
      cmd: 'plugin:event|listen',
      args: { event: CORE_EVENT_CHANNEL, target: { kind: 'Any' }, handler: 1 },
    })

    const callback = fake.callbacks.get(1)
    expect(callback).toBeDefined()
    callback?.(tauriEvent({ type: 'agent:text_delta', session_id: 's1', index: 0, text: 'hi' }))
    callback?.({
      type: 'agent:done',
      session_id: 's1',
      stop_reason: 'end_turn',
    })

    expect(seen).toHaveLength(2)
    expect(seen[0]).toMatchObject({ type: 'agent:text_delta', text: 'hi' })
    expect(seen[1]).toMatchObject({ type: 'agent:done', stop_reason: 'end_turn' })
  })

  it('转发 db:status，畸形连接载荷被丢弃', () => {
    const fake = installEventTauri()
    const seen: UiCoreEvent[] = []
    onCoreEvent(event => seen.push(event))
    const callback = fake.callbacks.get(1)

    callback?.(
      tauriEvent({
        type: 'db:status',
        connection: {
          connection_id: 'c1',
          name: 'Local',
          kind: 'sqlite',
          read_only: true,
          state: 'connected',
          database: 'app.db',
        },
      }),
    )
    callback?.(
      tauriEvent({
        type: 'db:status',
        connection: { connection_id: 'c1', kind: 'sqlite' },
      }),
    )

    expect(seen).toHaveLength(1)
    expect(seen[0]).toMatchObject({ type: 'db:status' })
    if (seen[0].type === 'db:status') {
      expect(seen[0].connection.connection_id).toBe('c1')
      expect(seen[0].connection.read_only).toBe(true)
    }
  })

  it('兼容 EventEnvelope 包装与裸 CoreEvent', () => {
    const fake = installEventTauri()
    const seen: CoreEvent[] = []
    onCoreEvent(event => seen.push(event))
    const callback = fake.callbacks.get(1)

    callback?.({
      event: CORE_EVENT_CHANNEL,
      id: 1,
      payload: {
        seq: 9,
        event: { type: 'agent:usage', session_id: 's1', input: 1, output: 2, cached: 0 },
      },
    })
    callback?.(tauriEvent({ type: 'error', error: { code: 'io', message: 'boom' } }))
    callback?.({ type: 'agent:thinking_delta', session_id: 's1', text: 'raw' })

    expect(seen.map(event => event.type)).toEqual([
      'agent:usage',
      'error',
      'agent:thinking_delta',
    ])
  })

  it('丢弃非契约载荷、未知事件与畸形审批卡', () => {
    const fake = installEventTauri()
    const seen: CoreEvent[] = []
    onCoreEvent(event => seen.push(event))
    const callback = fake.callbacks.get(1)

    callback?.(tauriEvent({ type: 'fs:tree', generation: 2 }))
    callback?.(tauriEvent({ type: 'agent:text_delta', session_id: 's1', text: 'missing index' }))
    callback?.(tauriEvent({ type: 'agent:approval_required', session_id: 's1', approval: {} }))
    callback?.(tauriEvent(null))
    callback?.('not-an-event')

    expect(seen).toHaveLength(0)
  })

  it('转发 agent:compacted；缺字段的压缩事件与旧形状 session_state 被丢弃', () => {
    const fake = installEventTauri()
    const seen: CoreEvent[] = []
    onCoreEvent(event => seen.push(event))
    const callback = fake.callbacks.get(1)

    callback?.(
      tauriEvent({
        type: 'agent:compacted',
        session_id: 's1',
        record_id: 3,
        before_tokens: 12300,
        after_tokens: 4100,
        removed_messages: 7,
        summary: '早期对话已摘要',
      }),
    )
    callback?.(
      tauriEvent({
        type: 'agent:compacted',
        session_id: 's1',
        record_id: 4,
        before_tokens: 9000,
        after_tokens: 3000,
        removed_messages: 3,
      }),
    )
    callback?.(
      tauriEvent({
        type: 'agent:session_state',
        session: {
          session_id: 's1',
          mode: 'auto',
          turn: 1,
          max_turns: 25,
          cancelled: false,
        },
      }),
    )

    expect(seen).toHaveLength(1)
    expect(seen[0]).toMatchObject({
      type: 'agent:compacted',
      record_id: 3,
      summary: '早期对话已摘要',
    })
  })

  it('转发 task:* 事件；畸形 Task 载荷与未知状态被丢弃', () => {
    const fake = installEventTauri()
    const seen: CoreEvent[] = []
    onCoreEvent(event => seen.push(event))
    const callback = fake.callbacks.get(1)

    const task = {
      task_id: 't1',
      title: 'Fix login',
      session_id: null,
      status: 'running',
      worktree_path: 'D:/wt/t1',
      branch: 'lc/t1-fix',
      port: 4300,
      base_sha: 'abc123',
      steps: [{ index: 0, title: 'plan', state: 'done' }],
      changes: [
        { path: 'a.ts', status: 'modified', applied: false, added: 2, removed: 1 },
      ],
      checks: [{ name: 'unit', ok: true, output_tail: '', duration_ms: 12 }],
      created_at: 1,
    }

    callback?.(tauriEvent({ type: 'task:created', task_id: 't1', title: 'Fix login' }))
    callback?.(tauriEvent({ type: 'task:status', task_id: 't1', status: 'running' }))
    callback?.(tauriEvent({ type: 'task:updated', task }))
    callback?.(
      tauriEvent({ type: 'task:step', task_id: 't1', index: 1, title: 'edit', state: 'running' }),
    )
    callback?.(
      tauriEvent({
        type: 'task:change',
        task_id: 't1',
        change: { path: 'b.ts', status: 'added', applied: false, added: 3, removed: 0 },
      }),
    )
    callback?.(
      tauriEvent({
        type: 'task:check',
        task_id: 't1',
        check: { name: 'lint', ok: false, output_tail: 'boom', duration_ms: 8 },
      }),
    )
    callback?.(tauriEvent({ type: 'task:needs_input', task_id: 't1', kind: 'approval' }))
    callback?.(tauriEvent({ type: 'task:merged', task_id: 't1', commit_sha: 'def456' }))
    callback?.(tauriEvent({ type: 'task:discarded', task_id: 't1' }))

    // 非法载荷：状态枚举、TaskInfo 缺字段、step 状态、diff 文件字段。
    callback?.(tauriEvent({ type: 'task:status', task_id: 't1', status: 'weird' }))
    callback?.(tauriEvent({ type: 'task:updated', task: { task_id: 't1' } }))
    callback?.(
      tauriEvent({ type: 'task:step', task_id: 't1', index: 0, title: 'x', state: 'weird' }),
    )
    callback?.(
      tauriEvent({
        type: 'task:change',
        task_id: 't1',
        change: { path: 'x', status: 'weird' },
      }),
    )

    expect(seen.map(event => event.type)).toEqual([
      'task:created',
      'task:status',
      'task:updated',
      'task:step',
      'task:change',
      'task:check',
      'task:needs_input',
      'task:merged',
      'task:discarded',
    ])
  })

  it('取消订阅：unlisten + unregisterListener，回调不再触发', async () => {
    const fake = installEventTauri()
    const seen: CoreEvent[] = []
    const off = onCoreEvent(event => seen.push(event))
    const callback = fake.callbacks.get(1)

    await Promise.resolve()
    await Promise.resolve()
    off()
    await Promise.resolve()

    expect(fake.calls).toContainEqual({
      cmd: 'plugin:event|unlisten',
      args: { event: CORE_EVENT_CHANNEL, eventId: 7 },
    })
    expect(fake.unregistered).toEqual([{ event: CORE_EVENT_CHANNEL, eventId: 7 }])

    callback?.(tauriEvent({ type: 'agent:text_delta', session_id: 's1', index: 0, text: 'late' }))
    expect(seen).toHaveLength(0)
  })

  it('listen 尚未返回就取消：返回后立即 unlisten', async () => {
    let resolveListen!: (eventId: number) => void
    const listenResult = new Promise<number>(resolve => {
      resolveListen = resolve
    })
    const fake = installEventTauri(listenResult)
    const off = onCoreEvent(() => {})
    off()

    resolveListen(11)
    await vi.waitFor(() => {
      expect(fake.calls).toContainEqual({
        cmd: 'plugin:event|unlisten',
        args: { event: CORE_EVENT_CHANNEL, eventId: 11 },
      })
    })
  })
})
