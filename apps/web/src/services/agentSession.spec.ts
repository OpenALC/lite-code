import type {
  ApprovalDecision,
  CoreEvent,
  ErrorCode,
  SessionInfo,
  SessionMode,
} from '@lc-proto'
import { describe, expect, it } from 'vitest'

import { createAgentSession, type AgentSessionDeps, type AgentSessionState } from './agentSession'
import type { AckResult, BridgeResult, SessionResult } from './bridge'

function sessionInfo(overrides: Partial<SessionInfo> = {}): SessionInfo {
  return {
    session_id: 's1',
    mode: 'auto',
    turn: 0,
    max_turns: 12,
    context_k: 400,
    compress_at_k: 300,
    cancelled: false,
    ...overrides,
  }
}

function okSession(session: SessionInfo): BridgeResult<SessionResult> {
  return { ok: true, data: { kind: 'session', session }, fallback: false }
}

function okAck(): BridgeResult<AckResult> {
  return { ok: true, data: { kind: 'ack' }, fallback: false }
}

function fail<T>(message = 'boom', code: ErrorCode = 'internal'): BridgeResult<T> {
  return { ok: false, error: { code, message, detail: null }, fallback: false }
}

function textDelta(text: string, sessionId = 's1'): CoreEvent {
  return { type: 'agent:text_delta', session_id: sessionId, index: 0, text }
}

function done(stopReason = 'end_turn', sessionId = 's1'): CoreEvent {
  return { type: 'agent:done', session_id: sessionId, stop_reason: stopReason }
}

function approvalRequired(approvalId = 1, sessionId = 's1'): CoreEvent {
  return {
    type: 'agent:approval_required',
    session_id: sessionId,
    approval: {
      approval_id: approvalId,
      tool: 'shell',
      command: 'rm -rf build',
      source: 'static_blacklist',
      reason: '命中静态黑名单',
    },
  }
}

interface HarnessOptions {
  createResult?: (mode: SessionMode) => BridgeResult<SessionResult>
  sendResult?: (sessionId: string, text: string) => BridgeResult<AckResult>
  cancelResult?: (sessionId: string) => BridgeResult<AckResult>
  approvalResult?: (
    sessionId: string,
    approvalId: number,
    decision: ApprovalDecision,
  ) => BridgeResult<AckResult>
  confirmYolo?: () => boolean | Promise<boolean>
}

function harness(options: HarnessOptions = {}) {
  const created: SessionMode[] = []
  const selections: Array<{ model: string | null; thinking: string | null }> = []
  const sent: Array<{ sessionId: string; text: string }> = []
  const cancelled: string[] = []
  const approvals: Array<{
    sessionId: string
    approvalId: number
    decision: ApprovalDecision
  }> = []
  const confirmCalls: number[] = []

  let handler: ((event: CoreEvent) => void) | null = null
  let coreUnsubscribed = false

  const createResult = options.createResult ?? (mode => okSession(sessionInfo({ mode })))
  const sendResult = options.sendResult ?? (() => okAck())
  const cancelResult = options.cancelResult ?? (() => okAck())
  const approvalResult = options.approvalResult ?? (() => okAck())

  const deps: AgentSessionDeps = {
    sessionCreate: async (mode, model = null, thinking = null) => {
      created.push(mode)
      selections.push({ model, thinking })
      return createResult(mode)
    },
    agentSend: async (sessionId, text) => {
      sent.push({ sessionId, text })
      return sendResult(sessionId, text)
    },
    agentCancel: async sessionId => {
      cancelled.push(sessionId)
      return cancelResult(sessionId)
    },
    resolveApproval: async (sessionId, approvalId, decision) => {
      approvals.push({ sessionId, approvalId, decision })
      return approvalResult(sessionId, approvalId, decision)
    },
    onCoreEvent: callback => {
      handler = callback
      return () => {
        handler = null
        coreUnsubscribed = true
      }
    },
  }
  if (options.confirmYolo) {
    const confirm = options.confirmYolo
    deps.confirmYolo = async () => {
      confirmCalls.push(1)
      return confirm()
    }
  }

  const controller = createAgentSession(deps)
  return {
    controller,
    created,
    selections,
    sent,
    cancelled,
    approvals,
    confirmCalls,
    emit(event: CoreEvent): void {
      if (!handler) throw new Error('controller did not subscribe to core events')
      handler(event)
    },
    coreUnsubscribed: () => coreUnsubscribed,
  }
}

