import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { BridgeResult } from './bridge'
import {
  extensionsList,
  extensionsReadTheme,
  type ExtThemeResult,
  type PluginInfo,
  type PluginList,
} from './extensions'
import {
  applyPluginThemeList,
  pluginThemeOptions,
  subscribePluginThemes,
  syncPluginThemes,
  type PluginThemeOption,
} from './pluginThemes'
import { pluginThemeSlug, setTheme, uiPrefs } from './themeStore'

// vi.mock 提升：用可变状态驱动 mock 的 list / 主题文件内容。
const extState = vi.hoisted(() => ({
  list: null as PluginList | null,
  themes: new Map<string, unknown>(),
}))

function internalError(message: string) {
  return { code: 'internal' as const, message, detail: null }
}

vi.mock('./extensions', async importOriginal => {
  const actual = await importOriginal<typeof import('./extensions')>()
  return {
    ...actual,
    extensionsList: vi.fn(async (): Promise<BridgeResult<PluginList>> => {
      if (!extState.list) {
        return { ok: false, error: internalError('列表不可用'), fallback: false }
      }
      return { ok: true, data: extState.list, fallback: false }
    }),
    extensionsReadTheme: vi.fn(
      async (pluginId: string, path: string): Promise<BridgeResult<ExtThemeResult>> => {
        const json = extState.themes.get(`${pluginId}>${path}`)
        if (json === undefined) {
          return { ok: false, error: internalError('主题不可用'), fallback: false }
        }
        return { ok: true, data: { json }, fallback: false }
      },
    ),
  }
})

class FakeElement {
  textContent = ''
  removed = false
  readonly attrs = new Map<string, string>()
  readonly dataset: Record<string, string> = {}
  readonly style: Record<string, string> = {}

  setAttribute(name: string, value: string): void {
    this.attrs.set(name, value)
  }

  remove(): void {
    this.removed = true
  }
}

class FakeDocument {
  readonly documentElement = new FakeElement()
  readonly tags: FakeElement[] = []
  readonly head = {
    appendChild: (node: FakeElement) => {
      this.tags.push(node)
    },
  }

  querySelector(selector: string): FakeElement | null {
    const match = /^style\[data-plugin-theme="(.+)"\]$/.exec(selector)
    if (!match) return null
    return (
      this.tags.find(tag => !tag.removed && tag.attrs.get('data-plugin-theme') === match[1]) ??
      null
    )
  }

  createElement(): FakeElement {
    return new FakeElement()
  }
}

let fakeDoc: FakeDocument

function plugin(partial: Partial<PluginInfo> & { id: string }): PluginInfo {
  return {
    name: partial.id,
    version: '0.1.0',
    description: '',
    runtime: 'declarative',
    enabled: true,
    path: `~/.lite-code/extensions/${partial.id}`,
    error: null,
    commands: [],
    mcpServers: [],
    languageServers: [],
    themes: [],
    languages: [],
    views: [],
    ...partial,
  }
}

function themedList(enabled: boolean): PluginList {
  return {
    plugins: [
      plugin({
        id: 'demo.hello',
        name: 'Hello Demo',
        enabled,
        themes: [{ label: 'Demo Dusk', path: './themes/dusk.json', uiTheme: 'dark' }],
      }),
    ],
    dir: '~/.lite-code/extensions',
    nodeAvailable: true,
  }
}

beforeEach(() => {
  fakeDoc = new FakeDocument()
  vi.stubGlobal('document', fakeDoc)
  setTheme('dark')
})

afterEach(() => {
  vi.unstubAllGlobals()
  extState.list = null
  extState.themes.clear()
  setTheme('dark')
})

describe('pluginThemes 同步', () => {
  it('读取启用插件的主题并注入 style；colors 走白名单；通知订阅者', async () => {
    extState.list = themedList(true)
    extState.themes.set('demo.hello>./themes/dusk.json', {
      name: 'Demo Dusk',
      colors: { '--bg': '#101014', body: 'red' },
    })
    const seen: PluginThemeOption[][] = []
    const unsub = subscribePluginThemes(next => seen.push(next))
    try {
      const list = await syncPluginThemes()
      expect(list?.plugins).toHaveLength(1)
      expect(vi.mocked(extensionsReadTheme)).toHaveBeenCalledWith(
        'demo.hello',
        './themes/dusk.json',
      )

      const slug = pluginThemeSlug('demo.hello', 'Demo Dusk')
      const tag = fakeDoc.tags.find(
        item => item.attrs.get('data-plugin-theme') === slug,
      )
      expect(tag).toBeDefined()
      expect(tag?.textContent).toContain(`:root[data-theme='${slug}']`)
      expect(tag?.textContent).toContain('--bg: #101014')
      expect(tag?.textContent).not.toContain('body')

      expect(pluginThemeOptions().map(option => option.slug)).toEqual([slug])
      expect(seen.at(-1)?.map(option => option.pluginName)).toEqual(['Hello Demo'])
      expect(vi.mocked(extensionsList)).toHaveBeenCalled()
    } finally {
      unsub()
    }
  })

  it('选中的插件主题在插件禁用后回落 dark 并清理样式', async () => {
    const slug = pluginThemeSlug('demo.hello', 'Demo Dusk')
    extState.list = themedList(true)
    extState.themes.set('demo.hello>./themes/dusk.json', {
      name: 'Demo Dusk',
      colors: { '--bg': '#101014' },
    })
    await applyPluginThemeList(themedList(true))
    setTheme(slug)
    expect(uiPrefs().theme).toBe(slug)
    expect(fakeDoc.documentElement.dataset.theme).toBe(slug)

    // 插件被禁用：声明消失 → 样式清理 + 持久化主题回落 dark。
    extState.list = themedList(false)
    await applyPluginThemeList(extState.list)
    expect(uiPrefs().theme).toBe('dark')
    expect(fakeDoc.documentElement.dataset.theme).toBe('dark')
    const tag = fakeDoc.tags.find(item => item.attrs.get('data-plugin-theme') === slug)
    expect(tag?.removed).toBe(true)
    expect(pluginThemeOptions()).toEqual([])
  })

  it('读取失败：样式不注入，选中该主题则回落 dark，选项仍保留', async () => {
    const slug = pluginThemeSlug('demo.hello', 'Demo Dusk')
    await applyPluginThemeList(themedList(true))
    setTheme(slug)

    // 主题文件丢失：声明还在（选项保留），但样式不可用。
    await applyPluginThemeList(themedList(true))
    expect(uiPrefs().theme).toBe('dark')
    expect(
      fakeDoc.tags.some(item => item.attrs.get('data-plugin-theme') === slug),
    ).toBe(false)
    expect(pluginThemeOptions()).toHaveLength(1)
  })

  it('error 插件的主题不进入选项', async () => {
    const broken = themedList(true)
    broken.plugins[0].error = '清单无效'
    await applyPluginThemeList(broken)
    expect(pluginThemeOptions()).toEqual([])
  })

  it('ext_list 失败返回 null，选项保持不变', async () => {
    extState.list = themedList(true)
    extState.themes.set('demo.hello>./themes/dusk.json', {
      name: 'Demo Dusk',
      colors: { '--bg': '#101014' },
    })
    await syncPluginThemes()
    expect(pluginThemeOptions()).toHaveLength(1)

    extState.list = null
    const result = await syncPluginThemes()
    expect(result).toBeNull()
    expect(pluginThemeOptions()).toHaveLength(1)
  })
})
