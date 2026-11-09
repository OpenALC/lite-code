import { describe, expect, it } from 'vitest'

import type { AgentSessionState } from './agentSession'
import { createAgentState } from './agentStore'
import { forkBeforeUserMessage } from './forkSession'

function state(): AgentSessionState {
  const agent = createAgentState({
    session_id: 's1',
    mode: 'auto',
    turn: 2,
    max_turns: 25,
    context_k: 1000,
    compress_at_k: 600,
    cancelled: false,
  })
  agent.messages = [
    { id: 'user-1', role: 'user', text: '第一条' },
    { id: 'assistant-2', role: 'assistant', text: '回答一' },
    { id: 'user-3', role: 'user', text: '第二条' },
    { id: 'assistant-4', role: 'assistant', text: '回答二' },
  ]
  agent.tools = [
    { callId: 'c1', name: 'read_file', argsJson: '', status: 'ok', text: 'x' },
  ]
  agent.approvals = [
    { approvalId: 1, tool: 'shell', source: 'mode', reason: 'reason' },
  ]
  agent.usage = { input: 10, output: 5, cached: 2 }
  return {
    mode: 'auto',
    agent,
    streaming: false,
    draft: '',
    model: 'gpt-5',
    thinking: 'high',
    error: null,
    resolvingApprovals: [],
    status: 'done',
    turnStats: [
      { turn: 1, input: 10, output: 5, cached: 2, durationMs: 100, stopReason: 'end_turn' },
      { turn: 2, input: 20, output: 10, cached: 0, durationMs: 200, stopReason: 'end_turn' },
    ],
    queue: [],
    archived: false,
  }
}

describe('从用户消息分叉', () => {
  it('截断到选中用户消息之前，并清理工具/审批/用量', () => {
    const fork = forkBeforeUserMessage(state(), 2, 'D:/work')

    expect(fork).not.toBeNull()
    expect(fork!.prompt).toBe('第二条')
    expect(fork!.session.title).toBe('第一条')
    expect(fork!.session.workspace).toBe('D:/work')
    expect(fork!.session.agent.messages.map(message => message.id)).toEqual([
      'user-1',
      'assistant-2',
    ])
    expect(fork!.session.agent.tools).toHaveLength(0)
    expect(fork!.session.agent.approvals).toHaveLength(0)
    expect(fork!.session.agent.usage).toEqual({ input: 0, output: 0, cached: 0 })
    expect(fork!.session.agent.turn).toBe(1)
    expect(fork!.session.turnStats.map(stat => stat.turn)).toEqual([1])
  })

  it('第一条消息分叉得到空上下文', () => {
    const fork = forkBeforeUserMessage(state(), 0, 'D:/work')

    expect(fork).not.toBeNull()
    expect(fork!.session.agent.messages).toHaveLength(0)
    expect(fork!.session.turnStats).toHaveLength(0)
    expect(fork!.session.title).toBe('分叉会话')
  })

  it('非用户消息/已归档会话返回 null', () => {
    expect(forkBeforeUserMessage(state(), 1, 'D:/work')).toBeNull()

    const archived = state()
    archived.archived = true
    expect(forkBeforeUserMessage(archived, 2, 'D:/work')).toBeNull()
  })
})
