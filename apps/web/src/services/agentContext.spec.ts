import { describe, expect, it } from 'vitest'

import {
  consumeAgentContext,
  drainAgentContexts,
  pushAgentContext,
  subscribeAgentContext,
  type AgentCodeContext,
} from './agentContext'

function context(path = 'src/main.rs'): AgentCodeContext {
  return { path, language: 'rust', text: 'fn main() {}', startLine: 1, endLine: 1 }
}

describe('编辑器选区到 Agent 的上下文通道', () => {
  it('推送后可从待处理队列取出，drain 会清空队列', () => {
    const item = context()
    pushAgentContext(item)

    expect(drainAgentContexts()).toEqual([item])
    expect(drainAgentContexts()).toEqual([])
  })

  it('空文本不进入队列', () => {
    pushAgentContext({ ...context(), text: '  \n' })
    expect(drainAgentContexts()).toEqual([])
  })

  it('consume 只移除对应条目，不影响同队列其他选区', () => {
    const first = context('src/a.rs')
    const second = context('src/b.rs')
    pushAgentContext(first)
    pushAgentContext(second)

    consumeAgentContext(first)

    expect(drainAgentContexts()).toEqual([second])
  })

  it('订阅者立即收到推送，取消订阅后不再收到', () => {
    const received: AgentCodeContext[] = []
    const off = subscribeAgentContext(item => received.push(item))
    const first = context('src/a.rs')
    pushAgentContext(first)

    off()
    pushAgentContext(context('src/b.rs'))

    expect(received).toEqual([first])
    drainAgentContexts()
  })
})