describe('会话创建与发送', () => {
  it('首次发送自动 sessionCreate(mode) 再 agentSend，并追加用户消息', async () => {
    const h = harness()
    h.controller.setDraft('hello')

    await h.controller.send()

    expect(h.created).toEqual(['auto'])
    expect(h.sent).toEqual([{ sessionId: 's1', text: 'hello' }])
    const state = h.controller.getState()
    expect(state.agent?.messages).toHaveLength(1)
    expect(state.agent?.messages[0]).toMatchObject({ role: 'user', text: 'hello' })
    expect(state.streaming).toBe(true)
    expect(state.draft).toBe('')
    expect(state.error).toBeNull()
  })

  it('已有会话时复用 sessionId，不再创建', async () => {
    const h = harness()
    await h.controller.send('first')
    h.emit(done())

    await h.controller.send('second')

    expect(h.created).toEqual(['auto'])
    expect(h.sent.map(item => item.text)).toEqual(['first', 'second'])
  })

  it('sessionCreate 失败：不发送、保留草稿并显示错误', async () => {
    const h = harness({ createResult: () => fail<SessionResult>('provider missing') })
    h.controller.setDraft('hello')

    await h.controller.send()

    expect(h.sent).toHaveLength(0)
    const state = h.controller.getState()
    expect(state.agent).toBeNull()
    expect(state.error?.message).toBe('provider missing')
    expect(state.draft).toBe('hello')
    expect(state.streaming).toBe(false)
  })

  it('agentSend 失败：回退 streaming，用户消息与错误同时保留', async () => {
    const h = harness({ sendResult: () => fail<AckResult>('send failed', 'io') })

    await h.controller.send('hello')

    const state = h.controller.getState()
    expect(state.streaming).toBe(false)
    expect(state.error).toMatchObject({ code: 'io', message: 'send failed' })
    expect(state.agent?.messages).toHaveLength(1)
  })

  it('空文本不触发建会话', async () => {
    const h = harness()
    h.controller.setDraft('   ')

    await h.controller.send()

    expect(h.created).toHaveLength(0)
    expect(h.sent).toHaveLength(0)
  })

  it('无会话时 setModel 生效，首次发送透传模型与思考挡位', async () => {
    const h = harness()
    h.controller.setModel('gpt-5', 'high')

    expect(h.controller.getState().model).toBe('gpt-5')
    await h.controller.send('hello')

    expect(h.selections).toEqual([{ model: 'gpt-5', thinking: 'high' }])
  })

  it('已有会话时 setModel 被忽略，resetSession 后新会话生效', async () => {
    const h = harness()
    await h.controller.send('first')
    h.emit(done())

    h.controller.setModel('other', 'low')
    expect(h.controller.getState().model).toBeNull()

    expect(h.controller.resetSession()).toBe(true)
    expect(h.controller.getState().agent).toBeNull()
    h.controller.setModel('other', 'low')
    await h.controller.send('second')

    expect(h.selections).toEqual([
      { model: null, thinking: null },
      { model: 'other', thinking: 'low' },
    ])
  })

  it('运行中 resetSession 返回 false', async () => {
    const h = harness()
    await h.controller.send('first')

    expect(h.controller.resetSession()).toBe(false)
    expect(h.controller.getState().agent).not.toBeNull()
  })

  it('仅附件也能发送：wire 携带 attachment 标签，展示只列文件名', async () => {
    const h = harness()

    await h.controller.send(undefined, [
      { name: 'notes.txt', path: 'D:/tmp/notes.txt', text: 'file body', bytes: 9 },
    ])

    expect(h.sent).toHaveLength(1)
    expect(h.sent[0]?.text).toContain(
      '<attachment name="notes.txt" path="D:/tmp/notes.txt">',
    )
    expect(h.sent[0]?.text).toContain('file body')
    const state = h.controller.getState()
    expect(state.agent?.messages[0]?.text).toBe('[附件: notes.txt]')
    expect(state.draft).toBe('')
  })
})

