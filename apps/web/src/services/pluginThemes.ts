// 插件主题同步（docs/EXTENSIONS.md §1）：ext_list 发现声明 → ext_read_theme
// 拉取 JSON → themeStore 注入 <style data-plugin-theme>；插件禁用/删除/读失败
// 时清掉样式，持久化的主题失效则回落 dark。视图层经 subscribePluginThemes 订阅。
import {
  extensionsList,
  extensionsReadTheme,
  type PluginList,
  type PluginUiTheme,
} from './extensions'
import {
  applyPluginTheme,
  isPluginThemeSlug,
  normalizePluginThemeJson,
  pluginThemeSlug,
  registerPluginTheme,
  removePluginTheme,
  setTheme,
  uiPrefs,
  unregisterPluginTheme,
} from './themeStore'

export interface PluginThemeOption {
  /** 即持久化进 prefs.theme 的 slug。 */
  slug: string
  label: string
  pluginId: string
  pluginName: string
  uiTheme: PluginUiTheme
  /** 清单声明的相对路径（读取用）。 */
  path: string
}

type Listener = (options: PluginThemeOption[]) => void
const listeners = new Set<Listener>()
let currentOptions: PluginThemeOption[] = []
/** 已成功注入样式的 slug（样式标签存在即视为可用）。 */
const appliedSlugs = new Set<string>()

export function pluginThemeOptions(): PluginThemeOption[] {
  return currentOptions
}

export function subscribePluginThemes(listener: Listener): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

function emit(): void {
  for (const listener of listeners) listener(currentOptions)
}

/** 从插件列表收集启用插件的主题声明（error 插件没有可用清单）。 */
function declaredOptions(list: PluginList): PluginThemeOption[] {
  const options: PluginThemeOption[] = []
  for (const plugin of list.plugins) {
    if (!plugin.enabled || plugin.error) continue
    for (const theme of plugin.themes) {
      options.push({
        slug: pluginThemeSlug(plugin.id, theme.label),
        label: theme.label,
        pluginId: plugin.id,
        pluginName: plugin.name,
        uiTheme: theme.uiTheme,
        path: theme.path,
      })
    }
  }
  return options
}

/**
 * 拉取最新插件列表并做主题同步；失败返回 null（视图层保持原状）。
 * App 启动与插件增删后调用。
 */
export async function syncPluginThemes(): Promise<PluginList | null> {
  const res = await extensionsList()
  if (!res.ok) return null
  await applyPluginThemeList(res.data)
  return res.data
}

/** 对一份插件列表做主题同步（测试与复用入口）。 */
export async function applyPluginThemeList(list: PluginList): Promise<void> {
  const options = declaredOptions(list)
  currentOptions = options
  emit()

  // 逐个读取声明的主题 JSON 并注入；读失败（文件缺失/超帽/已禁用）跳过。
  const available = new Set<string>()
  for (const option of options) {
    const res = await extensionsReadTheme(option.pluginId, option.path)
    if (!res.ok) continue
    const theme = normalizePluginThemeJson(res.data.json)
    if (!theme) continue
    registerPluginTheme(option.slug, option.uiTheme)
    if (applyPluginTheme(option.slug, theme.colors)) available.add(option.slug)
  }

  // 清理：声明消失（插件禁用/删除/改清单）或本次读取失败的注入样式。
  for (const slug of appliedSlugs) {
    if (available.has(slug)) continue
    removePluginTheme(slug)
    unregisterPluginTheme(slug)
  }
  appliedSlugs.clear()
  for (const slug of available) appliedSlugs.add(slug)

  // 回落：持久化的插件主题已不可用 → 回 dark（样式与注册已同步清掉）。
  const theme = uiPrefs().theme
  if (isPluginThemeSlug(theme) && !appliedSlugs.has(theme)) {
    setTheme('dark')
  }
}
