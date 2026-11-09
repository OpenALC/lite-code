import { describe, expect, it, vi } from 'vitest'

import { consumeBackForSheet, nextFocusIndex } from './sheet'

describe('nextFocusIndex', () => {
  it('Tab 正向循环，Shift+Tab 反向循环', () => {
    expect(nextFocusIndex(0, 3)).toBe(1)
    expect(nextFocusIndex(1, 3)).toBe(2)
    expect(nextFocusIndex(2, 3)).toBe(0)
    expect(nextFocusIndex(0, 3, true)).toBe(2)
    expect(nextFocusIndex(2, 3, true)).toBe(1)
  })

  it('焦点在 sheet 外时从头或尾进入', () => {
    expect(nextFocusIndex(-1, 4)).toBe(0)
    expect(nextFocusIndex(-1, 4, true)).toBe(3)
    expect(nextFocusIndex(9, 4)).toBe(0)
    expect(nextFocusIndex(9, 4, true)).toBe(3)
  })

  it('没有可聚焦元素时返回 -1', () => {
    expect(nextFocusIndex(0, 0)).toBe(-1)
  })
})

describe('consumeBackForSheet', () => {
  it('sheet 打开时关闭并消费返回键', () => {
    const close = vi.fn()
    expect(consumeBackForSheet(true, close)).toBe(true)
    expect(close).toHaveBeenCalledTimes(1)
  })

  it('sheet 关闭时不消费，交给默认返回', () => {
    const close = vi.fn()
    expect(consumeBackForSheet(false, close)).toBe(false)
    expect(close).not.toHaveBeenCalled()
  })
})
