import { describe, expect, it, vi } from 'vitest'

import {
  SSH_PORT_DEFAULT,
  TERMINAL_BUFFER_LIMIT,
  applyPtyEvent,
  base64ToBytes,
  base64ToUtf8,
  buildSshRequest,
  bytesToBase64,
  createSshDraft,
  createTerminalController,
  createTerminalState,
  defaultTerminalDeps,
  isPtyEvent,
  isPtyListResult,
  isPtyOpenResult,
  isPtyReplayResult,
  isPtySessionInfo,
  onPtyEvent,
  parseSshPort,
  terminalActiveChanged,
  terminalDataAppended,
  terminalExited,
  terminalReplayReplaced,
  terminalSessionClosed,
  terminalSessionOpened,
  terminalSessionsLoaded,
  terminalStateMigrated,
  utf8ToBase64,
  validateSshDraft,
  type PtyEvent,
  type PtySessionInfo,
  type SshFormDraft,
} from './terminal'

const encoder = new TextEncoder()

function bytes(text: string): Uint8Array {
  return encoder.encode(text)
}

function sessionInfo(overrides: Partial<PtySessionInfo> = {}): PtySessionInfo {
  return {
    sessionId: 's1',
    kind: 'local',
    state: 'connected',
    title: 'local',
    ...overrides,
  }
}

const ok = <T>(data: T) => ({ ok: true as const, data })
const err = () => ({
  ok: false as const,
  error: { code: 'internal' as const, message: 'x', detail: null },
})

// ==== DTO 守卫 ====

describe('DTO 守卫', () => {
  it('isPtySessionInfo：合法载荷通过', () => {
    expect(isPtySessionInfo(sessionInfo())).toBe(true)
    expect(isPtySessionInfo(sessionInfo({ kind: 'ssh', state: 'reconnecting' }))).toBe(true)
  })

  it('isPtySessionInfo：缺字段 / 非法枚举 / 空 id 拒绝', () => {
    expect(isPtySessionInfo(null)).toBe(false)
    expect(isPtySessionInfo({ sessionId: 's1', kind: 'local', state: 'connected' })).toBe(false)
    expect(isPtySessionInfo({ ...sessionInfo(), kind: 'docker' })).toBe(false)
    expect(isPtySessionInfo({ ...sessionInfo(), state: 'zombie' })).toBe(false)
    expect(isPtySessionInfo({ ...sessionInfo(), sessionId: '' })).toBe(false)
  })

  it('isPtyOpenResult / isPtyListResult / isPtyReplayResult', () => {
    expect(isPtyOpenResult({ sessionId: 's1' })).toBe(true)
    expect(isPtyOpenResult({ sessionId: '' })).toBe(false)
    expect(isPtyOpenResult({})).toBe(false)

    expect(isPtyListResult({ sessions: [sessionInfo()] })).toBe(true)
    expect(isPtyListResult({ sessions: [sessionInfo(), { nope: 1 }] })).toBe(false)
    expect(isPtyListResult({ sessions: 'all' })).toBe(false)

    expect(isPtyReplayResult({ data: utf8ToBase64('hi') })).toBe(true)
    expect(isPtyReplayResult({ data: '' })).toBe(true)
    expect(isPtyReplayResult({ data: 'not+base64!' })).toBe(false)
    expect(isPtyReplayResult({ data: 7 })).toBe(false)
  })

  it('isPtyEvent：三类事件合法载荷与畸形载荷', () => {
    expect(
      isPtyEvent({ type: 'pty:data', sessionId: 's1', data: utf8ToBase64('out') }),
    ).toBe(true)
    expect(isPtyEvent({ type: 'pty:data', sessionId: 's1', data: '!!' })).toBe(false)
    expect(isPtyEvent({ type: 'pty:data', data: '' })).toBe(false)
    expect(isPtyEvent({ type: 'pty:state', sessionId: 's1', state: 'reconnecting' })).toBe(true)
    expect(isPtyEvent({ type: 'pty:state', sessionId: 's1', state: 'nope' })).toBe(false)
    expect(isPtyEvent({ type: 'pty:exit', sessionId: 's1', code: 0 })).toBe(true)
    expect(isPtyEvent({ type: 'pty:exit', sessionId: 's1', code: null })).toBe(true)
    expect(isPtyEvent({ type: 'pty:exit', sessionId: 's1', code: 'zero' })).toBe(false)
    expect(isPtyEvent({ type: 'pty:other', sessionId: 's1' })).toBe(false)
  })
})

