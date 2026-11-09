import { describe, expect, it } from 'vitest'

import type { HistorySummary } from './bridge'
import { matchQuickNav } from './quickPalette'
import type { WorkspaceEntry } from './workspaceStore'

function chat(overrides: Partial<HistorySummary> = {}): HistorySummary {
  return {
    id: 's-1',
    title: '修复登录',
    savedAt: 1,
    workspace: 'D:/work/app',
    mode: 'auto',
    model: 'gpt-5',
    turn: 1,
    messages: 2,
    archived: false,
    ...overrides,
  }
}

function workspace(overrides: Partial<WorkspaceEntry> = {}): WorkspaceEntry {
  return { root: 'D:/work/app', name: 'app', addedAt: 1, ...overrides }
}

describe('命令面板工作区/会话匹配', () => {
  it('按标题、模型、模式与工作区路径匹配会话', () => {
    const chats = [
      chat(),
      chat({ id: 's-2', title: '更新文档', model: 'claude' }),
    ]
    expect(matchQuickNav(chats, [], '登录').chats.map(item => item.id)).toEqual(['s-1'])
    expect(matchQuickNav(chats, [], 'claude').chats.map(item => item.id)).toEqual(['s-2'])
    expect(matchQuickNav(chats, [], 'auto').chats).toHaveLength(2)
    expect(matchQuickNav(chats, [], 'work/app').chats).toHaveLength(2)
  })

  it('工作区按名称与路径匹配，不区分大小写', () => {
    const workspaces = [
      workspace(),
      workspace({ root: 'D:/work/site', name: 'site' }),
    ]
    expect(matchQuickNav([], workspaces, 'APP').workspaces.map(item => item.name)).toEqual([
      'app',
    ])
    expect(matchQuickNav([], workspaces, 'site').workspaces.map(item => item.name)).toEqual([
      'site',
    ])
  })

  it('空查询返回空，结果按 limit 截断', () => {
    expect(matchQuickNav([chat()], [workspace()], '   ')).toEqual({
      chats: [],
      workspaces: [],
    })
    const many = Array.from({ length: 8 }, (_, index) =>
      chat({ id: `s-${index}`, title: `会话 ${index}` }),
    )
    expect(matchQuickNav(many, [], '会话', 3).chats).toHaveLength(3)
  })
})
