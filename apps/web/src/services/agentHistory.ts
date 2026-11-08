// Agent 会话历史：把控制器的非流式快照落盘，并在列表/载入时复用。
// 历史只用于查看与续聊；不恢复 Rust 侧会话，续聊由控制器新建会话并注入上下文。
import type {
  AgentSessionController,
  AgentSessionState,
  StoredAgentSession,
} from './agentSession'
import { historyLoad, historySave } from './bridge'

const SAVE_DEBOUNCE_MS = 700
const TOOL_TEXT_LIMIT = 20_000

let flushImpl: (() => Promise<void>) | null = null

/** 列表打开前调用，确保当前会话已落盘。 */
export async function flushHistory(): Promise<void> {
  await flushImpl?.()
}

/** 历史快照可能包含超长工具输出；落盘前截断，避免单文件无限增长。 */
function compactSnapshot(state: AgentSessionState): StoredAgentSession | null {
  const agent = state.agent
  if (!agent || state.streaming || state.archived || agent.messages.length === 0) {
    return null
  }
  const firstUser = agent.messages.find(message => message.role === 'user')
  const title = firstUser?.text.split('\n')[0]?.trim().slice(0, 60) || '未命名会话'
  return {
    id: agent.sessionId,
    title,
    savedAt: Date.now(),
    mode: state.mode,
    model: state.model,
    thinking: state.thinking,
    turnStats: state.turnStats,
    agent: {
      ...agent,
      approvals: [],
      tools: agent.tools.map(tool =>
        tool.text && tool.text.length > TOOL_TEXT_LIMIT
          ? { ...tool, text: `${tool.text.slice(0, TOOL_TEXT_LIMIT)}\n…（历史记录已截断）` }
          : tool,
      ),
    },
  }
}

export interface HistoryRecorder {
  flush(): Promise<void>
  dispose(): void
}

export function createHistoryRecorder(
  controller: AgentSessionController,
): HistoryRecorder {
  let timer: ReturnType<typeof setTimeout> | null = null
  let pending: AgentSessionState | null = null
  let lastSaved = ''

  async function savePending(): Promise<void> {
    if (timer !== null) {
      clearTimeout(timer)
      timer = null
    }
    const state = pending ?? controller.getState()
    pending = null
    const snapshot = compactSnapshot(state)
    if (!snapshot) return
    const json = JSON.stringify(snapshot)
    if (json === lastSaved) return
    const saved = await historySave(snapshot.id, snapshot)
    if (saved.ok) lastSaved = json
  }

  const unsubscribe = controller.subscribe(state => {
    if (!state.agent || state.streaming || state.archived) return
    if (state.agent.messages.length === 0) return
    pending = state
    if (timer !== null) clearTimeout(timer)
    timer = setTimeout(() => {
      void savePending()
    }, SAVE_DEBOUNCE_MS)
  })

  const recorder: HistoryRecorder = {
    flush: savePending,
    dispose(): void {
      unsubscribe()
      if (timer !== null) clearTimeout(timer)
      timer = null
      pending = null
      if (flushImpl === recorder.flush) flushImpl = null
    },
  }
  flushImpl = recorder.flush
  return recorder
}

/** 读取并做最小形状校验；坏数据当不存在处理。 */
export async function loadStoredSession(
  id: string,
): Promise<StoredAgentSession | null> {
  const loaded = await historyLoad(id)
  if (!loaded.ok) return null
  const value = loaded.data as Partial<StoredAgentSession> | null
  if (
    !value ||
    typeof value !== 'object' ||
    !value.agent ||
    typeof value.agent !== 'object' ||
    !Array.isArray(value.agent.messages) ||
    typeof value.agent.sessionId !== 'string'
  ) {
    return null
  }
  return {
    id: typeof value.id === 'string' ? value.id : id,
    title: typeof value.title === 'string' ? value.title : '未命名会话',
    savedAt: Number(value.savedAt ?? 0),
    mode: (value.mode ?? 'auto') as StoredAgentSession['mode'],
    model: typeof value.model === 'string' ? value.model : null,
    thinking: typeof value.thinking === 'string' ? value.thinking : null,
    turnStats: Array.isArray(value.turnStats) ? value.turnStats : [],
    agent: value.agent,
  }
}