// ==== base64 编解码往返 ====

describe('base64 编解码', () => {
  it('ASCII / 多字节 UTF-8 / 空串往返一致', () => {
    for (const text of ['ls\r\n', '', '中文输出 ✅\n', 'emoji 🇸🇬 tail']) {
      const round = base64ToUtf8(utf8ToBase64(text))
      expect(round).toBe(text)
    }
  })

  it('原始字节往返一致（含 1/2 字节尾部与全 alphabet）', () => {
    const samples = [
      new Uint8Array(0),
      new Uint8Array([0]),
      new Uint8Array([0, 255]),
      new Uint8Array([1, 2, 3, 4, 5, 6, 7]),
      Uint8Array.from({ length: 256 }, (_, i) => i),
    ]
    for (const original of samples) {
      const decoded = base64ToBytes(bytesToBase64(original))
      expect(decoded).not.toBeNull()
      expect(Array.from(decoded!)).toEqual(Array.from(original))
    }
  })

  it('非法 base64 返回 null', () => {
    expect(base64ToBytes('abcde')).toBeNull() // 长度 % 4 == 1
    expect(base64ToBytes('a$c')).toBeNull() // 字母表外
    expect(base64ToUtf8('a$c')).toBeNull()
  })

  it('与浏览器 btoa/atob 语义一致（ASCII 抽样）', () => {
    for (const text of ['hello', 'exit code 0\r\n']) {
      expect(utf8ToBase64(text)).toBe(btoa(text))
      expect(base64ToUtf8(btoa(text))).toBe(text)
    }
  })
})

// ==== 事件 → store 归并 ====

