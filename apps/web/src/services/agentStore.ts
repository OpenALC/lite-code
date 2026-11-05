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

/** 一次上下文压缩的 UI 记录（docs/AGENT.md §6：每次压缩都必须留痕）。 */
export interface CompactionRecord {
  recordId: number
  beforeTokens: number
  afterTokens: number
  removedMessages: number
  summary: string
}

export interface AgentState {
  sessionId: string
  mode: SessionMode
  turn: number
  maxTurns: number
  /** 模型上下文窗口（k token），用量条分母。 */
  contextK: number
  /** 压缩阈值（k token），用量条刻度线。 */
  compressAtK: number
  messages: AgentMessage[]
  tools: ToolCard[]
  approvals: ApprovalCard[]
  usage: AgentUsage
  compactions: CompactionRecord[]
  /** 每次压缩 +1；视图层据此短暂闪烁一次，不影响记录内容。 */
  compactionPulse: number
  done: null | { stopReason: string }
  error: null | AgentError
}

/** 审批被拒绝后写入工具卡的说明文本（视图层可按需改用 i18n 文案）。 */
export const APPROVAL_DENIED_TEXT = '已拒绝：用户未批准该工具调用'

/** 契约缺失/非法时的保守预算，与 Rust 侧 Session 默认值一致。 */
const DEFAULT_CONTEXT_K = 400
const DEFAULT_COMPRESS_AT_K = 300

function positiveK(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value > 0
}

/**
 * 归一上下文预算。压缩阈值必须严格小于窗口；不满足时回落到默认值
 * （窗口本身小于默认阈值时取窗口值，避免刻度落在条外）。
 */
function contextBudget(session: SessionInfo): Pick<AgentState, 'contextK' | 'compressAtK'> {
  const contextK = positiveK(session.context_k) ? session.context_k : DEFAULT_CONTEXT_K
  const compressAtK =
    positiveK(session.compress_at_k) && session.compress_at_k < contextK
      ? session.compress_at_k
      : Math.min(DEFAULT_COMPRESS_AT_K, contextK)
  return { contextK, compressAtK }
}

export function createAgentState(session: SessionInfo): AgentState {
  return {
    sessionId: session.session_id,
    mode: session.mode,
    turn: session.turn,
    maxTurns: session.max_turns,
    ...contextBudget(session),
    messages: [],
    tools: [],
    approvals: [],
    usage: { input: 0, output: 0, cached: 0 },
    compactions: [],
    compactionPulse: 0,
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
        ...contextBudget(event.session),
      }
    }

    case 'agent:compacted': {
      if (event.session_id !== state.sessionId) return state
      if (state.compactions.some(record => record.recordId === event.record_id)) return state
      const record: CompactionRecord = {
        recordId: event.record_id,
        beforeTokens: event.before_tokens,
        afterTokens: event.after_tokens,
        removedMessages: event.removed_messages,
        summary: event.summary,
      }
      return {
        ...state,
        compactions: [...state.compactions, record],
        compactionPulse: state.compactionPulse + 1,
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
 * cached 是 input 的子集，不重复计入。默认用会话自身的 contextK，
 * 显式参数保留给「按其他预算预览」的调用方。0/非法预算安全返回 0。
 */
export function contextRatio(state: AgentState, contextK: number = state.contextK): number {
  if (!Number.isFinite(contextK) || contextK <= 0) return 0
  const used = state.usage.input + state.usage.output
  if (!Number.isFinite(used) || used <= 0) return 0
  return Math.min(1, used / (contextK * 1000))
}

/** 占用是否已达压缩阈值；UI 用它决定用量条变黄。 */
export function beyondCompressThreshold(state: AgentState): boolean {
  if (!Number.isFinite(state.contextK) || state.contextK <= 0) return false
  if (!Number.isFinite(state.compressAtK) || state.compressAtK <= 0) return false
  return contextRatio(state) >= state.compressAtK / state.contextK
}
