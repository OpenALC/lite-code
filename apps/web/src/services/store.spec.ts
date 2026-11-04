import { describe, expect, it } from 'vitest'

import { createStore } from './store'

describe('createStore', () => {
  it('set 更新状态并通知订阅者', () => {
    const store = createStore({ n: 1 })
    const seen: number[] = []
    store.subscribe(s => seen.push(s.n))
    store.set({ n: 2 })
    expect(store.get().n).toBe(2)
    expect(seen).toEqual([2])
  })

  it('unsubscribe 后不再收到通知', () => {
    const store = createStore({ n: 1 })
    const seen: number[] = []
    const unsub = store.subscribe(s => seen.push(s.n))
    store.set({ n: 2 })
    unsub()
    store.set({ n: 3 })
    expect(seen).toEqual([2])
    expect(store.get().n).toBe(3)
  })

  it('多个订阅者都收到通知', () => {
    const store = createStore({ n: 0 })
    let a = 0
    let b = 0
    store.subscribe(s => (a = s.n))
    store.subscribe(s => (b = s.n))
    store.set({ n: 7 })
    expect(a).toBe(7)
    expect(b).toBe(7)
  })
})