describe('store reducer：data 追加 / replay 替换 / state 迁移 / exit', () => {
  it('data 追加到缓冲并按上限截断保留尾部', () => {
    let state = terminalSessionOpened(createTerminalState(), sessionInfo())
    state = terminalDataAppended(state, 's1', bytes('hello '))
    state = terminalDataAppended(state, 's1', bytes('world'))
    expect(new TextDecoder().decode(state.sessions[0]!.buffer)).toBe('hello world')

    const big = new Uint8Array(TERMINAL_BUFFER_LIMIT + 100)
    big.fill(7)
    state = terminalDataAppended(state, 's1', big)
    expect(state.sessions[0]!.buffer.length).toBe(TERMINAL_BUFFER_LIMIT)
  })

  it('data 空帧 / 未知会话不产生新状态', () => {
    const state = terminalSessionOpened(createTerminalState(), sessionInfo())
    expect(terminalDataAppended(state, 's1', new Uint8Array(0))).toBe(state)
    expect(terminalDataAppended(state, 'ghost', bytes('x'))).toBe(state)
  })

  it('replay 替换缓冲', () => {
    let state = terminalSessionOpened(createTerminalState(), sessionInfo())
    state = terminalDataAppended(state, 's1', bytes('stale'))
    state = terminalReplayReplaced(state, 's1', bytes('fresh output'))
    expect(new TextDecoder().decode(state.sessions[0]!.buffer)).toBe('fresh output')
    // 相同内容不产生新引用
    expect(terminalReplayReplaced(state, 's1', bytes('fresh output'))).toBe(state)
    expect(terminalReplayReplaced(state, 'ghost', bytes('x'))).toBe(state)
  })

  it('state 迁移：connected → reconnecting → connected', () => {
    let state = terminalSessionOpened(createTerminalState(), sessionInfo())
    state = terminalStateMigrated(state, 's1', 'reconnecting')
    expect(state.sessions[0]!.info.state).toBe('reconnecting')
    state = terminalStateMigrated(state, 's1', 'connected')
    expect(state.sessions[0]!.info.state).toBe('connected')
    // 同状态不产生新引用；未知会话忽略
    expect(terminalStateMigrated(state, 's1', 'connected')).toBe(state)
    expect(terminalStateMigrated(state, 'ghost', 'closed')).toBe(state)
  })

  it('exit 记录退出码并置 closed', () => {
    let state = terminalSessionOpened(createTerminalState(), sessionInfo())
    state = terminalExited(state, 's1', 127)
    expect(state.sessions[0]!.exitCode).toBe(127)
    expect(state.sessions[0]!.info.state).toBe('closed')
    // code=null（信号终止）也允许
    state = terminalExited(state, 's1', null)
    expect(state.sessions[0]!.exitCode).toBeNull()
    expect(terminalExited(state, 'ghost', 0)).toBe(state)
  })

  it('applyPtyEvent 把三类事件路由到对应 reducer；非法 base64 帧按空处理', () => {
    let state = terminalSessionOpened(createTerminalState(), sessionInfo())
    state = applyPtyEvent(state, {
      type: 'pty:data',
      sessionId: 's1',
      data: utf8ToBase64('data'),
    })
    expect(new TextDecoder().decode(state.sessions[0]!.buffer)).toBe('data')
    state = applyPtyEvent(state, { type: 'pty:state', sessionId: 's1', state: 'reconnecting' })
    expect(state.sessions[0]!.info.state).toBe('reconnecting')
    state = applyPtyEvent(state, { type: 'pty:exit', sessionId: 's1', code: 2 })
    expect(state.sessions[0]!.info.state).toBe('closed')
    expect(state.sessions[0]!.exitCode).toBe(2)
  })
})

describe('store reducer：标签列表维护', () => {
  it('open 新会话并自动激活；close 移除并回落激活', () => {
    let state = terminalSessionOpened(createTerminalState(), sessionInfo({ sessionId: 'a' }))
    state = terminalSessionOpened(state, sessionInfo({ sessionId: 'b' }))
    expect(state.activeId).toBe('b')
    state = terminalSessionClosed(state, 'b')
    expect(state.sessions.map(s => s.info.sessionId)).toEqual(['a'])
    expect(state.activeId).toBe('a')
    state = terminalSessionClosed(state, 'a')
    expect(state.activeId).toBeNull()
    expect(terminalSessionClosed(state, 'ghost')).toBe(state)
  })

  it('pty_list 恢复：以 Rust 侧为准替换标签，保留本地缓冲，激活回落', () => {
    let state = terminalSessionOpened(createTerminalState(), sessionInfo({ sessionId: 'a', title: 'old' }))
    state = terminalDataAppended(state, 'a', bytes('kept'))
    state = terminalSessionsLoaded(state, [
      sessionInfo({ sessionId: 'a', title: 'restored' }),
      sessionInfo({ sessionId: 'c', kind: 'ssh', title: 'host' }),
    ])
    expect(state.sessions.map(s => s.info.sessionId)).toEqual(['a', 'c'])
    expect(state.sessions[0]!.info.title).toBe('restored')
    expect(new TextDecoder().decode(state.sessions[0]!.buffer)).toBe('kept')
    expect(state.activeId).toBe('a')

    const gone = terminalSessionsLoaded(state, [sessionInfo({ sessionId: 'c' })])
    expect(gone.activeId).toBe('c')
    expect(terminalSessionsLoaded(gone, []).activeId).toBeNull()
  })

  it('activate 只接受存在会话的 id', () => {
    let state = terminalSessionOpened(createTerminalState(), sessionInfo({ sessionId: 'a' }))
    state = terminalSessionOpened(state, sessionInfo({ sessionId: 'b' }))
    state = terminalActiveChanged(state, 'a')
    expect(state.activeId).toBe('a')
    expect(terminalActiveChanged(state, 'ghost')).toBe(state)
    expect(terminalActiveChanged(state, 'a')).toBe(state)
  })
})

