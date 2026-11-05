import { describe, expect, it } from 'vitest'

import type { SessionInfo } from '@lc-proto'
import {
  applyAgentEvent,
  appendUserMessage,
  APPROVAL_DENIED_TEXT,
  beyondCompressThreshold,
  contextRatio,
  createAgentState,
  resolveApproval,
  type AgentState,
} from './agentStore'

function session(overrides: Partial<SessionInfo> = {}): SessionInfo {
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

interface CompactionOverrides {
  before_tokens?: number
  after_tokens?: number
  removed_messages?: number
  summary?: string
}

function compacted(recordId: number, overrides: CompactionOverrides = {}) {
  return {
    type: 'agent:compacted' as const,
    session_id: 's1',
    record_id: recordId,
    before_tokens: 12300,
    after_tokens: 4100,
    removed_messages: 7,
    summary: '早期对话已摘要',
    ...overrides,
  }
}

function textDelta(text: string, index = 0, sessionId = 's1') {
  return { type: 'agent:text_delta' as const, session_id: sessionId, index, text }
}

function toolStart(callId: string, name: string, sessionId = 's1') {
  return { type: 'agent:tool_start' as const, session_id: sessionId, call_id: callId, name }
}

function approvalRequired(
  approvalId: number,
  tool: string,
  command: string | null = 'rm -rf build',
) {
  return {
    type: 'agent:approval_required' as const,
    session_id: 's1',
    approval: {
      approval_id: approvalId,
      tool,
      command,
      source: 'static_blacklist' as const,
      reason: '命中静态黑名单',
    },
  }
}

describe('createAgentState', () => {
  it('从 SessionInfo 初始化会话字段，消息/工具/审批为空', () => {
    const state = createAgentState(session({ session_id: 's9', mode: 'plan', turn: 2, max_turns: 20 }))
    expect(state).toMatchObject({
      sessionId: 's9',
      mode: 'plan',
      turn: 2,
      maxTurns: 20,
      messages: [],
      tools: [],
      approvals: [],
      usage: { input: 0, output: 0, cached: 0 },
      compactions: [],
      compactionPulse: 0,
      done: null,
      error: null,
    })
  })

  it('从 SessionInfo 读取上下文预算', () => {
    const state = createAgentState(session({ context_k: 128, compress_at_k: 96 }))
    expect(state.contextK).toBe(128)
    expect(state.compressAtK).toBe(96)
  })

  it('预算缺失或非法时回退 400/300；阈值不小于窗口时收敛到窗口内', () => {
    const legacy = {
      ...session(),
      context_k: undefined,
      compress_at_k: undefined,
    } as unknown as SessionInfo
    for (const bad of [
      legacy,
      session({ context_k: Number.NaN, compress_at_k: Number.POSITIVE_INFINITY }),
      session({ context_k: -1, compress_at_k: 0 }),
    ]) {
      const state = createAgentState(bad)
      expect(state.contextK).toBe(400)
      expect(state.compressAtK).toBe(300)
    }

    const small = createAgentState(session({ context_k: 100, compress_at_k: 300 }))
    expect(small.contextK).toBe(100)
    expect(small.compressAtK).toBe(100)
  })
})

describe('文本流', () => {
  it('多个 text_delta 合并到最后一条 assistant 消息，忽略 index 顺序', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, textDelta('Hel', 1))
    state = applyAgentEvent(state, textDelta('lo', 0))
    expect(state.messages).toHaveLength(1)
    expect(state.messages[0]).toMatchObject({ role: 'assistant', text: 'Hello' })
  })

  it('thinking_delta 与 text_delta 合并到同一条消息', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, {
      type: 'agent:thinking_delta',
      session_id: 's1',
      text: '先想',
    })
    state = applyAgentEvent(state, textDelta('答案'))
    state = applyAgentEvent(state, {
      type: 'agent:thinking_delta',
      session_id: 's1',
      text: '再想',
    })
    expect(state.messages).toHaveLength(1)
    expect(state.messages[0]).toMatchObject({
      role: 'assistant',
      text: '答案',
      thinking: '先想再想',
    })
  })

  it('用户消息之后的增量会新建 assistant 消息，而不是续写旧的', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, textDelta('第一轮'))
    state = appendUserMessage(state, '继续')
    state = applyAgentEvent(state, textDelta('第二轮'))
    expect(state.messages.map(m => m.role)).toEqual(['assistant', 'user', 'assistant'])
    expect(state.messages.map(m => m.text)).toEqual(['第一轮', '继续', '第二轮'])
    expect(state.messages[2].id).not.toBe(state.messages[0].id)
  })
})

