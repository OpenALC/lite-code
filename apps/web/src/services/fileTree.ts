// 文件树数据逻辑：纯函数 + 不可变更新（UI.md §8：services 只暴露纯函数）。
//
// 一次一层（docs/UI.md §3：按需加载；docs/ARCHITECTURE.md §5.2：目录按层返回）：
// 目录展开才请求下一层。这里只维护已加载层与展开状态；
// 展示顺序 = 目录优先 + 名称不区分大小写——lc-core 的 listing 只保证确定性
// 顺序（fs/listing.rs），排序策略属 UI 层。
// 错误以契约 ErrorBody 原样上抛给视图层：可读文案由 i18n 提供，
// 错误码放 tooltip / 日志（UI.md §4）。

import type { DirEntryDto } from '@lc-proto'

/** 一层目录的加载状态。key = 该层目录路径（根层为 root 本身）。 */
export interface TreeLayer {
  path: string
  state: 'loading' | 'ready' | 'error'
  entries: DirEntryDto[]
  /** state = error 时携带契约错误体 */
  error?: { code: string; message: string }
}

export interface TreeState {
  /** 项目根（fs 命令的安全边界，docs/PLAN.md R3）；空串表示未打开工作区 */
  root: string
  layers: Record<string, TreeLayer>
  /** 已展开目录集合（key = 目录路径） */
  expanded: Record<string, boolean>
  /** 数据来自浏览器 dev 占位（bridge 的 fallback 标注透传），UI 需显式提示 */
  fallback: boolean
}

/** 展开后的可见行。entry 行之外还有加载中 / 错误占位行，视图按 kind 渲染。 */
export type TreeRow =
  | { kind: 'entry'; entry: DirEntryDto; depth: number; expanded: boolean }
  | { kind: 'loading'; depth: number }
  | { kind: 'error'; depth: number; error: { code: string; message: string } }

export function createTreeState(root: string, fallback: boolean): TreeState {
  return {
    root,
    layers: { [root]: { path: root, state: 'loading', entries: [] } },
    expanded: {},
    fallback,
  }
}

/** 换根：重置全部层与展开状态，根层回到 loading（视图层随后发起加载）。 */
export function reRoot(state: TreeState, root: string): TreeState {
  if (root === state.root) return state
  return createTreeState(root, state.fallback)
}

// ==== 路径与拖放规划（纯函数；Windows 盘符/UNC 路径大小写不敏感） ====

function usesWindowsCase(path: string): boolean {
  return /^[A-Za-z]:\//.test(path) || path.startsWith('//')
}

/** 统一分隔符并去掉尾部斜杠；根路径保留（`/`、`C:/`）。 */
export function normalizePath(path: string): string {
  if (path === '') return ''
  const slashed = path.replace(/\\/g, '/')
  if (slashed === '/') return '/'
  if (/^[A-Za-z]:\/?$/.test(slashed)) return `${slashed.slice(0, 2)}/`
  return slashed.replace(/\/+$/, '')
}

function pathKey(path: string): string {
  const normalized = normalizePath(path)
  return usesWindowsCase(normalized) ? normalized.toLowerCase() : normalized
}

/** child 是 parent 本身或其后代（按路径组件边界，`/a/b` 不算 `/a/bc` 的父级）。 */
export function isPathWithin(parent: string, child: string): boolean {
  if (!parent || !child) return false
  const p = pathKey(parent)
  const c = pathKey(child)
  if (p === c) return true
  return c.startsWith(p.endsWith('/') ? p : `${p}/`)
}

/** child 是 parent 的严格后代（不含相等）。 */
export function isStrictDescendant(parent: string, child: string): boolean {
  return isPathWithin(parent, child) && pathKey(parent) !== pathKey(child)
}

function samePath(a: string, b: string): boolean {
  return pathKey(a) === pathKey(b)
}

function separatorOf(path: string): '/' | '\\' {
  return path.includes('\\') ? '\\' : '/'
}

function stripTrailingSeparators(path: string): string {
  if (path === '/' || path === '\\') return ''
  if (/^[A-Za-z]:$/.test(path)) return path
  if (/^[A-Za-z]:[/\\]$/.test(path)) return path.slice(0, 2)
  return path.replace(/[/\\]+$/, '')
}