// ==== SSH 表单校验纯函数 ====

describe('SSH 表单', () => {
  it('parseSshPort：空串默认 22；数字越界/非法返回 null', () => {
    expect(parseSshPort('')).toBe(SSH_PORT_DEFAULT)
    expect(parseSshPort('  2222 ')).toBe(2222)
    expect(parseSshPort('1')).toBe(1)
    expect(parseSshPort('65535')).toBe(65535)
    expect(parseSshPort('0')).toBeNull()
    expect(parseSshPort('65536')).toBeNull()
    expect(parseSshPort('-1')).toBeNull()
    expect(parseSshPort('22x')).toBeNull()
    expect(parseSshPort('22.5')).toBeNull()
  })

  it('validateSshDraft：host 必填优先于 port 校验', () => {
    const empty: SshFormDraft = { host: '', port: '', user: '' }
    expect(validateSshDraft(empty)).toBe('terminal.hostRequired')
    expect(validateSshDraft({ host: ' h ', port: '99999', user: '' })).toBe('terminal.portInvalid')
    expect(validateSshDraft({ host: 'h', port: '', user: '' })).toBeNull()
  })

  it('buildSshRequest：合法草稿 → pty_open 请求（user 空 → null，port 空 → 22）', () => {
    expect(buildSshRequest({ host: 'example.com', port: '', user: '' }, 80, 24)).toEqual({
      kind: 'ssh',
      host: 'example.com',
      port: 22,
      user: null,
      cols: 80,
      rows: 24,
    })
    expect(buildSshRequest({ host: ' h ', port: ' 2200 ', user: ' root ' }, 80, 24)).toEqual({
      kind: 'ssh',
      host: 'h',
      port: 2200,
      user: 'root',
      cols: 80,
      rows: 24,
    })
    expect(buildSshRequest(createSshDraft(), 80, 24)).toBeNull()
  })
})

// ==== 控制器：命令调用参数形状 + 事件归并 ====

interface Harness {
  controller: ReturnType<typeof createTerminalController>
  events: PtyEvent[]
  emit: (event: PtyEvent) => void
  calls: { command: string; args: Record<string, unknown> }[]
  failNext: (command: string) => void
}

function harness(): Harness {
  const calls: Harness['calls'] = []
  const failing = new Set<string>()
  let emit: (event: PtyEvent) => void = () => {}
  const controller = createTerminalController({
    ptyOpen: async request => {
      calls.push({ command: 'pty_open', args: { request } })
      if (failing.has('pty_open')) return err()
      return ok({ sessionId: `s-${calls.length}` })
    },
    ptyWrite: async (sessionId, data) => {
      calls.push({ command: 'pty_write', args: { sessionId, data } })
      return failing.has('pty_write') ? err() : ok(null)
    },
    ptyResize: async (sessionId, cols, rows) => {
      calls.push({ command: 'pty_resize', args: { sessionId, cols, rows } })
      return failing.has('pty_resize') ? err() : ok(null)
    },
    ptyClose: async sessionId => {
      calls.push({ command: 'pty_close', args: { sessionId } })
      return failing.has('pty_close') ? err() : ok(null)
    },
    ptyList: async () => {
      calls.push({ command: 'pty_list', args: {} })
      return failing.has('pty_list') ? err() : ok({ sessions: [] })
    },
    ptyReplay: async sessionId => {
      calls.push({ command: 'pty_replay', args: { sessionId } })
      return failing.has('pty_replay') ? err() : ok({ data: '' })
    },
    onPtyEvent: handler => {
      emit = handler
      return () => {}
    },
  })
  return {
    controller,
    events: [],
    emit: event => emit(event),
    calls,
    failNext: command => failing.add(command),
  }
}