describe('工具生命周期', () => {
  it('tool_start 新建 running 卡片，tool_result 按 callId 置 ok 并写入结果', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, toolStart('call-1', 'shell'))
    expect(state.tools).toEqual([
      { callId: 'call-1', name: 'shell', argsJson: '', status: 'running' },
    ])

    state = applyAgentEvent(state, {
      type: 'agent:tool_result',
      session_id: 's1',
      call_id: 'call-1',
      ok: true,
      text: 'exit 0',
    })
    expect(state.tools[0]).toMatchObject({ callId: 'call-1', name: 'shell', status: 'ok', text: 'exit 0' })
  })

  it('tool_result 失败置 error；未知 callId 原样返回', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, toolStart('call-1', 'read_file'))
    state = applyAgentEvent(state, {
      type: 'agent:tool_result',
      session_id: 's1',
      call_id: 'call-1',
      ok: false,
      text: 'not found',
    })
    expect(state.tools[0]).toMatchObject({ status: 'error', text: 'not found' })

    expect(
      applyAgentEvent(state, {
        type: 'agent:tool_result',
        session_id: 's1',
        call_id: 'missing',
        ok: true,
        text: 'ignored',
      }),
    ).toBe(state)
  })

  it('重复的 tool_start 不重复建卡', () => {
    let state = createAgentState(session())
    const event = toolStart('call-1', 'shell')
    state = applyAgentEvent(state, event)
    expect(applyAgentEvent(state, event)).toBe(state)
  })
})

describe('审批', () => {
  it('approval_required 加入审批卡并把同名最近 running 工具卡置 approval', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, toolStart('call-1', 'shell'))
    state = applyAgentEvent(state, toolStart('call-2', 'shell'))
    state = applyAgentEvent(state, approvalRequired(7, 'shell'))

    expect(state.approvals).toEqual([
      {
        approvalId: 7,
        tool: 'shell',
        command: 'rm -rf build',
        source: 'static_blacklist',
        reason: '命中静态黑名单',
      },
    ])
    expect(state.tools.map(t => t.status)).toEqual(['running', 'approval'])
  })

  it('没有对应工具卡时仍加入审批卡；command 缺省时不生成该字段', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, approvalRequired(3, 'http', null))
    expect(state.tools).toEqual([])
    expect(state.approvals).toHaveLength(1)
    expect(state.approvals[0].command).toBeUndefined()
    expect('command' in state.approvals[0]).toBe(false)
  })

  it('重复 approval_id 不重复入卡', () => {
    let state = createAgentState(session())
    const event = approvalRequired(7, 'shell')
    state = applyAgentEvent(state, event)
    expect(applyAgentEvent(state, event)).toBe(state)
  })

  it('AllowOnce / AllowSession 移除审批卡并把工具卡置回 running', () => {
    for (const decision of ['allow_once', 'allow_session'] as const) {
      let state = createAgentState(session())
      state = applyAgentEvent(state, toolStart('call-1', 'shell'))
      state = applyAgentEvent(state, approvalRequired(7, 'shell'))
      state = resolveApproval(state, 7, decision)
      expect(state.approvals).toEqual([])
      expect(state.tools[0]).toMatchObject({ callId: 'call-1', status: 'running' })
      expect(state.tools[0].text).toBeUndefined()
    }
  })

  it('Deny 移除审批卡、把工具卡置 error 并说明被拒绝', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, toolStart('call-1', 'shell'))
    state = applyAgentEvent(state, approvalRequired(7, 'shell'))
    state = resolveApproval(state, 7, 'deny')
    expect(state.approvals).toEqual([])
    expect(state.tools[0]).toMatchObject({ status: 'error', text: APPROVAL_DENIED_TEXT })
    expect(state.tools[0].text).toContain('拒绝')
  })

  it('未知 approval_id 原样返回', () => {
    const state = createAgentState(session())
    expect(resolveApproval(state, 99, 'deny')).toBe(state)
  })
})

