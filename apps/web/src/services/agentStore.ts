// Agent 会话状态与事件 reducer（docs/UI.md §3、docs/AGENT.md §5.4）。
// 框架无关：只做不可变状态变换，视图层自行通过 subscribe() 消费（UI.md §8）。
// 事件与 DTO 类型单一来源：@lc-proto（crates/lc-proto codegen 产物）。
import type {
  ApprovalDecision,
  ApprovalSource,
  CoreEvent,
  ErrorCode,
  SessionInfo,
  SessionMode,
} from '@lc-proto'

export type AgentRole = 'user' | 'assistant' | 'system'

export interface AgentMessage {
  /** 稳定标识，按消息序号生成（纯 reducer 不依赖全局计数器） */
  id: string
  role: AgentRole
  text: string
  thinking?: string
}

export type ToolStatus = 'running' | 'approval' | 'ok' | 'error'

export interface ToolCard {
  callId: string
  name: string
  /**
   * 工具参数 JSON。契约 v0.3 的 agent:tool_start 只带 call_id 与工具名，
   * 不流式下发参数（LlmEvent::ToolUseArgsDelta 尚未进入 CoreEvent），
   * 因此这里先留空串，视图层显示空摘要。
   */
  argsJson: string
  status: ToolStatus
  /** 工具结果 / 错误原文；running 时不存在 */
  text?: string
}

export interface ApprovalCard {
  approvalId: number
  tool: string
  /** 完整命令原文（审批卡必须展示，AGENT.md §5.4）；非命令类工具可缺省 */
  command?: string
  source: ApprovalSource
  reason: string
}

export interface AgentUsage {
  input: number
  output: number
  cached: number
}

export interface AgentError {
  code: ErrorCode
  message: string
}

export interface AgentState {
  sessionId: string
  mode: SessionMode
  turn: number
  maxTurns: number
  messages: AgentMessage[]
  tools: ToolCard[]
  approvals: ApprovalCard[]
  usage: AgentUsage
  done: null | { stopReason: string }
  error: null | AgentError
}

/** 审批被拒绝后写入工具卡的说明文本（视图层可按需改用 i18n 文案）。 */
export const APPROVAL_DENIED_TEXT = '已拒绝：用户未批准该工具调用'

export function createAgentState(session: SessionInfo): AgentState {
  return {
    sessionId: session.session_id,
    mode: session.mode,
    turn: session.turn,
    maxTurns: session.max_turns,
    messages: [],
    tools: [],
    approvals: [],
    usage: { input: 0, output: 0, cached: 0 },
    done: null,
    error: null,
  }
}

function messageId(messages: readonly AgentMessage[], role: AgentRole): string {
  return `${role}-${messages.length + 1}`
}

/** 对最后一条 assistant 消息应用变换；若末条不是 assistant，则新建一条。 */
function updateAssistant(
  messages: readonly AgentMessage[],
  patch: (message: AgentMessage) => AgentMessage,
): AgentMessage[] {
  const last = messages.length - 1
  if (last >= 0 && messages[last].role === 'assistant') {
    return messages.map((message, index) => (index === last ? patch(message) : message))
  }
  return [
    ...messages,
    patch({ id: messageId(messages, 'assistant'), role: 'assistant', text: '' }),
  ]
}

function lastIndex<T>(items: readonly T[], predicate: (item: T) => boolean): number {
  for (let index = items.length - 1; index >= 0; index--) {
    if (predicate(items[index])) return index
  }
  return -1
}

/**
 * 应用一条核心事件。非本会话事件与未识别的类型原样返回（引用不变）。
 * 所有更新均为不可变更新，调用方持有的旧 state 不会被改写。
 */
