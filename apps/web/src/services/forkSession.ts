// 从某条用户消息之前分叉：截断当前会话快照，载入为历史视图后编辑/重试。
// 不修改 Rust 会话；发送时会由控制器新建会话并注入截断后的上下文。
import type { AgentSessionState, StoredAgentSession } from './agentSession'
import type { AgentMessage } from './agentStore'

export interface ForkResult {
  session: StoredAgentSession
  prompt: string
}

function firstUserText(messages: readonly AgentMessage[]): string {
  const user = messages.find(message => message.role === 'user')
  return user?.text.split('\n')[0]?.trim().slice(0, 60) ?? '分叉会话'
}

export function forkBeforeUserMessage(
  state: AgentSessionState,
  userIndex: number,
  workspace: string,
): ForkResult | null {
  const agent = state.agent
  if (!agent || state.archived) return null
  const message = agent.messages[userIndex]
  if (!message || message.role !== 'user') return null

  const messages = agent.messages.slice(0, userIndex)
  const usersBefore = messages.filter(item => item.role === 'user').length
  const turnStats = state.turnStats.filter(stat => stat.turn <= usersBefore)

  return {
    prompt: message.text,
    session: {
      id: agent.sessionId,
      title: firstUserText(messages),
      savedAt: Date.now(),
      workspace,
      mode: state.mode,
      model: state.model,
      thinking: state.thinking,
      turnStats,
      agent: {
        ...agent,
        messages,
        tools: [],
        approvals: [],
        compactions: [],
        usage: { input: 0, output: 0, cached: 0 },
        turn: usersBefore,
        done: null,
        error: null,
      },
    },
  }
}
