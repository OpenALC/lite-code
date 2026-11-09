// 工作区文件索引：为 composer 的 @ 引用提供按需、限界的文件列表。
// 只读目录、不读取文件内容；node_modules 等大目录直接跳过。
import { readDir } from './bridge'

const MAX_FILES = 2_000
const MAX_DEPTH = 6
const SKIP_DIRS = new Set([
  '.git',
  '.zcode',
  '.cache',
  '.next',
  '.nuxt',
  'node_modules',
  'target',
  'dist',
  'build',
  'coverage',
  'vendor',
  '.venv',
  'venv',
  '__pycache__',
])

export interface FileIndex {
  root: string
  files: string[]
  truncated: boolean
}

const cache = new Map<string, FileIndex>()

function normalize(path: string): string {
  return path.replace(/\\/g, '/')
}

export function isSkippedDir(name: string): boolean {
  if (SKIP_DIRS.has(name)) return true
  // 跳过 .claude / .playwright-mcp 等工具目录；保留 .github。
  return name.startsWith('.') && name !== '.github'
}

export function relativePath(root: string, path: string): string {
  const base = normalize(root).replace(/\/+$/, '')
  const full = normalize(path)
  if (!base) return full
  if (full === base) return ''
  return full.startsWith(`${base}/`) ? full.slice(base.length + 1) : full
}

export function searchFiles(index: FileIndex, query: string, limit = 8): string[] {
  const term = query.trim().toLocaleLowerCase()
  if (!term) return index.files.slice(0, limit)
  const scored: Array<{ file: string; rank: number }> = []
  for (const file of index.files) {
    const lower = file.toLocaleLowerCase()
    const base = lower.split('/').pop() ?? ''
    const baseIndex = base.indexOf(term)
    const pathIndex = lower.indexOf(term)
    if (baseIndex < 0 && pathIndex < 0) continue
    scored.push({
      file,
      rank: baseIndex === 0 ? 0 : baseIndex > 0 ? 1 : 2,
    })
  }
  scored.sort(
    (a, b) => a.rank - b.rank || a.file.length - b.file.length || a.file.localeCompare(b.file),
  )
  return scored.slice(0, limit).map(item => item.file)
}

async function walk(
  root: string,
  path: string,
  depth: number,
  files: string[],
  state: { truncated: boolean },
): Promise<void> {
  if (depth > MAX_DEPTH || files.length >= MAX_FILES) {
    if (files.length >= MAX_FILES) state.truncated = true
    return
  }
  const res = await readDir(root, path)
  if (!res.ok) return
  for (const entry of res.data.entries) {
    if (files.length >= MAX_FILES) {
      state.truncated = true
      return
    }
    if (entry.kind === 'dir') {
      if (isSkippedDir(entry.name)) continue
      await walk(root, entry.path, depth + 1, files, state)
    } else if (entry.kind === 'file') {
      const relative = relativePath(root, entry.path)
      if (relative) files.push(relative)
    }
  }
}

export async function buildFileIndex(root: string): Promise<FileIndex> {
  if (!root) return { root: '', files: [], truncated: false }
  const cached = cache.get(root)
  if (cached) return cached
  const files: string[] = []
  const state = { truncated: false }
  await walk(root, root, 0, files, state)
  files.sort((a, b) => a.localeCompare(b))
  const index = { root, files, truncated: state.truncated }
  cache.set(root, index)
  return index
}

export function invalidateFileIndex(root?: string): void {
  if (root) cache.delete(root)
  else cache.clear()
}
