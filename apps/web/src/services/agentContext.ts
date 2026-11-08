// 编辑器选中代码 → Agent 对话框的跨组件通道。
// 右栏关闭时 App 仍订阅以打开 Agent；AgentPanel 挂载后 drain 待处理上下文。
export interface AgentCodeContext {
  path: string
  language: string
  text: string
  startLine: number
  endLine: number
}

const listeners = new Set<(context: AgentCodeContext) => void>()
let pending: AgentCodeContext[] = []

export function pushAgentContext(context: AgentCodeContext): void {
  if (!context.text.trim()) return
  pending.push(context)
  for (const listener of [...listeners]) listener(context)
}

/** 取走全部待处理上下文（AgentPanel 首次挂载时调用）。 */
export function drainAgentContexts(): AgentCodeContext[] {
  const items = pending
  pending = []
  return items
}

/** AgentPanel 已处理某个上下文后从 pending 移除，避免下次挂载重复。 */
export function consumeAgentContext(context: AgentCodeContext): void {
  pending = pending.filter(item => item !== context)
}

export function subscribeAgentContext(
  listener: (context: AgentCodeContext) => void,
): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}