describe('上下文压缩', () => {
  it('agent:compacted 追加本会话记录，pulse 每次 +1', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, compacted(1))
    state = applyAgentEvent(
      state,
      compacted(2, {
        before_tokens: 9000,
        after_tokens: 3000,
        removed_messages: 3,
        summary: '第二次压缩',
      }),
    )

    expect(state.compactions).toEqual([
      {
        recordId: 1,
        beforeTokens: 12300,
        afterTokens: 4100,
        removedMessages: 7,
        summary: '早期对话已摘要',
      },
      {
        recordId: 2,
        beforeTokens: 9000,
        afterTokens: 3000,
        removedMessages: 3,
        summary: '第二次压缩',
      },
    ])
    expect(state.compactionPulse).toBe(2)
  })

  it('重复 record_id 不追加记录、不增加 pulse', () => {
    let state = createAgentState(session())
    const event = compacted(1)
    state = applyAgentEvent(state, event)
    expect(applyAgentEvent(state, event)).toBe(state)
  })

  it('其他会话的压缩事件不影响本会话', () => {
    const state = createAgentState(session())
    expect(applyAgentEvent(state, { ...compacted(1), session_id: 's2' })).toBe(state)
  })
})

describe('usage、done、session_state', () => {
  it('agent:usage 覆盖三项计数', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, {
      type: 'agent:usage',
      session_id: 's1',
      input: 80000,
      output: 2000,
      cached: 5000,
    })
    expect(state.usage).toEqual({ input: 80000, output: 2000, cached: 5000 })
    expect(contextRatio(state, 100)).toBeCloseTo(0.82)
  })

  it('contextRatio 只计 input + output（cached 是 input 子集），并收敛到 0..1', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, {
      type: 'agent:usage',
      session_id: 's1',
      input: 50000,
      output: 0,
      cached: 50000,
    })
    expect(contextRatio(state, 100)).toBe(0.5)

    state = applyAgentEvent(state, {
      type: 'agent:usage',
      session_id: 's1',
      input: 200000,
      output: 1000,
      cached: 0,
    })
    expect(contextRatio(state, 100)).toBe(1)
  })

  it('contextK 为 0 / 负数 / NaN 时安全返回 0', () => {
    const state = createAgentState(session())
    expect(contextRatio(state, 0)).toBe(0)
    expect(contextRatio(state, -10)).toBe(0)
    expect(contextRatio(state, Number.NaN)).toBe(0)
  })

  it('contextRatio 默认使用 state.contextK，显式参数仍可覆盖预算', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, {
      type: 'agent:usage',
      session_id: 's1',
      input: 200000,
      output: 0,
      cached: 0,
    })
    expect(contextRatio(state)).toBe(0.5)
    expect(contextRatio(state, 100)).toBe(1)
  })

  it('beyondCompressThreshold 在占用达到 compressAtK/contextK 时为 true', () => {
    let state = createAgentState(session())
    expect(beyondCompressThreshold(state)).toBe(false)

    state = applyAgentEvent(state, {
      type: 'agent:usage',
      session_id: 's1',
      input: 299999,
      output: 0,
      cached: 0,
    })
    expect(beyondCompressThreshold(state)).toBe(false)

    state = applyAgentEvent(state, {
      type: 'agent:usage',
      session_id: 's1',
      input: 300000,
      output: 0,
      cached: 0,
    })
    expect(beyondCompressThreshold(state)).toBe(true)

    state = applyAgentEvent(state, {
      type: 'agent:usage',
      session_id: 's1',
      input: 500000,
      output: 0,
      cached: 0,
    })
    expect(beyondCompressThreshold(state)).toBe(true)
  })

  it('agent:done 记录停止原因', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, {
      type: 'agent:done',
      session_id: 's1',
      stop_reason: 'max_turns',
    })
    expect(state.done).toEqual({ stopReason: 'max_turns' })
  })

  it('agent:session_state 同步 mode/turn/maxTurns 与上下文预算，不改 sessionId', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, {
      type: 'agent:session_state',
      session: session({
        session_id: 's1',
        mode: 'yolo',
        turn: 3,
        max_turns: 30,
        context_k: 200,
        compress_at_k: 150,
      }),
    })
    expect(state).toMatchObject({
      sessionId: 's1',
      mode: 'yolo',
      turn: 3,
      maxTurns: 30,
      contextK: 200,
      compressAtK: 150,
    })
  })

  it('error 置错误，appendUserMessage 开启新一轮并清空 done/error', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, { type: 'agent:done', session_id: 's1', stop_reason: 'stop' })
    state = applyAgentEvent(state, {
      type: 'error',
      error: { code: 'timeout', message: 'request timed out' },
    })
    expect(state.error).toEqual({ code: 'timeout', message: 'request timed out' })

    state = appendUserMessage(state, '再试一次')
    expect(state.error).toBeNull()
    expect(state.done).toBeNull()
    expect(state.messages[0]).toMatchObject({ role: 'user', text: '再试一次' })
  })
})

