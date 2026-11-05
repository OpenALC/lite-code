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
} from './agentStore'
import type { AckResult, BridgeResult, SessionResult } from './bridge'

export interface AgentSessionState {
  /** 当前（或下一次创建会话时使用的）模式；会话建立后以 SessionInfo 为准。 */
  mode: SessionMode
  agent: AgentState | null
  /** 当前轮是否仍在产出事件；approval_required / done / error 会复位。 */
  streaming: boolean
  /** Composer 的待发送文本（跨 docked / 居中两个视图共享）。 */
  draft: string
  /** 命令失败或核心 error 事件；UI 用它渲染错误 banner。 */
  error: AgentError | null
  /** 正在提交的审批 id，多个视图共享禁用态。 */
  resolvingApprovals: number[]
}

/** 桥接函数与事件订阅均由调用方注入，单测不发真实 IPC。 */
export interface AgentSessionDeps {
  sessionCreate: (mode: SessionMode) => Promise<BridgeResult<SessionResult>>
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
  send(text?: string): Promise<void>
  cancel(): Promise<void>
  resolve(approvalId: number, decision: ApprovalDecision): Promise<void>
  switchMode(mode: SessionMode, options?: SwitchModeOptions): Promise<boolean>
  dispose(): void
}

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

export function createAgentSession(deps: AgentSessionDeps): AgentSessionController {
  let state: AgentSessionState = {
    mode: 'auto',
    agent: null,
    streaming: false,
    draft: '',
    error: null,
    resolvingApprovals: [],
  }
  const listeners = new Set<(state: AgentSessionState) => void>()
  let disposed = false
  /** 防止 sessionCreate 期间的并发发送（此时 streaming 还没置位）。 */
  let sending = false

  function commit(next: AgentSessionState): void {
    state = next
    if (disposed) return
    // 复制 listener 集合：回调里取消订阅不应影响本次通知。
    for (const listener of [...listeners]) listener(state)
  }

  function patch(next: Partial<AgentSessionState>): void {
    commit({ ...state, ...next })
  }

  const offCore = deps.onCoreEvent(event => {
    if (disposed) return
    const current = state.agent

    if (event.type === 'error') {
      const error = toAgentError(event.error)
      if (!current) {
        patch({ error })
        return
      }
      const agent = applyAgentEvent(current, event)
      commit({ ...state, agent, mode: agent.mode, streaming: false, error })
      return
    }

    if (!current) return
    if (eventSessionId(event) !== current.sessionId) return

    const agent = applyAgentEvent(current, event)
    let streaming = state.streaming
    if (event.type === 'agent:done' || event.type === 'agent:approval_required') {
      // 审批挂起时本轮没有 token 流，停止按钮应消失；resolve 成功后恢复。
      streaming = false
    } else if (event.type === 'agent:session_state' && event.session.cancelled) {
      streaming = false
    }
    commit({ ...state, agent, mode: agent.mode, streaming })
  })

  async function send(text?: string): Promise<void> {
    if (disposed || state.streaming || sending) return
    // 审批挂起时本轮处于暂停态，但不应允许再插一条用户消息。
    if (state.agent && state.agent.approvals.length > 0) return
    const content = (text ?? state.draft).trim()
    if (!content) return

    sending = true
    try {
      let current = state.agent
      if (!current) {
        const created = await deps.sessionCreate(state.mode)
        if (disposed) return
        if (!created.ok) {
          patch({ error: toAgentError(created.error) })
          return
        }
        current = createAgentState(created.data.session)
        commit({ ...state, agent: current, mode: current.mode, error: null })
      }

      if (state.streaming) return
      const sessionId = current.sessionId
      const agent = appendUserMessage(current, content)
      // 乐观更新：用户消息立即入流、草稿清空、进入 streaming；
      // agentSend 失败时再回退 streaming 并保留错误 banner。
      commit({ ...state, agent, streaming: true, draft: '', error: null })

      const sent = await deps.agentSend(sessionId, content)
      if (disposed) return
      if (!sent.ok) {
        // 只回退本轮；若已切到新会话则不动新会话状态。
        if (state.agent?.sessionId === sessionId) {
          patch({ streaming: false, error: toAgentError(sent.error) })
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
      patch({ streaming: false })
    }
  }

  async function resolve(approvalId: number, decision: ApprovalDecision): Promise<void> {
    if (disposed) return
    const current = state.agent
    if (!current) return
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
      error: null,
    })
  }

  async function switchMode(mode: SessionMode, options: SwitchModeOptions = {}): Promise<boolean> {
    if (disposed) return false
    // 运行中/发送中/有审批挂起时切换会与工具执行交叉；UI 同时禁用选择器，这里再兜底。
    if (state.streaming || sending) return false
    if (state.agent && state.agent.approvals.length > 0) return false
    if (state.mode === mode && (!state.agent || state.agent.mode === mode)) return true

    if (mode === 'yolo' && options.confirmed !== true) {
      const confirmed = deps.confirmYolo ? await deps.confirmYolo() : false
      if (!confirmed) return false
    }
    if (disposed) return false

    if (!state.agent) {
      // 首个会话尚未创建：只记下模式，首次发送时按该模式 sessionCreate。
      patch({ mode, error: null })
      return true
    }

    // 契约暂无会话内切模式命令：建新会话替换旧会话（消息流清空）。
    const created = await deps.sessionCreate(mode)
    if (disposed) return false
    if (!created.ok) {
      patch({ error: toAgentError(created.error) })
      return false
    }
    const agent = createAgentState(created.data.session)
    patch({
      agent,
      mode: agent.mode,
      streaming: false,
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
    send,
    cancel,
    resolve,
    switchMode,
    dispose,
  }
}
