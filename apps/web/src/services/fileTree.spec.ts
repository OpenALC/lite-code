import { describe, expect, it } from 'vitest'

import type { DirEntryDto } from '@lc-proto'
import {
  basenamePath,
  createTreeState,
  dropTargetDir,
  forgetPath,
  isPathWithin,
  isStrictDescendant,
  joinPath,
  layerFailed,
  layerLoaded,
  parentPath,
  planMove,
  reRoot,
  replacePathPrefix,
  setExpanded,
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

describe('路径父子判断', () => {
  it('按组件边界匹配：/a/b 不是 /a/bc 的父级', () => {
    expect(isPathWithin('/a/b', '/a/b/c.rs')).toBe(true)
    expect(isPathWithin('/a/b', '/a/bc/c.rs')).toBe(false)
    expect(isPathWithin('/a/b', '/a/b')).toBe(true)
    expect(isStrictDescendant('/a/b', '/a/b')).toBe(false)
    expect(isStrictDescendant('/a/b', '/a/b/c.rs')).toBe(true)
  })

  it('Windows 盘符路径大小写不敏感，POSIX 路径大小写敏感', () => {
    expect(isPathWithin('D:/proj', 'd:/PROJ/src/a.rs')).toBe(true)
    expect(isPathWithin('/proj', '/Proj/a.rs')).toBe(false)
    expect(isPathWithin('', '/proj/a.rs')).toBe(false)
  })

  it('parentPath / basenamePath / joinPath 保留分隔符风格', () => {
    expect(parentPath('D:\\proj\\src\\a.rs')).toBe('D:\\proj\\src')
    expect(parentPath('D:\\proj\\a.rs')).toBe('D:\\proj')
    expect(parentPath('D:\\proj')).toBe('D:\\')
    expect(parentPath('/proj/src/a.rs')).toBe('/proj/src')
    expect(parentPath('/a.rs')).toBe('/')
    expect(basenamePath('D:\\proj\\src\\a.rs')).toBe('a.rs')
    expect(basenamePath('/proj/')).toBe('proj')
    expect(joinPath('D:\\proj\\src', 'a.rs')).toBe('D:\\proj\\src\\a.rs')
    expect(joinPath('/', 'a.rs')).toBe('/a.rs')
    expect(joinPath('C:\\', 'a.rs')).toBe('C:\\a.rs')
  })

  it('replacePathPrefix 只替换组件边界且跟随目标分隔符', () => {
    expect(replacePathPrefix('D:\\proj\\src\\a.rs', 'D:\\proj\\src', 'D:\\proj\\lib')).toBe(
      'D:\\proj\\lib\\a.rs',
    )
    expect(replacePathPrefix('/proj/src/a.rs', '/proj/src', '/proj/lib')).toBe(
      '/proj/lib/a.rs',
    )
    expect(replacePathPrefix('/proj/src2/a.rs', '/proj/src', '/proj/lib')).toBeNull()
  })
})

describe('拖放目标解析与移动校验', () => {
  it('目录目标 = 自身，文件目标 = 父目录', () => {
    expect(dropTargetDir(dir('D:/proj/sub'))).toBe('D:/proj/sub')
    expect(dropTargetDir(file('D:/proj/sub/a.rs'))).toBe('D:/proj/sub')
  })

  it('文件拖到目录：目标为目录 + 原文件名', () => {
    expect(planMove('D:/proj', file('D:/proj/a.txt'), dir('D:/proj/sub'))).toEqual({
      ok: true,
      from: 'D:/proj/a.txt',
      to: 'D:/proj/sub/a.txt',
      destDir: 'D:/proj/sub',
    })
  })

  it('拖到文件：移动到该文件的父目录', () => {
    expect(
      planMove('D:/proj', file('D:/proj/a.txt'), file('D:/proj/sub/b.txt')),
    ).toEqual({
      ok: true,
      from: 'D:/proj/a.txt',
      to: 'D:/proj/sub/a.txt',
      destDir: 'D:/proj/sub',
    })
  })

  it('目录拖到根：目标为根目录', () => {
    expect(planMove('D:/proj', dir('D:/proj/sub/inner'), dir('D:/proj'))).toEqual({
      ok: true,
      from: 'D:/proj/sub/inner',
      to: 'D:/proj/inner',
      destDir: 'D:/proj',
    })
  })

  it('拖到自身/子孙：拒绝自嵌套', () => {
    expect(planMove('D:/proj', dir('D:/proj/sub'), dir('D:/proj/sub'))).toEqual({
      ok: false,
      reason: 'selfNesting',
    })
    expect(
      planMove('D:/proj', dir('D:/proj/sub'), dir('D:/proj/sub/inner')),
    ).toEqual({ ok: false, reason: 'selfNesting' })
  })

  it('原地移动：同一父目录返回 sameParent（不产生写操作）', () => {
    expect(planMove('D:/proj', file('D:/proj/a.txt'), file('D:/proj/a.txt'))).toEqual({
      ok: false,
      reason: 'sameParent',
    })
    expect(
      planMove('D:/proj', dir('D:/proj/sub/inner'), dir('D:/proj/sub')),
    ).toEqual({ ok: false, reason: 'sameParent' })
  })

  it('源或目标在根外：拒绝', () => {
    expect(planMove('D:/proj', file('D:/other/a.txt'), dir('D:/proj/sub'))).toEqual({
      ok: false,
      reason: 'outsideRoot',
    })
    expect(planMove('D:/proj', file('D:/proj/a.txt'), dir('D:/other/sub'))).toEqual({
      ok: false,
      reason: 'outsideRoot',
    })
  })
})

describe('移动/删除后的树状态清理', () => {
  it('setExpanded 幂等，值不变返回原状态', () => {
    const state = createTreeState('D:/proj', false)
    const expanded = setExpanded(state, 'D:/proj/sub', true)
    expect(expanded.expanded['D:/proj/sub']).toBe(true)
    expect(setExpanded(expanded, 'D:/proj/sub', true)).toBe(expanded)
    const collapsed = setExpanded(expanded, 'D:/proj/sub', false)
    expect(collapsed.expanded['D:/proj/sub']).toBe(false)
  })

  it('forgetPath 丢弃路径及其后代的层与展开状态，保留兄弟节点', () => {
    let state = createTreeState('D:/proj', false)
    state = layerLoaded(state, 'D:/proj', [dir('D:/proj/sub'), dir('D:/proj/sub2')])
    state = layerLoaded(state, 'D:/proj/sub', [file('D:/proj/sub/a.rs')])
    state = layerLoaded(state, 'D:/proj/sub2', [file('D:/proj/sub2/b.rs')])
    state = setExpanded(state, 'D:/proj/sub', true)
    state = setExpanded(state, 'D:/proj/sub2', true)

    const next = forgetPath(state, 'D:/proj/sub')
    expect(next.layers['D:/proj/sub']).toBeUndefined()
    expect(next.expanded['D:/proj/sub']).toBeUndefined()
    expect(next.layers['D:/proj/sub2']).toBeDefined()
    expect(next.expanded['D:/proj/sub2']).toBe(true)
    expect(next.layers['D:/proj']).toBeDefined()
  })
})