describe('未知事件与跨会话事件', () => {
  it('非 agent 事件原样返回（引用不变）', () => {
    const state = createAgentState(session())
    expect(applyAgentEvent(state, { type: 'fs:entry', path: 'src/main.rs' })).toBe(state)
  })

  it('其他 session 的 agent 事件不影响本会话', () => {
    const state = createAgentState(session())
    expect(applyAgentEvent(state, textDelta('别的会话', 0, 's2'))).toBe(state)
    expect(applyAgentEvent(state, toolStart('call-x', 'shell', 's2'))).toBe(state)
    expect(
      applyAgentEvent(state, {
        type: 'agent:session_state',
        session: session({ session_id: 's2', turn: 5 }),
      }),
    ).toBe(state)
  })
})

describe('不可变更新', () => {
  it('applyAgentEvent 不改写原 state 及其嵌套数组/消息', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, textDelta('A'))
    const snapshot = JSON.parse(JSON.stringify(state)) as AgentState

    const next = applyAgentEvent(state, textDelta('B'))
    expect(next).not.toBe(state)
    expect(next.messages).not.toBe(state.messages)
    expect(next.messages[0]).not.toBe(state.messages[0])
    expect(state).toEqual(snapshot)
    expect(state.messages[0].text).toBe('A')
  })

  it('resolveApproval 不改写原审批数组与工具卡', () => {
    let state = createAgentState(session())
    state = applyAgentEvent(state, toolStart('call-1', 'shell'))
    state = applyAgentEvent(state, approvalRequired(7, 'shell'))
    const snapshot = JSON.parse(JSON.stringify(state)) as AgentState

    const next = resolveApproval(state, 7, 'deny')
    expect(next.tools).not.toBe(state.tools)
    expect(next.tools[0]).not.toBe(state.tools[0])
    expect(next.approvals).not.toBe(state.approvals)
    expect(state).toEqual(snapshot)
    expect(state.tools[0].status).toBe('approval')
    expect(state.approvals).toHaveLength(1)
  })
})
