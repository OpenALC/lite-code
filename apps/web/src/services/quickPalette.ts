// Ctrl+P 命令面板里的工作区/历史会话匹配（纯函数，便于测试）。
import type { HistorySummary } from './bridge'
import type { WorkspaceEntry } from './workspaceStore'

export interface QuickNavMatches {
  chats: HistorySummary[]
  workspaces: WorkspaceEntry[]
}

export function matchQuickNav(
  chats: readonly HistorySummary[],
  workspaces: readonly WorkspaceEntry[],
  query: string,
  limit = 5,
): QuickNavMatches {
  const term = query.trim().toLocaleLowerCase()
  if (!term) return { chats: [], workspaces: [] }
  return {
    chats: chats
      .filter(item =>
        `${item.title} ${item.model} ${item.mode} ${item.workspace} ${item.excerpt}`
          .toLocaleLowerCase()
          .includes(term),
      )
      .slice(0, limit),
    workspaces: workspaces
      .filter(item =>
        `${item.name} ${item.root}`.toLocaleLowerCase().includes(term),
      )
      .slice(0, limit),
  }
}