describe('控制器', () => {
  it('事件 → store 归并：data 追加、state 迁移、exit', async () => {
    const h = harness()
    const seen: string[] = []
    h.controller.subscribe(state => seen.push(state.sessions.map(s => s.info.state).join(',')))

    const id = await h.controller.openLocal(80, 24)
    expect(id).not.toBeNull()
    h.emit({ type: 'pty:data', sessionId: id!, data: utf8ToBase64('out\r\n') })
    h.emit({ type: 'pty:state', sessionId: id!, state: 'reconnecting' })
    h.emit({ type: 'pty:exit', sessionId: id!, code: 0 })

    const state = h.controller.getState()
    expect(new TextDecoder().decode(state.sessions[0]!.buffer)).toBe('out\r\n')
    expect(state.sessions[0]!.info.state).toBe('closed')
    expect(state.sessions[0]!.exitCode).toBe(0)
    // subscribe 至少收到 open + data + state + exit 四次提交
    expect(seen.length).toBeGreaterThanOrEqual(4)
  })

  it('pty_open 参数形状（local / ssh）', async () => {
    const h = harness()
    await h.controller.openLocal(100, 30)
    expect(h.calls[0]).toEqual({
      command: 'pty_open',
      args: { request: { kind: 'local', cols: 100, rows: 30 } },
    })
    await h.controller.openSsh({ host: 'example.com', port: '2222', user: 'root' }, 90, 25)
    expect(h.calls[1]).toEqual({
      command: 'pty_open',
      args: {
        request: { kind: 'ssh', host: 'example.com', port: 2222, user: 'root', cols: 90, rows: 25 },
      },
    })
  })

  it('write 走 base64；resize/close 参数形状', async () => {
    const h = harness()
    const id = await h.controller.openLocal(80, 24)
    h.calls.length = 0
    await h.controller.write(id!, 'ls\n')
    await h.controller.resize(id!, 120, 40)
    await h.controller.close(id!)
    expect(h.calls.map(call => call.command)).toEqual(['pty_write', 'pty_resize', 'pty_close'])
    expect(h.calls[0]!.args).toEqual({ sessionId: id, data: utf8ToBase64('ls\n') })
    expect(h.calls[1]!.args).toEqual({ sessionId: id, cols: 120, rows: 40 })
    expect(h.calls[2]!.args).toEqual({ sessionId: id })
    expect(h.controller.getState().sessions).toHaveLength(0)
  })

  it('replay 替换缓冲；close 失败保留标签', async () => {
    const h = harness()
    const id = await h.controller.openLocal(80, 24)
    h.failNext('pty_close')
    h.calls.length = 0
    h.controller.replay(id!)
    const replayed = await h.controller.replay(id!)
    expect(replayed).toEqual(new Uint8Array(0))
    expect(h.calls[0]).toEqual({ command: 'pty_replay', args: { sessionId: id } })
    expect(await h.controller.close(id!)).toBe(false)
    expect(h.controller.getState().sessions).toHaveLength(1)
  })

  it('restore 调用 pty_list 并装载会话；失败不炸', async () => {
    const h = harness()
    expect(await h.controller.restore()).toBe(true)
    expect(h.calls[0]!.command).toBe('pty_list')
    h.failNext('pty_list')
    expect(await h.controller.restore()).toBe(false)
  })

  it('非法 SSH 草稿不发起命令；dispose 后命令静默', async () => {
    const h = harness()
    expect(await h.controller.openSsh(createSshDraft(), 80, 24)).toBeNull()
    expect(h.calls).toHaveLength(0)
    h.controller.dispose()
    expect(await h.controller.openLocal(80, 24)).toBeNull()
  })

  it('畸形事件（未知会话 / 坏 base64）不影响既有会话', async () => {
    const h = harness()
    const id = await h.controller.openLocal(80, 24)
    h.emit({ type: 'pty:data', sessionId: 'ghost', data: '' })
    h.emit({ type: 'pty:data', sessionId: id!, data: '@@@bad@@@' })
    h.emit({ type: 'pty:state', sessionId: 'ghost', state: 'closed' })
    const state = h.controller.getState()
    expect(state.sessions).toHaveLength(1)
    expect(state.sessions[0]!.buffer).toHaveLength(0)
  })
})

