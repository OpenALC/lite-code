// 文件路径 → Monaco language id（纯函数，UI.md §8 services 层）。
// 内置映射优先：插件声明的扩展名只在未被内置表覆盖时生效
// （docs/EXTENSIONS.md §1），插件映射由 setPluginLanguages 全量替换。
const BY_EXTENSION: Record<string, string> = {
  rs: 'rust',
  ts: 'typescript',
  tsx: 'typescript',
  js: 'javascript',
  jsx: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  json: 'json',
  md: 'markdown',
  css: 'css',
  scss: 'scss',
  less: 'less',
  html: 'html',
  htm: 'html',
  vue: 'html',
  yml: 'yaml',
  yaml: 'yaml',
  toml: 'ini',
  py: 'python',
  go: 'go',
  java: 'java',
  c: 'c',
  h: 'c',
  cpp: 'cpp',
  hpp: 'cpp',
  sql: 'sql',
  sh: 'shell',
  bash: 'shell',
  ps1: 'powershell',
  xml: 'xml',
}

/** 插件语言扩展名映射（不含点、小写 → language id），全量替换。 */
let pluginByExtension: Record<string, string> = {}

/**
 * 全量替换插件语言的扩展名映射（docs/EXTENSIONS.md §1）。扩展名先做点/
 * 大小写归一；内置表已覆盖的扩展名一律忽略（内置优先）；插件之间先声明
 * 者胜出（Rust 侧校验已保证插件间扩展名不冲突，这里是兜底）。
 */
export function setPluginLanguages(
  languages: readonly { id: string; extensions: readonly string[] }[],
): void {
  const next: Record<string, string> = {}
  for (const language of languages) {
    const id = language.id.trim()
    if (!id) continue
    for (const raw of language.extensions) {
      const extension = raw.trim().replace(/^\./, '').toLowerCase()
      if (!extension || extension in BY_EXTENSION || extension in next) continue
      next[extension] = id
    }
  }
  pluginByExtension = next
}

/** 无扩展名 / 未知扩展名回退 plaintext；大小写不敏感；内置优先于插件。 */
export function languageForPath(path: string | null): string {
  if (!path) return 'plaintext'
  const name = path.replace(/\\/g, '/').split('/').pop() ?? ''
  const dot = name.lastIndexOf('.')
  if (dot <= 0 || dot === name.length - 1) return 'plaintext'
  const extension = name.slice(dot + 1).toLowerCase()
  return BY_EXTENSION[extension] ?? pluginByExtension[extension] ?? 'plaintext'
}
