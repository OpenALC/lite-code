import { describe, expect, it } from 'vitest'

import {
  EMPTY_QUICK_RECENT,
  normalizeQuickRecent,
  rememberRecent,
  sortByRecent,
} from './quickRecent'

describe('快速打开最近使用', () => {
  it('记录去重并移到最前，超过上限截断', () => {
    const first = rememberRecent(EMPTY_QUICK_RECENT, 'files', 'a')
    const second = rememberRecent(first, 'files', 'b')
    const again = rememberRecent(second, 'files', 'a')

    expect(again.files).toEqual(['a', 'b'])
    expect(rememberRecent(again, 'files', 'c', 2).files).toEqual(['c', 'a'])
    expect(rememberRecent(again, 'files', '').files).toEqual(['a', 'b'])
  })

  it('排序时已知项按记录顺序优先，未知项保持稳定', () => {
    const items = [{ id: 'a' }, { id: 'b' }, { id: 'c' }, { id: 'd' }]
    expect(sortByRecent(items, ['c', 'a']).map(item => item.id)).toEqual([
      'c',
      'a',
      'b',
      'd',
    ])
    expect(sortByRecent(items, []).map(item => item.id)).toEqual(['a', 'b', 'c', 'd'])
  })

  it('归一化非法存储值，过滤空字符串与重复项', () => {
    expect(normalizeQuickRecent(null)).toEqual({ files: [], commands: [] })
    expect(
      normalizeQuickRecent({ files: ['a', 'a', '', 3], commands: 'x' }),
    ).toEqual({ files: ['a'], commands: [] })
  })
})
