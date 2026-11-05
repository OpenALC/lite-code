import { describe, expect, it } from 'vitest'

import type { DirEntryDto } from './contract'
import {
  createTreeState,
  layerFailed,
  layerLoaded,
  reRoot,
  sortEntries,
  toggleDir,
  visibleRows,
} from './fileTree'

function dir(path: string, name = path): DirEntryDto {
  return { name, path, kind: 'dir', size: null }
}

function file(path: string, name = path, size = 1): DirEntryDto {
  return { name, path, kind: 'file', size }
}

describe('sortEntries（展示排序属 UI 层，lc-core 只保证确定性）', () => {
  it('目录优先，名称不区分大小写', () => {
    const sorted = sortEntries([
      file('b.txt', 'b.txt'),
      dir('Apple'),
      file('Aardvark'),
      dir('zoo'),
    ])
    expect(sorted.map(e => e.name)).toEqual(['Apple', 'zoo', 'Aardvark', 'b.txt'])
  })
})

describe('一次一层（docs/UI.md §3）', () => {
  it('初始根层为 loading，可见行是 loading 占位', () => {
    const state = createTreeState('D:/proj', false)
    expect(state.layers['D:/proj'].state).toBe('loading')
    expect(visibleRows(state)).toEqual([{ kind: 'loading', depth: 0 }])
  })

  it('根层就绪后按条目渲染，深度 0', () => {
    let state = createTreeState('D:/proj', false)
    state = layerLoaded(state, 'D:/proj', [file('D:/proj/a.rs'), dir('D:/proj/sub')])
    const rows = visibleRows(state)
    expect(rows).toEqual([
      { kind: 'entry', entry: dir('D:/proj/sub'), depth: 0, expanded: false },
      { kind: 'entry', entry: file('D:/proj/a.rs'), depth: 0, expanded: false },
    ])
  })

  it('展开目录但下一层未加载：只标记 expanded，不虚构子行', () => {
    let state = createTreeState('D:/proj', false)
    state = layerLoaded(state, 'D:/proj', [dir('D:/proj/sub')])
    state = toggleDir(state, 'D:/proj/sub')
    const rows = visibleRows(state)
    expect(rows).toEqual([
      { kind: 'entry', entry: dir('D:/proj/sub'), depth: 0, expanded: true },
    ])
  })

  it('展开且下一层加载中：显示子层 loading 行（深度 +1）', () => {
    let state = createTreeState('D:/proj', false)
    state = layerLoaded(state, 'D:/proj', [dir('D:/proj/sub')])
    state = toggleDir(state, 'D:/proj/sub')
    // 视图层在展开后立刻发起加载：层回到 loading 态
    state = { ...state, layers: { ...state.layers, 'D:/proj/sub': { path: 'D:/proj/sub', state: 'loading', entries: [] } } }
    expect(visibleRows(state)).toEqual([
      { kind: 'entry', entry: dir('D:/proj/sub'), depth: 0, expanded: true },
      { kind: 'loading', depth: 1 },
    ])
  })

  it('展开且下一层就绪：子行内联下钻', () => {
    let state = createTreeState('D:/proj', false)
    state = layerLoaded(state, 'D:/proj', [dir('D:/proj/sub')])
    state = toggleDir(state, 'D:/proj/sub')
    state = layerLoaded(state, 'D:/proj/sub', [file('D:/proj/sub/deep.rs')])
    const rows = visibleRows(state)
    expect(rows).toEqual([
      { kind: 'entry', entry: dir('D:/proj/sub'), depth: 0, expanded: true },
      { kind: 'entry', entry: file('D:/proj/sub/deep.rs'), depth: 1, expanded: false },
    ])
  })

  it('再次点击折叠：expanded 复位，子行消失（层缓存保留）', () => {
    let state = createTreeState('D:/proj', false)
    state = layerLoaded(state, 'D:/proj', [dir('D:/proj/sub')])
    state = toggleDir(state, 'D:/proj/sub')
    state = layerLoaded(state, 'D:/proj/sub', [file('D:/proj/sub/deep.rs')])
    state = toggleDir(state, 'D:/proj/sub')
    const rows = visibleRows(state)
    expect(rows).toEqual([{ kind: 'entry', entry: dir('D:/proj/sub'), depth: 0, expanded: false }])
    expect(state.layers['D:/proj/sub'].state).toBe('ready') // 折叠不清缓存，再展开不重拉
  })

  it('根层加载失败：error 行携带契约错误码与消息', () => {
    let state = createTreeState('D:/proj', false)
    state = layerFailed(state, 'D:/proj', { code: 'not_found', message: 'path not found' })
    expect(visibleRows(state)).toEqual([
      { kind: 'error', depth: 0, error: { code: 'not_found', message: 'path not found' } },
    ])
  })
})

describe('reRoot（换工作区根）', () => {
  it('换根重置层与展开状态，根层回到 loading，fallback 标注保留', () => {
    let state = createTreeState('D:/a', true)
    state = layerLoaded(state, 'D:/a', [file('D:/a/x')])
    state = toggleDir(state, 'D:/a')
    const next = reRoot(state, 'D:/b')
    expect(next.root).toBe('D:/b')
    expect(next.layers['D:/b'].state).toBe('loading')
    expect(next.layers['D:/a']).toBeUndefined()
    expect(next.expanded).toEqual({})
    expect(next.fallback).toBe(true)
  })

  it('同根返回原状态（视图层 watch 不触发多余重置）', () => {
    const state = createTreeState('D:/a', false)
    expect(reRoot(state, 'D:/a')).toBe(state)
  })
})