export function applyAgentEvent(state: AgentState, event: CoreEvent): AgentState {
  switch (event.type) {
    case 'agent:text_delta': {
      if (event.session_id !== state.sessionId || event.text === '') return state
      // index 不参与排序：按到达顺序追加到最后一条 assistant 消息（契约允许乱序 index）。
      const messages = updateAssistant(state.messages, message => ({
        ...message,
        text: message.text + event.text,
      }))
      return { ...state, messages }
    }

    case 'agent:thinking_delta': {
      if (event.session_id !== state.sessionId || event.text === '') return state
      const messages = updateAssistant(state.messages, message => ({
        ...message,
        thinking: (message.thinking ?? '') + event.text,
      }))
      return { ...state, messages }
    }

    case 'agent:tool_start': {
      if (event.session_id !== state.sessionId) return state
      if (state.tools.some(tool => tool.callId === event.call_id)) return state
      const card: ToolCard = {
        callId: event.call_id,
        name: event.name,
        argsJson: '',
        status: 'running',
      }
      return { ...state, tools: [...state.tools, card] }
    }

    case 'agent:tool_result': {
      if (event.session_id !== state.sessionId) return state
      const index = state.tools.findIndex(tool => tool.callId === event.call_id)
      if (index < 0) return state
      const tools: ToolCard[] = state.tools.map((tool, i) =>
        i === index
          ? { ...tool, status: event.ok ? 'ok' : 'error', text: event.text }
          : tool,
      )
      return { ...state, tools }
    }

    case 'agent:approval_required': {
      if (event.session_id !== state.sessionId) return state
      const prompt = event.approval
      if (state.approvals.some(card => card.approvalId === prompt.approval_id)) return state

      const card: ApprovalCard = {
        approvalId: prompt.approval_id,
        tool: prompt.tool,
        ...(prompt.command !== null && prompt.command !== undefined
          ? { command: prompt.command }
          : {}),
        source: prompt.source,
        reason: prompt.reason,
      }

      // 审批卡没有 callId：按工具名匹配最近一张仍在 running 的卡片，把它切到 approval。
      const toolIndex = lastIndex(
        state.tools,
        tool => tool.name === prompt.tool && tool.status === 'running',
      )
      const tools: ToolCard[] =
        toolIndex < 0
          ? state.tools
          : state.tools.map((tool, i) =>
              i === toolIndex ? { ...tool, status: 'approval' } : tool,
            )

      return { ...state, tools, approvals: [...state.approvals, card] }
    }

    case 'agent:session_state': {
      if (event.session.session_id !== state.sessionId) return state
      return {
        ...state,
        mode: event.session.mode,
        turn: event.session.turn,
        maxTurns: event.session.max_turns,
      }
    }

    case 'agent:usage': {
      if (event.session_id !== state.sessionId) return state
      return {
        ...state,
        usage: { input: event.input, output: event.output, cached: event.cached },
      }
    }

    case 'agent:done': {
      if (event.session_id !== state.sessionId) return state
      return { ...state, done: { stopReason: event.stop_reason } }
    }

    case 'error': {
      return {
        ...state,
        error: { code: event.error.code, message: event.error.message },
      }
    }

    default:
      return state
  }
}

/** 新一轮用户输入：追加 user 消息并清空上一轮的完成/错误标记。 */
export function appendUserMessage(state: AgentState, text: string): AgentState {
  const message: AgentMessage = {
    id: messageId(state.messages, 'user'),
    role: 'user',
    text,
  }
  return {
    ...state,
    messages: [...state.messages, message],
    done: null,
    error: null,
  }
}

/**
 * 用户在审批卡上作出选择：移除审批卡，并把匹配的工具卡切回运行或标记拒绝。
 * 审批卡不携带 callId，匹配规则是「同工具名 + 最近一张 running/approval 卡」。
 * 找不到审批卡时原样返回。
 */
export function resolveApproval(
  state: AgentState,
  approvalId: number,
  decision: ApprovalDecision,
): AgentState {
  const approval = state.approvals.find(card => card.approvalId === approvalId)
  if (!approval) return state

  const approvals = state.approvals.filter(card => card.approvalId !== approvalId)
  const toolIndex = lastIndex(
    state.tools,
    tool =>
      tool.name === approval.tool &&
      (tool.status === 'running' || tool.status === 'approval'),
  )
  const tools: ToolCard[] =
    toolIndex < 0
      ? state.tools
      : state.tools.map((tool, i) => {
          if (i !== toolIndex) return tool
          return decision === 'deny'
            ? { ...tool, status: 'error', text: APPROVAL_DENIED_TEXT }
            : { ...tool, status: 'running' }
        })

  return { ...state, approvals, tools }
}

/**
 * 上下文占用比例（0..1）。input + output 是下一次请求要带上的上下文；
 * cached 是 input 的子集，不重复计入。contextK 单位为千 token，0/非法值安全返回 0。
 */
export function contextRatio(state: AgentState, contextK: number): number {
  if (!Number.isFinite(contextK) || contextK <= 0) return 0
  const used = state.usage.input + state.usage.output
  if (!Number.isFinite(used) || used <= 0) return 0
  return Math.min(1, used / (contextK * 1000))
}
