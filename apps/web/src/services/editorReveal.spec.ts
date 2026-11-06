import { describe, expect, it } from 'vitest'

import { currentReveal, requestReveal, subscribeReveal } from './editorReveal'

describe('editorReveal', () => {
  it('requestReveal 收敛行/列并递增 seq', () => {
    requestReveal('a.ts', 12, 3)
    const first = currentReveal()
    expect(first?.tabId).toBe('a.ts')
    expect(first?.line).toBe(12)
    expect(first?.column).toBe(3)

    requestReveal('a.ts', 12, 3)
    expect(currentReveal()?.seq).toBe((first?.seq ?? 0) + 1)
  })

  it('行/列小于 1 时收敛到 1，小数取整', () => {
    requestReveal('b.ts', 0, -4)
    const target = currentReveal()
    expect(target?.line).toBe(1)
    expect(target?.column).toBe(1)

    requestReveal('b.ts', 7.9, 2.2)
    expect(currentReveal()?.line).toBe(7)
    expect(currentReveal()?.column).toBe(2)
  })

  it('空 tabId 不产生请求', () => {
    requestReveal('b.ts', 5, 1)
    const before = currentReveal()
    requestReveal('', 99, 9)
    expect(currentReveal()).toEqual(before)
  })

  it('订阅者收到请求，退订后不再收到', () => {
    const seen: string[] = []
    const unsub = subscribeReveal(target => {
      if (target) seen.push(`${target.tabId}:${target.line}`)
    })
    requestReveal('c.ts', 4)
    requestReveal('d.ts', 8)
    unsub()
    requestReveal('e.ts', 10)

    expect(seen).toEqual(['c.ts:4', 'd.ts:8'])
  })
})
