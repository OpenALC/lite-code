// 插件语言同步（docs/EXTENSIONS.md §1）：ext_list 发现声明 → Monaco 注册
// 语言（内置 id 不重复注册，内置优先）→ 有 grammar 的经 ext_read_grammar
// 拉取，白名单（只放行 { tokenizer } 纯对象，防注入）后挂 Monarch 高亮；
// 声明消失 / 读失败时用空 tokenizer 清高亮并 console.warn 注记——Monaco
// 没有 unregister，语言与高亮都无法真正撤回（限制写进 docs/EXTENSIONS.md）。
// 扩展名检测并入 editorLanguage（内置优先）。App 启动与插件增删后调用。
import {
  extensionsList,
  extensionsReadGrammar,
  type PluginList,
} from './extensions'
import { setPluginLanguages } from './editorLanguage'

/** 本服务用到的最小 Monaco 语言接口（测试注入假对象，不引真 monaco）。 */
export interface PluginMonaco {
  languages: {
    /** 已注册语言贡献（含内置）；据此跳过内置 id 的重复注册。 */
    getLanguages(): { id: string }[]
    register(language: { id: string; extensions: string[]; aliases: string[] }): void
    setMonarchTokensProvider(languageId: string, languageDef: unknown): void
  }
}

interface DeclaredLanguage {
  id: string
  extensions: string[]
  aliases: string[]
  grammar: string | null
  pluginId: string
}

/** 默认经动态 import 拿 monaco（chunk 随首次同步加载，服务不静态依赖）。 */
let monacoLoader: () => Promise<PluginMonaco> = async () => {
  const monaco = await import('monaco-editor')
  return monaco as unknown as PluginMonaco
}

/** 替换 Monaco 加载器（测试注入假对象；嵌入方也可复用）。 */
export function setPluginMonacoLoader(loader: () => Promise<PluginMonaco>): void {
  monacoLoader = loader
}

/** 本会话注册过的插件语言 id：Monaco 无法注销，重复同步不许重复注册。 */
const registeredIds = new Set<string>()
/** 上一次同步成功挂了 Monarch 高亮的语言 id。 */
let highlightedIds = new Set<string>()

/** 测试用：清空注册/高亮记账（Monaco 侧的注册与高亮无法撤回）。 */
export function resetPluginLanguages(): void {
  registeredIds.clear()
  highlightedIds = new Set()
  setPluginLanguages([])
}

/**
 * 语法 JSON 白名单（docs/EXTENSIONS.md §1）：只放行 `tokenizer` 键且值是
 * 纯对象，其余键一律忽略（防注入其它 Monaco 语言定义字段）；形状不符返回
 * null，调用方按读失败处理。
 */
export function normalizeGrammarJson(raw: unknown): { tokenizer: Record<string, unknown> } | null {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return null
  const tokenizer = (raw as Record<string, unknown>).tokenizer
  if (typeof tokenizer !== 'object' || tokenizer === null || Array.isArray(tokenizer)) {
    return null
  }
  return { tokenizer: { ...(tokenizer as Record<string, unknown>) } }
}

/** 从插件列表收集启用插件的语言声明（error 插件没有可用清单）。 */
function declaredLanguages(list: PluginList): DeclaredLanguage[] {
  const languages: DeclaredLanguage[] = []
  for (const plugin of list.plugins) {
    if (!plugin.enabled || plugin.error) continue
    for (const language of plugin.languages) {
      languages.push({
        id: language.id,
        extensions: language.extensions,
        aliases: language.aliases,
        grammar: language.grammar,
        pluginId: plugin.id,
      })
    }
  }
  return languages
}

/**
 * 拉取最新插件列表并做语言同步；失败返回 null（视图层保持原状）。
 * App 启动与插件增删后调用。
 */
export async function syncPluginLanguages(): Promise<PluginList | null> {
  const res = await extensionsList()
  if (!res.ok) return null
  await applyPluginLanguageList(res.data)
  return res.data
}

/** 对一份插件列表做语言同步（测试与复用入口）。 */
export async function applyPluginLanguageList(list: PluginList): Promise<void> {
  const declared = declaredLanguages(list)
  // 扩展名检测并入 editorLanguage：内置优先，插件只补内置未覆盖的扩展名。
  setPluginLanguages(declared)

  let monaco: PluginMonaco
  try {
    monaco = await monacoLoader()
  } catch (error) {
    console.warn('[插件语言] Monaco 不可用，跳过语言注册：', error)
    return
  }

  // 注册：只注册本会话没注册过、且 Monaco 尚不知道的语言（内置 id 优先）。
  const known = new Set(monaco.languages.getLanguages().map(language => language.id))
  for (const language of declared) {
    if (registeredIds.has(language.id) || known.has(language.id)) continue
    monaco.languages.register({
      id: language.id,
      extensions: language.extensions,
      aliases: language.aliases,
    })
    registeredIds.add(language.id)
  }

  // 高亮：只给本会话注册的语言挂语法（不覆盖内置语言的 Monarch 定义）。
  const applied = new Set<string>()
  for (const language of declared) {
    if (!registeredIds.has(language.id) || !language.grammar) continue
    const res = await extensionsReadGrammar(language.pluginId, language.grammar)
    const grammar = res.ok ? normalizeGrammarJson(res.data.json) : null
    if (grammar) {
      monaco.languages.setMonarchTokensProvider(language.id, grammar)
      applied.add(language.id)
    } else {
      console.warn(
        `[插件语言] 语法不可用：${language.pluginId} 的 ${language.grammar}，` +
          '已清空该语言高亮',
      )
    }
  }

  // 降级：上次挂了高亮、这次没有的语言（声明消失 / 读失败 / 白名单拒绝）
  // 用空 tokenizer 清高亮；语言注册保留（Monaco 无 unregister）。
  for (const id of highlightedIds) {
    if (applied.has(id)) continue
    monaco.languages.setMonarchTokensProvider(id, { tokenizer: {} })
  }
  highlightedIds = applied
}
