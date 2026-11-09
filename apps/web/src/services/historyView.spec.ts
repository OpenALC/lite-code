import { describe, expect, it } from 'vitest'

import type { HistorySummary } from './bridge'
import {
  SHOW_ARCHIVED_STORAGE_KEY,
  historyInWorkspace,
  historyMatchesQuery,
  loadShowArchived,
  saveShowArchived,
  splitHistoryItems,
} from './historyView'

function item(overrides: Partial<HistorySummary> = {}): HistorySummary {
  return {
    id: 's-1',
    title: '重构历史面板',
    savedAt: 1,
    workspace: '/work/app',
    mode: 'plan',
    model: 'stub-chat',
    turn: 1,
    messages: 2,
    archived: false,
    excerpt: '给历史会话补上重命名按钮',
    ...overrides,
  }
}

describe('historyMatchesQuery', () => {
  it('空白词匹配全部，其余字段大小写不敏感', () => {
    const source = item()
    expect(historyMatchesQuery(source, '   ')).toBe(true)
    expect(historyMatchesQuery(source, '历史')).toBe(true)
    expect(historyMatchesQuery(source, 'STUB-CHAT')).toBe(true)
    expect(historyMatchesQuery(source, 'PLAN')).toBe(true)
    expect(historyMatchesQuery(source, '/WORK')).toBe(true)
    expect(historyMatchesQuery(source, '重命名按钮')).toBe(true)
    expect(historyMatchesQuery(source, 'not-there')).toBe(false)
  })
})

describe('historyInWorkspace', () => {
  it('比较工作区路径；无 workspace 时只保留未归属项', () => {
    expect(historyInWorkspace(item(), '/work/app')).toBe(true)
    expect(historyInWorkspace(item({ workspace: 'C:\\Work\\App' }), 'c:/work/app')).toBe(true)
    expect(historyInWorkspace(item(), '/work/other')).toBe(false)
    expect(historyInWorkspace(item({ workspace: '' }), '')).toBe(true)
    expect(historyInWorkspace(item({ workspace: '/work/app' }), '')).toBe(false)
  })
})

describe('splitHistoryItems', () => {
  it('归档项从默认列表分离，顺序保持不变', () => {
    const active = item({ id: 'a' })
    const archived = item({ id: 'b', archived: true })
    const second = item({ id: 'c' })
    const result = splitHistoryItems([active, archived, second], {
      query: '',
      workspace: '/work/app',
    })
    expect(result.active.map(entry => entry.id)).toEqual(['a', 'c'])
    expect(result.archived.map(entry => entry.id)).toEqual(['b'])
  })

  it('同时应用搜索词与工作区过滤', () => {
    const result = splitHistoryItems(
      [
        item({ id: 'a', title: 'alpha' }),
        item({ id: 'b', title: 'beta', archived: true }),
        item({ id: 'c', title: 'alpha archived', archived: true }),
        item({ id: 'd', title: 'alpha other', workspace: '/work/other' }),
        item({ id: 'e', title: 'alpha unassigned', workspace: '' }),
      ],
      { query: 'alpha', workspace: '/work/app' },
    )
    expect(result.active.map(entry => entry.id)).toEqual(['a', 'e'])
    expect(result.archived.map(entry => entry.id)).toEqual(['c'])
  })

  it('空列表返回两个空数组', () => {
    expect(splitHistoryItems([])).toEqual({ active: [], archived: [] })
  })
})

describe('显示已归档偏好', () => {
  function fakeStorage(initial: Record<string, string> = {}): {
    getItem: (key: string) => string | null
    setItem: (key: string, value: string) => void
    data: Record<string, string>
  } {
    const data = { ...initial }
    return {
      getItem: key => data[key] ?? null,
      setItem: (key, value) => {
        data[key] = value
      },
      data,
    }
  }

  it('默认关闭，只认 1 为开启', () => {
    expect(loadShowArchived(fakeStorage())).toBe(false)
    expect(
      loadShowArchived(fakeStorage({ [SHOW_ARCHIVED_STORAGE_KEY]: '1' })),
    ).toBe(true)
    expect(
      loadShowArchived(fakeStorage({ [SHOW_ARCHIVED_STORAGE_KEY]: '0' })),
    ).toBe(false)
    expect(
      loadShowArchived(fakeStorage({ [SHOW_ARCHIVED_STORAGE_KEY]: 'yes' })),
    ).toBe(false)
  })

  it('写入固定 key，null/storage 异常安全降级', () => {
    const storage = fakeStorage()
    saveShowArchived(true, storage)
    expect(storage.data[SHOW_ARCHIVED_STORAGE_KEY]).toBe('1')
    saveShowArchived(false, storage)
    expect(storage.data[SHOW_ARCHIVED_STORAGE_KEY]).toBe('0')

    expect(loadShowArchived(null)).toBe(false)
    expect(() => saveShowArchived(true, null)).not.toThrow()
    expect(() =>
      saveShowArchived(true, {
        getItem: () => null,
        setItem: () => {
          throw new Error('QuotaExceededError')
        },
      }),
    ).not.toThrow()
    expect(
      loadShowArchived({
        getItem: () => {
          throw new Error('denied')
        },
        setItem: () => {},
      }),
    ).toBe(false)
  })
})
