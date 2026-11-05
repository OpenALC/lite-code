import { afterEach, describe, expect, it, vi } from 'vitest'

import type { CommandResult, CoreEvent, SessionInfo } from '@lc-proto'

import {
  agentCancel,
  agentSend,
  CORE_EVENT_CHANNEL,
  detectNode,
  downloadNodePortable,
  isTauri,
  onCoreEvent,
  openWorkspace,
  readFile,
  readDir,
  resolveApproval,
  sessionCreate,
  setNodeManual,
  workspaceRoot,
  writeFile,
  type BridgeResult,
  type DirListingResult,
} from './bridge'

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
    cancelled: false,
  }

  it('浏览器 dev：session 返回 fallback 占位，其余命令返回 fallback ack', async () => {
    const created = await sessionCreate('plan')
    expect(created.ok).toBe(true)
    expect(created.fallback).toBe(true)
    if (created.ok) {
      expect(created.data.kind).toBe('session')
      expect(created.data.session).toMatchObject({ mode: 'plan', turn: 0 })
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
      { cmd: 'session_create', args: { mode: 'auto' } },
      { cmd: 'agent_send', args: { session_id: 's1', text: 'hello' } },
      { cmd: 'agent_cancel', args: { session_id: 's1' } },
      {
        cmd: 'approval_resolve',
        args: { session_id: 's1', approval_id: 3, decision: 'allow_session' },
      },
    ])
  })

  it('Tauri：返回 kind 与命令不匹配时归入 internal', async () => {
    installTauri({ invoke: async () => ({ kind: 'ack' }) satisfies CommandResult })

    const created = await sessionCreate('auto')
    expect(created.ok).toBe(false)
    if (!created.ok) expect(created.error.code).toBe('internal')
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