describe('核心事件归并', () => {
  it('文本/思考/usage/session_state 合并到同一状态', async () => {
    const h = harness()
    await h.controller.send('hi')

    h.emit(textDelta('Hel'))
    h.emit(textDelta('lo'))
    h.emit({ type: 'agent:thinking_delta', session_id: 's1', text: 'plan first' })
    h.emit({ type: 'agent:usage', session_id: 's1', input: 120, output: 30, cached: 10 })
    h.emit({
      type: 'agent:session_state',
      session: sessionInfo({ turn: 2, max_turns: 20 }),
    })

    const state = h.controller.getState()
    const assistant = state.agent?.messages.find(message => message.role === 'assistant')
    expect(assistant?.text).toBe('Hello')
    expect(assistant?.thinking).toBe('plan first')
    expect(state.agent?.usage).toEqual({ input: 120, output: 30, cached: 10 })
    expect(state.agent?.turn).toBe(2)
    expect(state.agent?.maxTurns).toBe(20)
    expect(state.streaming).toBe(true)
  })

  it('agent:compacted 透传到 reducer：记录追加且 pulse +1，不结束本轮', async () => {
    const h = harness()
    await h.controller.send('hi')

    h.emit({
      type: 'agent:compacted',
      session_id: 's1',
      record_id: 3,
      before_tokens: 12300,
      after_tokens: 4100,
      removed_messages: 7,
      summary: '早期对话已摘要',
    })

    const state = h.controller.getState()
    expect(state.agent?.compactions).toEqual([
      {
        recordId: 3,
        beforeTokens: 12300,
        afterTokens: 4100,
        removedMessages: 7,
        summary: '早期对话已摘要',
      },
    ])
    expect(state.agent?.compactionPulse).toBe(1)
    expect(state.streaming).toBe(true)
  })

  it('其它会话的事件被忽略', async () => {
    const h = harness()
    await h.controller.send('hi')

    h.emit(textDelta('nope', 'other'))
    h.emit(done('end_turn', 'other'))

    const state = h.controller.getState()
    expect(state.agent?.messages).toHaveLength(1)
    expect(state.streaming).toBe(true)
  })

  it('agent:done 清除 streaming 并记录停止原因', async () => {
    const h = harness()
    await h.controller.send('hi')

    h.emit(done('max_turns'))

    const state = h.controller.getState()
    expect(state.streaming).toBe(false)
    expect(state.agent?.done).toEqual({ stopReason: 'max_turns' })
  })

  it('error 事件清除 streaming 并写入控制器错误', async () => {
    const h = harness()
    await h.controller.send('hi')

    h.emit({ type: 'error', error: { code: 'timeout', message: 'too slow', detail: null } })

    const state = h.controller.getState()
    expect(state.streaming).toBe(false)
    expect(state.error).toEqual({ code: 'timeout', message: 'too slow' })
    expect(state.agent?.error).toEqual({ code: 'timeout', message: 'too slow' })
  })
})

describe('审批', () => {
  it('approval_required 自动结束当前轮并生成审批卡', async () => {
    const h = harness()
    await h.controller.send('run')
    h.emit({ type: 'agent:tool_start', session_id: 's1', call_id: 'c1', name: 'shell' })

    h.emit(approvalRequired())

    const state = h.controller.getState()
    expect(state.streaming).toBe(false)
    expect(state.agent?.approvals).toEqual([
      {
        approvalId: 1,
        tool: 'shell',
        command: 'rm -rf build',
        source: 'static_blacklist',
        reason: '命中静态黑名单',
      },
    ])
    expect(state.agent?.tools[0].status).toBe('approval')
  })

  it('resolve 成功：先调桥接，成功后用 reducer 移除审批卡', async () => {
    const h = harness()
    await h.controller.send('run')
    h.emit({ type: 'agent:tool_start', session_id: 's1', call_id: 'c1', name: 'shell' })
    h.emit(approvalRequired())

    await h.controller.resolve(1, 'allow_session')

    expect(h.approvals).toEqual([
      { sessionId: 's1', approvalId: 1, decision: 'allow_session' },
    ])
    const state = h.controller.getState()
    expect(state.agent?.approvals).toHaveLength(0)
    expect(state.agent?.tools[0].status).toBe('running')
    expect(state.resolvingApprovals).toEqual([])
    expect(state.streaming).toBe(true)
    expect(state.error).toBeNull()
  })

  it('resolve 失败：审批卡保留、按钮恢复可点、显示错误', async () => {
    const h = harness({ approvalResult: () => fail<AckResult>('approval rejected', 'invalid_argument') })
    await h.controller.send('run')
    h.emit({ type: 'agent:tool_start', session_id: 's1', call_id: 'c1', name: 'shell' })
    h.emit(approvalRequired())

    await h.controller.resolve(1, 'deny')

    const state = h.controller.getState()
    expect(state.agent?.approvals).toHaveLength(1)
    expect(state.agent?.tools[0].status).toBe('approval')
    expect(state.resolvingApprovals).toEqual([])
    expect(state.error).toMatchObject({ code: 'invalid_argument', message: 'approval rejected' })
  })

  it('resolve 期间标记审批 id，重复调用只发一次桥接', async () => {
    const h = harness()
    await h.controller.send('run')
    h.emit({ type: 'agent:tool_start', session_id: 's1', call_id: 'c1', name: 'shell' })
    h.emit(approvalRequired())

    const first = h.controller.resolve(1, 'allow_once')
    const second = h.controller.resolve(1, 'allow_once')
    expect(h.approvals).toHaveLength(1)
    expect(h.controller.getState().resolvingApprovals).toEqual([1])
    await Promise.all([first, second])

    expect(h.approvals).toHaveLength(1)
    expect(h.controller.getState().resolvingApprovals).toEqual([])
  })
})

