import { describe, expect, it } from 'vitest'

import { DEFAULT_LAYOUT, LAYOUT_LIMITS } from './layoutProfile'
import {
  LAYOUT_STORAGE_KEY,
  loadLayoutPrefs,
  parseLayoutPrefs,
  saveLayoutPrefs,
  serializeLayoutPrefs,
} from './layoutPrefs'

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

describe('parseLayoutPrefs', () => {
  it('空值/损坏 JSON/非对象回落到默认', () => {
    expect(parseLayoutPrefs(null)).toEqual(DEFAULT_LAYOUT)
    expect(parseLayoutPrefs('{')).toEqual(DEFAULT_LAYOUT)
    expect(parseLayoutPrefs('42')).toEqual(DEFAULT_LAYOUT)
  })

  it('越界尺寸被 clamp，缺失字段用默认值', () => {
    const prefs = parseLayoutPrefs('{"sidebarWidth":9999,"agentWidth":-1,"bottomCollapsed":true}')
    expect(prefs.sidebarWidth).toBe(LAYOUT_LIMITS.sidebar[1])
    expect(prefs.agentWidth).toBe(LAYOUT_LIMITS.agent[0])
    expect(prefs.bottomHeight).toBe(DEFAULT_LAYOUT.bottomHeight)
    expect(prefs.bottomCollapsed).toBe(true)
  })
})

describe('serialize / load / save', () => {
  it('序列化前 normalize，往返稳定', () => {
    const json = serializeLayoutPrefs({ sidebarWidth: 9999, agentCollapsed: true })
    expect(JSON.parse(json).sidebarWidth).toBe(LAYOUT_LIMITS.sidebar[1])
    expect(parseLayoutPrefs(json)).toEqual({
      ...DEFAULT_LAYOUT,
      sidebarWidth: LAYOUT_LIMITS.sidebar[1],
      agentCollapsed: true,
    })
  })

  it('load/save 使用固定 key；无 storage 时安全降级', () => {
    const storage = fakeStorage()
    saveLayoutPrefs({ agentWidth: 500 }, storage)
    expect(JSON.parse(storage.data[LAYOUT_STORAGE_KEY]).agentWidth).toBe(500)
    expect(loadLayoutPrefs(storage).agentWidth).toBe(500)
    expect(loadLayoutPrefs(null)).toEqual(DEFAULT_LAYOUT)
    expect(() => saveLayoutPrefs({}, null)).not.toThrow()
  })
})
