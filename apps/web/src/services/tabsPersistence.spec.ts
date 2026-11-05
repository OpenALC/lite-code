import { describe, expect, it } from 'vitest'

import { strategyForTier } from './largeFile'
import { INITIAL_TABS_STATE } from './tabs'
import {
  TABS_STORAGE_KEY,
  loadTabs,
  parseTabs,
  saveTabs,
  serializeTabs,
} from './tabsPersistence'

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

describe('serialize / parse', () => {
  it('往返保留内容、基线、路径与档位', () => {
    const state = {
      activeId: 'a.rs',
      tabs: [
        {
          id: 'a.rs',
          title: 'a.rs',
          path: 'D:/p/a.rs',
          content: 'v2',
          savedContent: 'v1',
          strategy: strategyForTier('reduced' as const),
          dirty: true,
        },
      ],
    }
    const back = parseTabs(serializeTabs(state))
    expect(back?.tabs[0]).toMatchObject({
      id: 'a.rs',
      path: 'D:/p/a.rs',
      content: 'v2',
      savedContent: 'v1',
      dirty: true,
    })
    expect(back?.tabs[0].strategy.tier).toBe('reduced')
    expect(back?.activeId).toBe('a.rs')
  })

  it('空标签集合可恢复；activeId 不存在回落到第一个', () => {
    expect(parseTabs('{"activeId":null,"tabs":[]}')).toEqual({ tabs: [], activeId: null })
    const back = parseTabs(
      '{"activeId":"ghost","tabs":[{"id":"a","title":"a","content":"x"}]}',
    )
    expect(back?.activeId).toBe('a')
    expect(back?.tabs[0].dirty).toBe(false)
  })

  it('损坏 JSON / 非法 tab 被过滤，不抛错', () => {
    expect(parseTabs('{')).toBe(null)
    expect(parseTabs('[1,2]')).toBe(null)
    const back = parseTabs(
      '{"activeId":"ok","tabs":[{"id":"","title":"x","content":"c"},{"id":"ok","title":"ok","content":"c"},42]}',
    )
    expect(back?.tabs.map(t => t.id)).toEqual(['ok'])
    expect(back?.activeId).toBe('ok')
  })
})

describe('load / save', () => {
  it('使用固定 key，往返一致；无 storage 安全降级', () => {
    const storage = fakeStorage()
    saveTabs(INITIAL_TABS_STATE, storage)
    expect(storage.data[TABS_STORAGE_KEY]).toBeTruthy()
    expect(loadTabs(storage)?.tabs[0].id).toBe('sample')
    expect(loadTabs(null)).toBe(null)
    expect(() => saveTabs(INITIAL_TABS_STATE, null)).not.toThrow()
  })

  it('存储配额异常不抛出', () => {
    const storage = {
      getItem: () => null,
      setItem: () => {
        throw new Error('QuotaExceededError')
      },
    }
    expect(() => saveTabs(INITIAL_TABS_STATE, storage)).not.toThrow()
  })
})