// ==== 依赖封装的常量与守卫接线 ====

describe('默认依赖与通道封装', () => {
  it('defaultTerminalDeps 提供全部命令封装', () => {
    const deps = defaultTerminalDeps()
    for (const key of ['ptyOpen', 'ptyWrite', 'ptyResize', 'ptyClose', 'ptyList', 'ptyReplay', 'onPtyEvent']) {
      expect(typeof deps[key as keyof typeof deps]).toBe('function')
    }
  })

  it('isPtySessionInfo 拒绝非对象', () => {
    expect(isPtySessionInfo(undefined)).toBe(false)
    expect(isPtySessionInfo(42)).toBe(false)
  })

  it('onPtyEvent 在非 Tauri 环境返回 no-op 取消函数（node 测试环境）', () => {
    const handler = vi.fn()
    const off = defaultTerminalDeps().onPtyEvent(handler)
    expect(() => off()).not.toThrow()
    expect(handler).not.toHaveBeenCalled()
  })

  it('onPtyEvent 给裸载荷注入通道 type（Rust 事件不带 type；回归：曾整帧被守卫丢弃）', async () => {
    const handlers = new Map<number, (raw: unknown) => void>()
    let nextId = 1
    const internals = {
      transformCallback: (cb: (raw: unknown) => void) => {
        const id = nextId++
        handlers.set(id, cb)
        return id
      },
      invoke: async (cmd: string) => (cmd === 'plugin:event|listen' ? nextId - 1 : undefined),
    }
    // node 测试环境无 window：临时挂一个全局替身供模块内 internals() 读取。
    const scope = globalThis as unknown as { window?: { __TAURI_INTERNALS__?: unknown } }
    const previousWindow = scope.window
    scope.window = { __TAURI_INTERNALS__: internals }
    try {
      const received: Array<Record<string, unknown>> = []
      const off = onPtyEvent(event => received.push(event as unknown as Record<string, unknown>))
      // 等 listen 完成（transformCallback 同步注册，invoke 异步返回）。
      await new Promise(resolve => setTimeout(resolve, 0))
      const callbacks = [...handlers.values()]
      expect(callbacks).toHaveLength(3)
      const dataCb = callbacks[0] as (raw: unknown) => void
      const stateCb = callbacks[1] as (raw: unknown) => void
      const exitCb = callbacks[2] as (raw: unknown) => void
      // Tauri 包封形态：{ event, payload, id }。
      dataCb({ event: 'pty:data', id: 1, payload: { sessionId: 's1', data: btoa('hi') } })
      stateCb({ event: 'pty:state', id: 2, payload: { sessionId: 's1', state: 'connected' } })
      exitCb({ event: 'pty:exit', id: 3, payload: { sessionId: 's1', code: 0 } })
      // 畸形帧丢弃。
      dataCb({ event: 'pty:data', id: 4, payload: { sessionId: '', data: btoa('x') } })
      expect(received).toEqual([
        { type: 'pty:data', sessionId: 's1', data: btoa('hi') },
        { type: 'pty:state', sessionId: 's1', state: 'connected', },
        { type: 'pty:exit', sessionId: 's1', code: 0 },
      ])
      off()
    } finally {
      scope.window = previousWindow
    }
  })
})
