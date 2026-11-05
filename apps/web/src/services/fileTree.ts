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