/** 父目录路径；已是根时返回自身。保留调用方使用的分隔符风格。 */
export function parentPath(path: string): string {
  if (!path) return ''
  const sep = separatorOf(path)
  const trimmed = path.replace(/[/\\]+$/, '')
  if (!trimmed || /^[A-Za-z]:$/.test(trimmed)) return path
  const normalized = trimmed.replace(/\\/g, '/')
  const index = normalized.lastIndexOf('/')
  if (index < 0) return path
  if (index === 0) return '/'
  const parent = normalized.slice(0, index)
  return /^[A-Za-z]:$/.test(parent) ? `${parent}${sep}` : parent.replace(/\//g, sep)
}

/** 文件名（末段）；根路径返回空串。 */
export function basenamePath(path: string): string {
  const trimmed = path.replace(/[/\\]+$/, '')
  if (!trimmed) return ''
  return trimmed.split(/[/\\]/).pop() ?? ''
}

/** 拼接目录与条目名，保留目录的分隔符风格。 */
export function joinPath(dir: string, name: string): string {
  if (!dir) return name
  const sep = separatorOf(dir)
  const base = stripTrailingSeparators(dir)
  const leaf = name.replace(/^[/\\]+/, '')
  return base ? `${base}${sep}${leaf}` : `${sep}${leaf}`
}

/** 把 path 的 from 前缀替换成 to（目录移动会命中全部后代）；不匹配返回 null。 */
export function replacePathPrefix(path: string, from: string, to: string): string | null {
  if (!isPathWithin(from, path)) return null
  const sep = separatorOf(to)
  const fromNormalized = normalizePath(from).replace(/\/+$/, '')
  const suffix = normalizePath(path).slice(fromNormalized.length)
  const rest = suffix.replace(/^[/\\]+/, '').replace(/[/\\]/g, sep)
  if (!rest) return to
  const base = stripTrailingSeparators(to)
  return base ? `${base}${sep}${rest}` : `${sep}${rest}`
}

/**
 * 拖放目标解析：拖到文件夹 → 该目录；拖到文件 → 其父目录。
 * 符号链接按文件处理，避免把链接目标当目录写入。
 */
export function dropTargetDir(target: Pick<DirEntryDto, 'path' | 'kind'>): string {
  return target.kind === 'dir' ? target.path : parentPath(target.path)
}

export type MoveFailure = 'outsideRoot' | 'sameParent' | 'selfNesting'

export type MovePlan =
  | { ok: true; from: string; to: string; destDir: string }
  | { ok: false; reason: MoveFailure }

/**
 * 拖拽移动规划：校验根内、目标目录、自嵌套与原地移动。
 * 同名覆盖由后端拒绝（前端不缓存完整目录树，不能可靠判断目标是否存在）。
 */
export function planMove(
  root: string,
  source: Pick<DirEntryDto, 'path' | 'kind'>,
  target: Pick<DirEntryDto, 'path' | 'kind'>,
): MovePlan {
  if (
    !root ||
    !isPathWithin(root, source.path) ||
    !isPathWithin(root, target.path)
  ) {
    return { ok: false, reason: 'outsideRoot' }
  }
  const destDir = dropTargetDir(target)
  if (!isPathWithin(root, destDir)) return { ok: false, reason: 'outsideRoot' }

  const to = joinPath(destDir, basenamePath(source.path))
  if (samePath(to, source.path)) return { ok: false, reason: 'sameParent' }
  if (source.kind === 'dir' && isPathWithin(source.path, to)) {
    return { ok: false, reason: 'selfNesting' }
  }
  return { ok: true, from: source.path, to, destDir }
}

/** 设置/清除目录展开状态；值不变时返回原状态。 */
export function setExpanded(state: TreeState, path: string, expanded: boolean): TreeState {
  if ((state.expanded[path] === true) === expanded) return state
  return { ...state, expanded: { ...state.expanded, [path]: expanded } }
}

/** 删除/移动后丢弃该路径及其后代的层缓存与展开状态。 */
export function forgetPath(state: TreeState, path: string): TreeState {
  if (!path) return state
  const layers: Record<string, TreeLayer> = {}
  for (const [key, layer] of Object.entries(state.layers)) {
    if (!isPathWithin(path, key)) layers[key] = layer
  }
  const expanded: Record<string, boolean> = {}
  for (const [key, value] of Object.entries(state.expanded)) {
    if (!isPathWithin(path, key)) expanded[key] = value
  }
  return { ...state, layers, expanded }
}

export function layerLoaded(state: TreeState, path: string, entries: DirEntryDto[]): TreeState {
  return {
    ...state,
    layers: { ...state.layers, [path]: { path, state: 'ready', entries } },
  }
}

export function layerFailed(
  state: TreeState,
  path: string,
  error: { code: string; message: string },
): TreeState {
  return {
    ...state,
    layers: { ...state.layers, [path]: { path, state: 'error', entries: [], error } },
  }
}

/** 切换展开状态。是否需要发起加载由视图层根据 layer.state 决定。 */
export function toggleDir(state: TreeState, path: string): TreeState {
  return { ...state, expanded: { ...state.expanded, [path]: state.expanded[path] !== true } }
}

/** 展示排序：目录优先，再按名称不区分大小写（确定性，避免每次渲染抖动）。 */
export function sortEntries(entries: DirEntryDto[]): DirEntryDto[] {
  return [...entries].sort((a, b) => {
    const da = a.kind === 'dir' ? 0 : 1
    const db = b.kind === 'dir' ? 0 : 1
    if (da !== db) return da - db
    return a.name.toLowerCase() < b.name.toLowerCase()
      ? -1
      : a.name.toLowerCase() > b.name.toLowerCase()
        ? 1
        : 0
  })
}

/** 当前应渲染的行（深度优先，展开层内联下钻）。 */
export function visibleRows(state: TreeState): TreeRow[] {
  const out: TreeRow[] = []
  rowsFor(state, state.root, 0, out)
  return out
}

function rowsFor(state: TreeState, path: string, depth: number, out: TreeRow[]): void {
  const layer = state.layers[path]
  if (!layer) return
  if (layer.state === 'loading') {
    out.push({ kind: 'loading', depth })
    return
  }
  if (layer.state === 'error') {
    out.push({ kind: 'error', depth, error: layer.error ?? { code: 'io', message: '' } })
    return
  }
  for (const entry of sortEntries(layer.entries)) {
    const expanded = entry.kind === 'dir' && state.expanded[entry.path] === true
    out.push({ kind: 'entry', entry, depth, expanded })
    if (expanded) rowsFor(state, entry.path, depth + 1, out)
  }
}
