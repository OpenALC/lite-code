import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { languageForPath } from './editorLanguage'
import {
  applyPluginLanguageList,
  normalizeGrammarJson,
  resetPluginLanguages,
  setPluginMonacoLoader,
  syncPluginLanguages,
  type PluginMonaco,
} from './pluginLanguages'
import {
  extensionsList,
  extensionsReadGrammar,
  type ExtGrammarResult,
  type PluginInfo,
  type PluginList,
} from './extensions'

// vi.mock 提升：用可变状态驱动 mock 的 list / 语法文件内容。
const extState = vi.hoisted(() => ({
  list: null as PluginList | null,
  grammars: new Map<string, unknown>(),
}))

function internalError(message: string) {
  return { code: 'internal' as const, message, detail: null }
}

type BridgeResult<T> =
  | { ok: true; data: T; fallback: boolean }
  | { ok: false; error: ReturnType<typeof internalError>; fallback: boolean }
type BridgeResultList = BridgeResult<PluginList>

vi.mock('./extensions', async importOriginal => {
  const actual = await importOriginal<typeof import('./extensions')>()
  return {
    ...actual,
    extensionsList: vi.fn(async (): Promise<BridgeResultList> => {
      if (!extState.list) {
        return { ok: false, error: internalError('列表不可用'), fallback: false }
      }
      return { ok: true, data: extState.list, fallback: false }
    }),
    extensionsReadGrammar: vi.fn(
      async (pluginId: string, path: string): Promise<BridgeResult<ExtGrammarResult>> => {
        const json = extState.grammars.get(`${pluginId}>${path}`)
        if (json === undefined) {
          return { ok: false, error: internalError('语法不可用'), fallback: false }
        }
        return { ok: true, data: { json }, fallback: false }
      },
    ),
  }
})

class FakeMonaco implements PluginMonaco {
  readonly registered: { id: string; extensions: string[]; aliases: string[] }[] = []
  readonly monarch: { id: string; def: unknown }[] = []
  /**  Monaco 已知语言（模拟内置贡献）。 */
  knownIds: string[] = []

  readonly languages = {
    getLanguages: (): { id: string }[] => this.knownIds.map(id => ({ id })),
    register: (language: { id: string; extensions: string[]; aliases: string[] }): void => {
      this.registered.push(language)
    },
    setMonarchTokensProvider: (languageId: string, languageDef: unknown): void => {
      this.monarch.push({ id: languageId, def: languageDef })
    },
  }

  lastMonarch(id: string): unknown {
    return [...this.monarch].reverse().find(entry => entry.id === id)?.def
  }
}

let fakeMonaco: FakeMonaco
let warnSpy: ReturnType<typeof vi.spyOn>

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
    ...partial,
  }
}

function languageList(enabled: boolean): PluginList {
  return {
    plugins: [
      plugin({
        id: 'demo.lang',
        name: 'Lang Demo',
        enabled,
        languages: [
          {
            id: 'demo-lang',
            extensions: ['.demo'],
            aliases: ['Demo'],
            grammar: './grammars/demo.json',
          },
        ],
      }),
    ],
    dir: '~/.lite-code/extensions',
    nodeAvailable: true,
  }
}

beforeEach(() => {
  fakeMonaco = new FakeMonaco()
  setPluginMonacoLoader(async () => fakeMonaco)
  warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {})
  resetPluginLanguages()
})

afterEach(() => {
  vi.unstubAllGlobals()
  extState.list = null
  extState.grammars.clear()
  warnSpy.mockRestore()
  resetPluginLanguages()
})

