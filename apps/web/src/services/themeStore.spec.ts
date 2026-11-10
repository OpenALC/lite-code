import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  AGENT_DENSITY_DEFAULT,
  AGENT_FONT_DEFAULT,
  AGENT_FONT_MAX,
  AGENT_FONT_MIN,
  AGENT_SEND_KEY_DEFAULT,
  DEFAULT_PREFS,
  FONT_SIZE_DEFAULT,
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  PREFS_STORAGE_KEY,
  TAB_SIZE_DEFAULT,
  WORD_WRAP_DEFAULT,
  applyPluginTheme,
  isPluginThemeSlug,
  loadPrefs,
  normalizePluginThemeColors,
  normalizePluginThemeJson,
  normalizePrefs,
  pluginThemeCss,
  pluginThemeOwner,
  pluginThemeSlug,
  registerPluginTheme,
  removePluginTheme,
  savePrefs,
  setAgentDensity,
  setAgentFontSize,
  setAgentSendKey,
  setFontSize,
  setTabSize,
  setTheme,
  setWordWrap,
  subscribePrefs,
  themeBaseMode,
  toggleTheme,
  uiPrefs,
  unregisterPluginTheme,
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
  setAgentSendKey(AGENT_SEND_KEY_DEFAULT)
  setAgentDensity(AGENT_DENSITY_DEFAULT)
  setAgentFontSize(AGENT_FONT_DEFAULT)
})