describe('取消', () => {
  it('cancel 调用 agentCancel 并停止 streaming', async () => {
    const h = harness()
    await h.controller.send('hi')

    await h.controller.cancel()

    expect(h.cancelled).toEqual(['s1'])
    expect(h.controller.getState().streaming).toBe(false)
  })

  it('cancel 失败保留 streaming 并显示错误', async () => {
    const h = harness({ cancelResult: () => fail<AckResult>('cancel failed') })
    await h.controller.send('hi')

    await h.controller.cancel()

    const state = h.controller.getState()
    expect(state.streaming).toBe(true)
    expect(state.error?.message).toBe('cancel failed')
  })
})

describe('模式切换与 yolo 确认', () => {
  it('无确认回调时 yolo 被拒绝（fail-closed）', async () => {
    const h = harness()

    expect(await h.controller.switchMode('yolo')).toBe(false)

    expect(h.controller.getState().mode).toBe('auto')
    expect(h.created).toHaveLength(0)
  })

  it('确认回调返回 false 时不切换，且只询问一次', async () => {
    const h = harness({ confirmYolo: () => false })

    expect(await h.controller.switchMode('yolo')).toBe(false)

    expect(h.confirmCalls).toEqual([1])
    expect(h.controller.getState().mode).toBe('auto')
  })

  it('确认后无会话只记录模式，首次发送才创建', async () => {
    const h = harness({ confirmYolo: () => true })

    expect(await h.controller.switchMode('yolo')).toBe(true)

    expect(h.controller.getState().mode).toBe('yolo')
    expect(h.created).toHaveLength(0)
    expect(h.confirmCalls).toEqual([1])
  })

  it('已有会话时切换模式创建新会话并清空消息流', async () => {
    let next = 0
    const h = harness({
      createResult: mode =>
        okSession(sessionInfo({ session_id: `s${++next}`, mode })),
    })
    h.controller.setModel('gpt-5', 'high')
    await h.controller.send('hi')
    h.emit(done())
    expect(h.controller.getState().agent?.sessionId).toBe('s1')

    expect(await h.controller.switchMode('yolo', { confirmed: true })).toBe(true)

    expect(h.created).toEqual(['auto', 'yolo'])
    expect(h.selections).toEqual([
      { model: 'gpt-5', thinking: 'high' },
      { model: 'gpt-5', thinking: 'high' },
    ])
    expect(h.confirmCalls).toHaveLength(0)
    const state = h.controller.getState()
    expect(state.agent?.sessionId).toBe('s2')
    expect(state.agent?.messages).toHaveLength(0)
    expect(state.mode).toBe('yolo')
  })

  it('视图层已确认时 confirmed 跳过回调；相同模式不重复建会话', async () => {
    const h = harness({ confirmYolo: () => false })

    expect(await h.controller.switchMode('auto')).toBe(true)
    expect(await h.controller.switchMode('yolo', { confirmed: true })).toBe(true)

    expect(h.confirmCalls).toHaveLength(0)
    expect(h.created).toHaveLength(0)
    expect(h.controller.getState().mode).toBe('yolo')
  })

  it('streaming 中拒绝切换模式', async () => {
    const h = harness({ confirmYolo: () => true })
    await h.controller.send('hi')

    expect(await h.controller.switchMode('plan')).toBe(false)
    expect(h.controller.getState().mode).toBe('auto')
  })
})

describe('订阅与不可变状态', () => {
  it('取消订阅后不再收到通知；dispose 取消核心事件订阅', () => {
    const h = harness()
    const seen: AgentSessionState[] = []
    const off = h.controller.subscribe(state => seen.push(state))

    h.controller.setDraft('a')
    off()
    h.controller.setDraft('ab')

    expect(seen).toHaveLength(1)
    expect(h.coreUnsubscribed()).toBe(false)

    h.controller.dispose()
    expect(h.coreUnsubscribed()).toBe(true)
  })

  it('每次更新返回新状态对象，旧快照不被改写', () => {
    const h = harness()
    const before = h.controller.getState()

    h.controller.setDraft('hello')

    expect(before.draft).toBe('')
    expect(h.controller.getState()).not.toBe(before)
    expect(h.controller.getState().draft).toBe('hello')
  })
})