describe('pluginLanguages 同步', () => {
  it('注册语言并挂白名单过滤后的 Monarch 语法；扩展名检测并入', async () => {
    extState.list = languageList(true)
    extState.grammars.set('demo.lang>./grammars/demo.json', {
      tokenizer: { root: [['\\w+', 'keyword']] },
      ignoreCase: true, // 白名单外字段一律忽略（防注入）
    })

    const list = await syncPluginLanguages()
    expect(list?.plugins).toHaveLength(1)
    expect(vi.mocked(extensionsReadGrammar)).toHaveBeenCalledWith(
      'demo.lang',
      './grammars/demo.json',
    )
    expect(fakeMonaco.registered).toEqual([
      { id: 'demo-lang', extensions: ['.demo'], aliases: ['Demo'] },
    ])
    expect(fakeMonaco.lastMonarch('demo-lang')).toEqual({
      tokenizer: { root: [['\\w+', 'keyword']] },
    })
    // 扩展名映射并入检测（editorLanguage 内置优先）。
    expect(languageForPath('D:/p/a.demo')).toBe('demo-lang')
    expect(vi.mocked(extensionsList)).toHaveBeenCalled()
  })

  it('幂等：重复同步不重复注册、不重复拉取语法', async () => {
    extState.list = languageList(true)
    extState.grammars.set('demo.lang>./grammars/demo.json', {
      tokenizer: { root: [] },
    })
    await applyPluginLanguageList(extState.list)
    await applyPluginLanguageList(extState.list)
    expect(fakeMonaco.registered).toHaveLength(1)
    expect(vi.mocked(extensionsReadGrammar)).toHaveBeenCalledTimes(2)
    expect(fakeMonaco.monarch.filter(entry => entry.id === 'demo-lang')).toHaveLength(2)
  })

  it('白名单拒绝形状不符的语法：不挂高亮、注记 warn、语言仍注册', async () => {
    extState.list = languageList(true)
    extState.grammars.set('demo.lang>./grammars/demo.json', { root: [] })
    await applyPluginLanguageList(extState.list)
    expect(fakeMonaco.monarch).toHaveLength(0)
    expect(fakeMonaco.registered.map(language => language.id)).toEqual(['demo-lang'])
    expect(warnSpy).toHaveBeenCalled()
  })

  it('内置语言 id 不重复注册、不覆盖内置高亮', async () => {
    fakeMonaco.knownIds = ['typescript']
    extState.list = {
      plugins: [
        plugin({
          id: 'demo.lang',
          languages: [
            {
              id: 'typescript',
              extensions: ['.tst'],
              aliases: [],
              grammar: './grammars/ts.json',
            },
          ],
        }),
      ],
      dir: '~/.lite-code/extensions',
      nodeAvailable: true,
    }
    extState.grammars.set('demo.lang>./grammars/ts.json', { tokenizer: { root: [] } })

    await applyPluginLanguageList(extState.list)
    expect(fakeMonaco.registered).toEqual([])
    expect(fakeMonaco.monarch).toEqual([])
    // 扩展名检测与 Monaco 注册是两回事：.tst 未被内置表覆盖，仍并入映射。
    expect(languageForPath('D:/p/a.tst')).toBe('typescript')
    // 内置表覆盖的扩展名不受影响。
    expect(languageForPath('D:/p/a.ts')).toBe('typescript')
  })

  it('声明消失：空 tokenizer 清高亮（monaco 无 unregister，注册保留）', async () => {
    extState.list = languageList(true)
    extState.grammars.set('demo.lang>./grammars/demo.json', {
      tokenizer: { root: [] },
    })
    await applyPluginLanguageList(extState.list)
    expect(fakeMonaco.lastMonarch('demo-lang')).toEqual({ tokenizer: { root: [] } })

    // 插件被禁用：声明消失 → 空 tokenizer 清高亮 + 扩展名映射撤掉。
    extState.list = languageList(false)
    await applyPluginLanguageList(extState.list)
    expect(fakeMonaco.lastMonarch('demo-lang')).toEqual({ tokenizer: {} })
    expect(languageForPath('D:/p/a.demo')).toBe('plaintext')
    expect(warnSpy).not.toHaveBeenCalled()
  })

  it('语法读失败：已挂高亮的语言清高空 tokenizer 并注记', async () => {
    extState.list = languageList(true)
    extState.grammars.set('demo.lang>./grammars/demo.json', {
      tokenizer: { root: [] },
    })
    await applyPluginLanguageList(extState.list)
    expect(fakeMonaco.monarch).toHaveLength(1)

    // 语法文件丢失：读失败 → 清高亮 + warn；语言注册与声明保留。
    extState.grammars.clear()
    await applyPluginLanguageList(extState.list)
    expect(fakeMonaco.lastMonarch('demo-lang')).toEqual({ tokenizer: {} })
    expect(fakeMonaco.registered.map(language => language.id)).toEqual(['demo-lang'])
    expect(languageForPath('D:/p/a.demo')).toBe('demo-lang')
    expect(warnSpy).toHaveBeenCalled()
  })

  it('error 插件的语言不注册；ext_list 失败返回 null', async () => {
    const broken = languageList(true)
    broken.plugins[0].error = '清单无效'
    await applyPluginLanguageList(broken)
    expect(fakeMonaco.registered).toEqual([])

    extState.list = languageList(true)
    const ok = await syncPluginLanguages()
    expect(ok?.plugins).toHaveLength(1)

    extState.list = null
    const failed = await syncPluginLanguages()
    expect(failed).toBeNull()
  })
})

describe('normalizeGrammarJson 白名单', () => {
  it('只放行 tokenizer 纯对象，其余键忽略', () => {
    expect(normalizeGrammarJson({ tokenizer: { root: [] }, other: 1 })).toEqual({
      tokenizer: { root: [] },
    })
  })

  it('形状不符返回 null', () => {
    expect(normalizeGrammarJson(null)).toBeNull()
    expect(normalizeGrammarJson('x')).toBeNull()
    expect(normalizeGrammarJson([{ tokenizer: {} }])).toBeNull()
    expect(normalizeGrammarJson({})).toBeNull()
    expect(normalizeGrammarJson({ tokenizer: null })).toBeNull()
    expect(normalizeGrammarJson({ tokenizer: [] })).toBeNull()
    expect(normalizeGrammarJson({ tokenizer: 'x' })).toBeNull()
  })
})