describe('normalizePrefs', () => {
  it('空配置回落到默认值，包含编辑器默认项', () => {
    expect(normalizePrefs({})).toEqual(DEFAULT_PREFS)
    expect(DEFAULT_PREFS.fontSize).toBe(13)
    expect(DEFAULT_PREFS.tabSize).toBe(2)
    expect(DEFAULT_PREFS.wordWrap).toBe('off')
    expect(DEFAULT_PREFS.agentSendKey).toBe('enter')
    expect(DEFAULT_PREFS.agentDensity).toBe('comfortable')
    expect(DEFAULT_PREFS.agentFontSize).toBe(14)
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

  it('Agent 偏好：非法发送键/密度回落，字号 clamp 到 12–18', () => {
    expect(normalizePrefs({ agentSendKey: 'ctrl-enter' }).agentSendKey).toBe('ctrl-enter')
    expect(normalizePrefs({ agentSendKey: 'x' }).agentSendKey).toBe('enter')
    expect(normalizePrefs({ agentDensity: 'compact' }).agentDensity).toBe('compact')
    expect(normalizePrefs({ agentDensity: 'x' }).agentDensity).toBe('comfortable')
    expect(normalizePrefs({ agentFontSize: 9 }).agentFontSize).toBe(AGENT_FONT_MIN)
    expect(normalizePrefs({ agentFontSize: 99 }).agentFontSize).toBe(AGENT_FONT_MAX)
    expect(normalizePrefs({ agentFontSize: Number.NaN }).agentFontSize).toBe(AGENT_FONT_DEFAULT)
  })
})

describe('持久化', () => {
  it('savePrefs 写入固定 key，loadPrefs 读取并归一化', () => {
    const storage = fakeStorage()
    savePrefs(
      {
        ...DEFAULT_PREFS,
        fontSize: 18,
        tabSize: 8,
        wordWrap: 'on',
        agentSendKey: 'ctrl-enter',
        agentDensity: 'compact',
        agentFontSize: 16,
      },
      storage,
    )
    expect(JSON.parse(storage.data[PREFS_STORAGE_KEY])).toMatchObject({
      fontSize: 18,
      tabSize: 8,
      wordWrap: 'on',
      agentSendKey: 'ctrl-enter',
      agentDensity: 'compact',
      agentFontSize: 16,
    })
    expect(loadPrefs(storage)).toMatchObject({
      fontSize: 18,
      tabSize: 8,
      wordWrap: 'on',
      agentSendKey: 'ctrl-enter',
      agentDensity: 'compact',
      agentFontSize: 16,
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

// ---- 插件主题（docs/EXTENSIONS.md §1）：slug、白名单、样式注入与回落 ----

class FakeStyleTag {
  textContent = ''
  removed = false
  attrs = new Map<string, string>()

  setAttribute(name: string, value: string): void {
    this.attrs.set(name, value)
  }

  remove(): void {
    this.removed = true
  }

  matches(slug: string): boolean {
    return !this.removed && this.attrs.get('data-plugin-theme') === slug
  }
}

class FakeThemeDoc {
  readonly tags: FakeStyleTag[] = []
  readonly head = {
    appendChild: (node: FakeStyleTag) => {
      this.tags.push(node)
    },
  }

  querySelector(selector: string): FakeStyleTag | null {
    const match = /^style\[data-plugin-theme="(.+)"\]$/.exec(selector)
    if (!match) return null
    return this.tags.find(tag => tag.matches(match[1])) ?? null
  }

  createElement(): FakeStyleTag {
    return new FakeStyleTag()
  }
}

describe('插件主题', () => {
  afterEach(() => {
    setTheme('dark')
    unregisterPluginTheme('plugin--demo.hello--demo-dusk')
    unregisterPluginTheme('plugin--my--plugin--x')
  })

  it('slug：label 折叠成小写线连，纯中文回落 theme', () => {
    expect(pluginThemeSlug('demo.hello', 'Demo Dusk')).toBe('plugin--demo.hello--demo-dusk')
    expect(pluginThemeSlug('demo.hello', 'My  Theme!')).toBe(
      'plugin--demo.hello--my-theme',
    )
    expect(pluginThemeSlug('demo.hello', '暗夜')).toBe('plugin--demo.hello--theme')
    expect(isPluginThemeSlug('plugin--a--b')).toBe(true)
    expect(isPluginThemeSlug('dark')).toBe(false)
  })

  it('pluginThemeOwner 还原插件 id（含 id 自身带 -- 的情形）', () => {
    expect(pluginThemeOwner('plugin--demo.hello--demo-dusk')).toBe('demo.hello')
    expect(pluginThemeOwner('plugin--my--plugin--x')).toBe('my--plugin')
    expect(pluginThemeOwner('dark')).toBeNull()
    expect(pluginThemeOwner('plugin--x')).toBeNull()
  })

  it('colors 白名单只收合法的 -- 变量名，忽略其余键与非字符串值', () => {
    expect(
      normalizePluginThemeColors({
        '--bg': ' #101014 ',
        '--fg': '#e6e9ef',
        body: 'red',
        '--evil key': 'x',
        '--empty': '   ',
        '--num': 42,
      }),
    ).toEqual({ '--bg': '#101014', '--fg': '#e6e9ef' })
    expect(normalizePluginThemeColors(null)).toEqual({})
    expect(normalizePluginThemeColors('x')).toEqual({})
  })

  it('normalizePluginThemeJson 校验形状；normalizePrefs 保留 slug', () => {
    expect(
      normalizePluginThemeJson({ name: 'Dusk', colors: { '--bg': '#000' } }),
    ).toEqual({ name: 'Dusk', colors: { '--bg': '#000' } })
    expect(normalizePluginThemeJson({ name: 'Dusk' })).toBeNull()
    expect(normalizePluginThemeJson('x')).toBeNull()
    const slug = 'plugin--demo.hello--demo-dusk'
    expect(normalizePrefs({ theme: slug }).theme).toBe(slug)
    expect(normalizePrefs({ theme: 'light' }).theme).toBe('light')
    expect(normalizePrefs({ theme: 'garbage' }).theme).toBe('dark')
  })

  it('pluginThemeCss 生成 :root[data-theme] 规则，空 colors 返回 null', () => {
    expect(pluginThemeCss('plugin--demo.hello--demo-dusk', { '--bg': '#000' })).toBe(
      ":root[data-theme='plugin--demo.hello--demo-dusk'] {\n  --bg: #000;\n}",
    )
    expect(pluginThemeCss('plugin--x--y', {})).toBeNull()
  })

  it('applyPluginTheme 注入/更新/移除 style 标签，removePluginTheme 清理', () => {
    const doc = new FakeThemeDoc()
    const slug = 'plugin--demo.hello--demo-dusk'
    expect(applyPluginTheme(slug, { '--bg': '#000' }, doc)).toBe(true)
    expect(doc.tags).toHaveLength(1)
    expect(doc.tags[0].attrs.get('data-plugin-theme')).toBe(slug)

    // 同一 slug 再次注入：更新既有标签而不是新增。
    expect(applyPluginTheme(slug, { '--bg': '#111' }, doc)).toBe(true)
    expect(doc.tags).toHaveLength(1)
    expect(doc.tags[0].textContent).toContain('#111')

    // 内容相同时不重复写。
    const before = doc.tags[0].textContent
    expect(applyPluginTheme(slug, { '--bg': '#111' }, doc)).toBe(true)
    expect(doc.tags[0].textContent).toBe(before)

    expect(removePluginTheme(slug, doc)).toBe(true)
    expect(doc.tags[0].removed).toBe(true)
    expect(removePluginTheme(slug, doc)).toBe(false)
    expect(applyPluginTheme(slug, {}, doc)).toBe(false)
  })

  it('toggleTheme 基于注册 uiTheme 的基色往返', () => {
    const slug = 'plugin--demo.hello--demo-dusk'
    registerPluginTheme(slug, 'dark')
    setTheme(slug)
    expect(uiPrefs().theme).toBe(slug)
    toggleTheme()
    expect(uiPrefs().theme).toBe('light')
    toggleTheme()
    expect(uiPrefs().theme).toBe('dark')

    // 注册亮基色插件主题：toggle 回 dark；注销后基色回落 dark。
    setTheme(slug)
    expect(themeBaseMode(slug)).toBe('dark')
    unregisterPluginTheme(slug)
    expect(themeBaseMode(slug)).toBe('dark')
  })
})
