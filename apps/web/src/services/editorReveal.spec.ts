import { describe, expect, it } from 'vitest'

import { consumeReveal, currentReveal, requestReveal, subscribeReveal } from './editorReveal'

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

  it('consumeReveal 只消费一次，且不消费其它标签的请求', () => {
    requestReveal('f.ts', 30, 2)
    expect(consumeReveal('g.ts')).toBeNull()
    const first = consumeReveal('f.ts')
    expect(first?.line).toBe(30)
    expect(consumeReveal('f.ts')).toBeNull()

    requestReveal('f.ts', 40)
    expect(consumeReveal('f.ts')?.line).toBe(40)
  })
})
