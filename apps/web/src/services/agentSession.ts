// Agent 会话控制器：把桥接命令与核心事件流接到 agentStore 的不可变 reducer。
// 框架无关、依赖注入，视图层只通过 subscribe() 渲染（UI.md §8）。
import type { ApprovalDecision, CoreEvent, SessionMode } from '@lc-proto'

import {
  appendUserMessage,
  applyAgentEvent,
  createAgentState,
  resolveApproval as applyApprovalResolution,
  type AgentError,
  type AgentState,
  type AgentUsage,
} from './agentStore'
import type { AckResult, AgentAttachment, BridgeResult, SessionResult } from './bridge'

/** 会话活动状态：驱动 Agent 头部“思考中/工作中”等提示。 */
export type AgentStatus =
  | 'idle'
  | 'thinking'
  | 'generating'
  | 'working'
  | 'waiting'
  | 'done'
  | 'error'
  | 'archived'

/** 一轮用户消息的总消耗与耗时；usage 事件按 LLM 调用累计。 */
export interface TurnStat {
  turn: number
  input: number
  output: number
  cached: number
  durationMs: number
  stopReason: string
}

/** 可落盘的会话快照（历史列表与续聊使用）。 */
export interface StoredAgentSession {
  id: string
  title: string
  savedAt: number
  mode: SessionMode
  model: string | null
  thinking: string | null
  turnStats: TurnStat[]
  agent: AgentState
}

export interface AgentSessionState {
  /** 当前（或下一次创建会话时使用的）模式；会话建立后以 SessionInfo 为准。 */
  mode: SessionMode
  agent: AgentState | null
  /** 当前轮是否仍在产出事件；approval_required / done / error 会复位。 */
  streaming: boolean
  /** Composer 的待发送文本（跨 docked / 居中两个视图共享）。 */
  draft: string
  /** 下一次创建会话使用的模型显示 id；null = 配置默认模型。 */
  model: string | null
  /** 下一次创建会话使用的思考挡位；null = 配置默认。 */
  thinking: string | null
  /** 命令失败或核心 error 事件；UI 用它渲染错误 banner。 */
  error: AgentError | null
  /** 正在提交的审批 id，多个视图共享禁用态。 */
  resolvingApprovals: number[]
  /** 当前活动状态（思考/生成/工具/审批/完成）。 */
  status: AgentStatus
  /** 已完成轮次的 token 与耗时，按 turn 升序。 */
  turnStats: TurnStat[]
  /** 从历史记录载入的只读视图；发送时会新建会话并带上上下文。 */
  archived: boolean
}

/** 桥接函数与事件订阅均由调用方注入，单测不发真实 IPC。 */
export interface AgentSessionDeps {
  sessionCreate: (
    mode: SessionMode,
    model?: string | null,
    thinking?: string | null,
  ) => Promise<BridgeResult<SessionResult>>
  agentSend: (sessionId: string, text: string) => Promise<BridgeResult<AckResult>>
  agentCancel: (sessionId: string) => Promise<BridgeResult<AckResult>>
  resolveApproval: (
    sessionId: string,
    approvalId: number,
    decision: ApprovalDecision,
  ) => Promise<BridgeResult<AckResult>>
  onCoreEvent: (handler: (event: CoreEvent) => void) => () => void
  /** yolo 二次确认；缺省视为拒绝（fail-closed）。 */
  confirmYolo?: () => boolean | Promise<boolean>
}

export interface SwitchModeOptions {
  /** 视图层已用面板内确认条完成确认时置 true，避免重复弹确认。 */
  confirmed?: boolean
}

export interface AgentSessionController {
  getState(): AgentSessionState
  subscribe(listener: (state: AgentSessionState) => void): () => void
  setDraft(text: string): void
  /** 选择下一次会话的模型/思考挡位；活跃会话中不允许改变。 */
  setModel(model: string | null, thinking: string | null): void
  /** 载入历史记录为只读视图；发送时新建会话并带上最近上下文。 */
  loadHistory(session: StoredAgentSession): boolean
  /** 丢弃当前会话（不取消运行中的轮次），下一次发送重新创建。 */
  resetSession(): boolean
  send(text?: string, attachments?: AgentAttachment[]): Promise<void>
  cancel(): Promise<void>
  resolve(approvalId: number, decision: ApprovalDecision): Promise<void>
  switchMode(mode: SessionMode, options?: SwitchModeOptions): Promise<boolean>
  dispose(): void
}

