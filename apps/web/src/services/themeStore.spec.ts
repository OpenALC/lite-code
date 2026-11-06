import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  DEFAULT_PREFS,
  FONT_SIZE_DEFAULT,
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  PREFS_STORAGE_KEY,
  TAB_SIZE_DEFAULT,
  WORD_WRAP_DEFAULT,
  loadPrefs,
  normalizePrefs,
  savePrefs,
  setFontSize,
  setTabSize,
  setWordWrap,
  subscribePrefs,
  uiPrefs,
} from './themeStore'

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

afterEach(() => {
  vi.unstubAllGlobals()
  setFontSize(FONT_SIZE_DEFAULT)
  setTabSize(TAB_SIZE_DEFAULT)
  setWordWrap(WORD_WRAP_DEFAULT)
})

describe('normalizePrefs', () => {
  it('空配置回落到默认值，包含编辑器默认项', () => {
    expect(normalizePrefs({})).toEqual(DEFAULT_PREFS)
    expect(DEFAULT_PREFS.fontSize).toBe(13)
    expect(DEFAULT_PREFS.tabSize).toBe(2)
    expect(DEFAULT_PREFS.wordWrap).toBe('off')
  })

  it('字号取整并 clamp 到 11–20，非法值回落 13', () => {
    expect(normalizePrefs({ fontSize: 13.6 }).fontSize).toBe(14)
    expect(normalizePrefs({ fontSize: 9 }).fontSize).toBe(FONT_SIZE_MIN)
    expect(normalizePrefs({ fontSize: 99 }).fontSize).toBe(FONT_SIZE_MAX)
    expect(normalizePrefs({ fontSize: Number.NaN }).fontSize).toBe(FONT_SIZE_DEFAULT)
    expect(normalizePrefs({ fontSize: '16' }).fontSize).toBe(FONT_SIZE_DEFAULT)
  })

  it('Tab 宽度只接受 2/4/8，自动换行归一为 on/off，其他偏好保持旧行为', () => {
    expect(normalizePrefs({ tabSize: 4 }).tabSize).toBe(4)
    expect(normalizePrefs({ tabSize: 8 }).tabSize).toBe(8)
    expect(normalizePrefs({ tabSize: 3 }).tabSize).toBe(2)
    expect(normalizePrefs({ wordWrap: 'on' }).wordWrap).toBe('on')
    expect(normalizePrefs({ wordWrap: 'yes' }).wordWrap).toBe('off')
    expect(normalizePrefs({ theme: 'light', zoom: 9, locale: 'en' })).toMatchObject({
      theme: 'light',
      zoom: 1.6,
      locale: 'en',
    })
  })
})

describe('持久化', () => {
  it('savePrefs 写入固定 key，loadPrefs 读取并归一化', () => {
    const storage = fakeStorage()
    savePrefs(
      { ...DEFAULT_PREFS, fontSize: 18, tabSize: 8, wordWrap: 'on' },
      storage,
    )
    expect(JSON.parse(storage.data[PREFS_STORAGE_KEY])).toMatchObject({
      fontSize: 18,
      tabSize: 8,
      wordWrap: 'on',
    })
    expect(loadPrefs(storage)).toMatchObject({
      fontSize: 18,
      tabSize: 8,
      wordWrap: 'on',
    })
  })

  it('损坏 JSON / 非对象 / 非法持久值收敛到合法范围', () => {
    expect(loadPrefs(fakeStorage({ [PREFS_STORAGE_KEY]: '{' }))).toEqual(DEFAULT_PREFS)
    expect(loadPrefs(fakeStorage({ [PREFS_STORAGE_KEY]: '42' }))).toEqual(DEFAULT_PREFS)
    const invalid = fakeStorage({
      [PREFS_STORAGE_KEY]: JSON.stringify({
        fontSize: 200,
        tabSize: 7,
        wordWrap: 'maybe',
      }),
    })
    expect(loadPrefs(invalid)).toMatchObject({
      fontSize: FONT_SIZE_MAX,
      tabSize: 2,
      wordWrap: 'off',
    })
  })

  it('无 storage 时安全降级', () => {
    expect(loadPrefs(null)).toEqual(DEFAULT_PREFS)
    expect(() => savePrefs(DEFAULT_PREFS, null)).not.toThrow()
  })
})

describe('setter 与订阅', () => {
  it('归一化、持久化并通过 subscribePrefs 通知', () => {
    const storage = fakeStorage()
    vi.stubGlobal('localStorage', storage)
    const seen: Array<{ fontSize: number; tabSize: number; wordWrap: string }> = []
    const unsubscribe = subscribePrefs(prefs => {
      seen.push({
        fontSize: prefs.fontSize,
        tabSize: prefs.tabSize,
        wordWrap: prefs.wordWrap,
      })
    })
    try {
      setFontSize(99)
      setTabSize(8)
      setWordWrap('on')

      expect(uiPrefs()).toMatchObject({
        fontSize: FONT_SIZE_MAX,
        tabSize: 8,
        wordWrap: 'on',
      })
      expect(JSON.parse(storage.data[PREFS_STORAGE_KEY])).toMatchObject({
        fontSize: FONT_SIZE_MAX,
        tabSize: 8,
        wordWrap: 'on',
      })
      expect(seen).toEqual([
        { fontSize: FONT_SIZE_MAX, tabSize: 2, wordWrap: 'off' },
        { fontSize: FONT_SIZE_MAX, tabSize: 8, wordWrap: 'off' },
        { fontSize: FONT_SIZE_MAX, tabSize: 8, wordWrap: 'on' },
      ])
    } finally {
      unsubscribe()
    }
  })
})
