import { describe, expect, it } from 'vitest'

import type { HistorySummary } from './bridge'
import { matchQuickNav, parseQuickQuery } from './quickPalette'
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
    excerpt: '',
    ...overrides,
  }
}

function workspace(overrides: Partial<WorkspaceEntry> = {}): WorkspaceEntry {
  return { root: 'D:/work/app', name: 'app', addedAt: 1, ...overrides }
}

describe('快速打开前缀语法', () => {
  it('> 进入命令模式并去掉前缀空白', () => {
    expect(parseQuickQuery('>')).toEqual({ mode: 'commands', term: '' })
    expect(parseQuickQuery('  > 设置 ')).toEqual({ mode: 'commands', term: '设置' })
  })

  it(':数字 进入跳行模式，其他冒号查询保持普通模式', () => {
    expect(parseQuickQuery(':42')).toEqual({ mode: 'line', term: '42' })
    expect(parseQuickQuery(':12abc')).toEqual({ mode: 'all', term: ':12abc' })
    expect(parseQuickQuery(':')).toEqual({ mode: 'all', term: ':' })
  })

  it('普通查询返回 all 模式并保留原有匹配', () => {
    expect(parseQuickQuery(' login ')).toEqual({ mode: 'all', term: 'login' })
  })
})

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
    expect(
      matchQuickNav(
        [chat({ excerpt: '错误出在 token 刷新逻辑' }), chat({ id: 's-2', excerpt: '样式调整' })],
        [],
        'token',
      ).chats.map(item => item.id),
    ).toEqual(['s-1'])
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