const HISTORY_CONTEXT_CHARS = 12_000
const HISTORY_CONTEXT_MESSAGES = 12
export const HISTORY_APPROVAL_EXPIRED = '历史会话：该审批已失效'

function toAgentError(error: { code: AgentError['code']; message: string }): AgentError {
  return { code: error.code, message: error.message }
}

/** 事件归属的会话 id；全局 error 没有会话字段。 */
function eventSessionId(event: CoreEvent): string | null {
  if (event.type === 'error') return null
  if (event.type === 'agent:session_state') return event.session.session_id
  if ('session_id' in event) return event.session_id
  return null
}

function emptyUsage(): AgentUsage {
  return { input: 0, output: 0, cached: 0 }
}

function buildHistoryPreamble(messages: AgentState['messages']): string {
  const lines = messages
    .filter(message => message.role !== 'system' && message.text.trim())
    .slice(-HISTORY_CONTEXT_MESSAGES)
    .map(
      message =>
        `${message.role === 'user' ? '用户' : '助手'}：${message.text.trim()}`,
    )
  if (lines.length === 0) return ''
  const joined = lines.join('\n\n')
  const clipped =
    joined.length > HISTORY_CONTEXT_CHARS
      ? joined.slice(joined.length - HISTORY_CONTEXT_CHARS)
      : joined
  return `以下是之前会话的记录，请在此基础上继续回答：\n\n${clipped}`
}

/** 历史载入时清掉过期审批，避免用户点击一个不存在的审批。 */
function sanitizeHistoryAgent(agent: AgentState): AgentState {
  return {
    ...agent,
    approvals: [],
    tools: agent.tools.map(tool =>
      tool.status === 'approval'
        ? { ...tool, status: 'error', text: HISTORY_APPROVAL_EXPIRED }
        : tool,
    ),
  }
}

