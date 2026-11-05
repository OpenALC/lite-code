// 文件路径 → Monaco language id（纯函数，UI.md §8 services 层）。
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

/** 无扩展名 / 未知扩展名回退 plaintext；大小写不敏感。 */
export function languageForPath(path: string | null): string {
  if (!path) return 'plaintext'
  const name = path.replace(/\\/g, '/').split('/').pop() ?? ''
  const dot = name.lastIndexOf('.')
  if (dot <= 0 || dot === name.length - 1) return 'plaintext'
  return BY_EXTENSION[name.slice(dot + 1).toLowerCase()] ?? 'plaintext'
}