export function createAgentSession(deps: AgentSessionDeps): AgentSessionController {
  let state: AgentSessionState = {
    mode: 'auto',
    agent: null,
    streaming: false,
    draft: '',
    model: null,
    thinking: null,
    error: null,
    resolvingApprovals: [],
    status: 'idle',
    turnStats: [],
    archived: false,
  }
  const listeners = new Set<(state: AgentSessionState) => void>()
  let disposed = false
  /** 防止 sessionCreate 期间的并发发送（此时 streaming 还没置位）。 */
  let sending = false
  let turnUsage: AgentUsage = emptyUsage()
  let turnStartedAt = 0
  let historyPreamble: string | null = null

  function commit(next: AgentSessionState): void {
    state = next
    if (disposed) return
    // 复制 listener 集合：回调里取消订阅不应影响本次通知。
    for (const listener of [...listeners]) listener(state)
  }

  function patch(next: Partial<AgentSessionState>): void {
    commit({ ...state, ...next })
  }

  function resetTurn(): void {
    turnUsage = emptyUsage()
    turnStartedAt = Date.now()
  }

  function finalizeTurn(
    stats: TurnStat[],
    turn: number,
    stopReason: string,
  ): TurnStat[] {
    if (turnStartedAt === 0) return stats
    const resolvedTurn = turn > 0 ? turn : stats.length + 1
    const stat: TurnStat = {
      turn: resolvedTurn,
      input: turnUsage.input,
      output: turnUsage.output,
      cached: turnUsage.cached,
      durationMs: Math.max(0, Date.now() - turnStartedAt),
      stopReason,
    }
    return [
      ...stats.filter(item => item.turn !== resolvedTurn),
      stat,
    ].sort((a, b) => a.turn - b.turn)
  }

  const offCore = deps.onCoreEvent(event => {
    if (disposed) return
    const current = state.agent

    if (event.type === 'error') {
      const error = toAgentError(event.error)
      if (!current) {
        patch({ error, status: 'error' })
        return
      }
      const agent = applyAgentEvent(current, event)
      const turnStats = finalizeTurn(state.turnStats, agent.turn, 'error')
      turnStartedAt = 0
      turnUsage = emptyUsage()
      commit({
        ...state,
        agent,
        mode: agent.mode,
        streaming: false,
        error,
        status: 'error',
        turnStats,
      })
      return
    }

    if (!current) return
    if (eventSessionId(event) !== current.sessionId) return

    if (event.type === 'agent:usage') {
      turnUsage = {
        input: turnUsage.input + event.input,
        output: turnUsage.output + event.output,
        cached: turnUsage.cached + event.cached,
      }
    }

    const agent = applyAgentEvent(current, event)
    let streaming = state.streaming
    let status = state.status
    if (event.type === 'agent:thinking_delta') status = 'thinking'
    else if (event.type === 'agent:text_delta') status = 'generating'
    else if (event.type === 'agent:tool_start') status = 'working'
    else if (event.type === 'agent:tool_result') {
      status = agent.tools.some(
        tool => tool.status === 'running' || tool.status === 'approval',
      )
        ? 'working'
        : 'thinking'
    } else if (event.type === 'agent:approval_required') status = 'waiting'
    else if (event.type === 'agent:done') status = 'done'

    let turnStats = state.turnStats
    if (event.type === 'agent:done') {
      turnStats = finalizeTurn(turnStats, agent.turn, event.stop_reason)
      turnStartedAt = 0
      turnUsage = emptyUsage()
    }

    if (event.type === 'agent:done' || event.type === 'agent:approval_required') {
      // 审批挂起时本轮没有 token 流，停止按钮应消失；resolve 成功后恢复。
      streaming = false
    } else if (event.type === 'agent:session_state' && event.session.cancelled) {
      streaming = false
    }
    commit({ ...state, agent, mode: agent.mode, streaming, status, turnStats })
  })

  function setModel(model: string | null, thinking: string | null): void {
    if (disposed || (state.agent && !state.archived)) return
    commit({ ...state, model, thinking })
  }

  function loadHistory(session: StoredAgentSession): boolean {
    if (disposed || state.streaming) return false
    const agent = sanitizeHistoryAgent(session.agent)
    historyPreamble = buildHistoryPreamble(agent.messages)
    commit({
      ...state,
      agent,
      mode: session.mode,
      model: session.model,
      thinking: session.thinking,
      turnStats: session.turnStats ?? [],
      archived: true,
      streaming: false,
      status: 'archived',
      error: null,
      resolvingApprovals: [],
      draft: '',
    })
    return true
  }

  function resetSession(): boolean {
    if (disposed || state.streaming) return false
    if (state.agent && !state.archived && state.agent.approvals.length > 0) return false
    historyPreamble = null
    turnStartedAt = 0
    turnUsage = emptyUsage()
    commit({
      ...state,
      agent: null,
      streaming: false,
      archived: false,
      status: 'idle',
      turnStats: [],
      error: null,
      resolvingApprovals: [],
    })
    return true
  }

  async function send(text?: string, attachments: AgentAttachment[] = []): Promise<void> {
    if (disposed || state.streaming || sending) return
    // 活跃会话的审批挂起时本轮处于暂停态，但不应允许再插一条用户消息。
    if (state.agent && !state.archived && state.agent.approvals.length > 0) return
    const content = (text ?? state.draft).trim()
    if (!content && attachments.length === 0) return

    sending = true
    try {
      let current = state.archived ? null : state.agent
      const preamble = state.archived ? historyPreamble : null
      if (!current) {
        const created = await deps.sessionCreate(state.mode, state.model, state.thinking)
        if (disposed) return
        if (!created.ok) {
          patch({ error: toAgentError(created.error) })
          return
        }
        current = createAgentState(created.data.session)
        resetTurn()
        commit({
          ...state,
          agent: current,
          mode: current.mode,
          archived: false,
          error: null,
          status: 'thinking',
          turnStats: [],
        })
      } else if (!state.streaming) {
        resetTurn()
        patch({ status: 'thinking', error: null })
      }

      if (state.streaming) return
      const sessionId = current.sessionId
      const attachmentNames = attachments.map(file => file.name).join(', ')
      const display = [content, attachmentNames ? `[附件: ${attachmentNames}]` : '']
        .filter(Boolean)
        .join('\n\n')
      const wire = [
        preamble,
        content,
        ...attachments.map(
          file => `<attachment name="${file.name}" path="${file.path}">\n${file.text}\n</attachment>`,
        ),
      ]
        .filter(Boolean)
        .join('\n\n')
      historyPreamble = null
      const agent = appendUserMessage(current, display)
      // 乐观更新：用户消息立即入流、草稿清空、进入 streaming；
      // agentSend 失败时再回退 streaming 并保留错误 banner。
      commit({ ...state, agent, streaming: true, draft: '', error: null, status: 'thinking' })

      const sent = await deps.agentSend(sessionId, wire)
      if (disposed) return
      if (!sent.ok) {
        // 只回退本轮；若已切到新会话则不动新会话状态。
        if (state.agent?.sessionId === sessionId) {
          patch({
            streaming: false,
            error: toAgentError(sent.error),
            status: 'error',
          })
        }
      }
    } finally {
      sending = false
    }
  }

  async function cancel(): Promise<void> {
    if (disposed) return
    const current = state.agent
    if (!current || !state.streaming) return

    const cancelled = await deps.agentCancel(current.sessionId)
    if (disposed) return
    if (!cancelled.ok) {
      patch({ error: toAgentError(cancelled.error) })
      return
    }
    if (state.agent?.sessionId === current.sessionId) {
      patch({ streaming: false, status: 'done' })
    }
  }

  async function resolve(approvalId: number, decision: ApprovalDecision): Promise<void> {
    if (disposed) return
    const current = state.agent
    if (!current || state.archived) return
    if (!current.approvals.some(card => card.approvalId === approvalId)) return
    if (state.resolvingApprovals.includes(approvalId)) return

    const sessionId = current.sessionId
    patch({
      resolvingApprovals: [...state.resolvingApprovals, approvalId],
      error: null,
    })

    const resolved = await deps.resolveApproval(sessionId, approvalId, decision)
    if (disposed) return

    const remaining = state.resolvingApprovals.filter(id => id !== approvalId)
    if (!resolved.ok) {
      patch({ resolvingApprovals: remaining, error: toAgentError(resolved.error) })
      return
    }

    // 等待期间可能已切换会话：旧审批不再应用到新会话。
    if (!state.agent || state.agent.sessionId !== sessionId) {
      patch({ resolvingApprovals: remaining })
      return
    }

    const agent = applyApprovalResolution(state.agent, approvalId, decision)
    commit({
      ...state,
      agent,
      mode: agent.mode,
      resolvingApprovals: remaining,
      // Rust 恢复该轮执行，后续事件继续到达直到 done/error/下一次审批。
      streaming: true,
      status: 'working',
      error: null,
    })
  }

  async function switchMode(mode: SessionMode, options: SwitchModeOptions = {}): Promise<boolean> {
    if (disposed) return false
    // 运行中/发送中/有审批挂起时切换会与工具执行交叉；UI 同时禁用选择器，这里再兜底。
    if (state.streaming || sending) return false
    if (state.agent && !state.archived && state.agent.approvals.length > 0) return false
    if (state.mode === mode && (!state.agent || state.agent.mode === mode)) return true

    if (mode === 'yolo' && options.confirmed !== true) {
      const confirmed = deps.confirmYolo ? await deps.confirmYolo() : false
      if (!confirmed) return false
    }
    if (disposed) return false

    if (!state.agent || state.archived) {
      // 首个会话尚未创建：只记下模式，首次发送时按该模式 sessionCreate。
      historyPreamble = null
      patch({
        mode,
        agent: state.archived ? null : state.agent,
        archived: false,
        status: 'idle',
        turnStats: [],
        error: null,
      })
      return true
    }

    // 契约暂无会话内切模式命令：建新会话替换旧会话（消息流清空）。
    const created = await deps.sessionCreate(mode, state.model, state.thinking)
    if (disposed) return false
    if (!created.ok) {
      patch({ error: toAgentError(created.error) })
      return false
    }
    const agent = createAgentState(created.data.session)
    historyPreamble = null
    turnStartedAt = 0
    turnUsage = emptyUsage()
    patch({
      agent,
      mode: agent.mode,
      streaming: false,
      archived: false,
      status: 'idle',
      turnStats: [],
      error: null,
      resolvingApprovals: [],
    })
    return true
  }

  function setDraft(text: string): void {
    if (disposed || text === state.draft) return
    patch({ draft: text })
  }

  function subscribe(listener: (next: AgentSessionState) => void): () => void {
    listeners.add(listener)
    return () => {
      listeners.delete(listener)
    }
  }

  function dispose(): void {
    if (disposed) return
    disposed = true
    offCore()
    listeners.clear()
  }

  return {
    getState: () => state,
    subscribe,
    setDraft,
    setModel,
    loadHistory,
    resetSession,
    send,
    cancel,
    resolve,
    switchMode,
    dispose,
  }
}
