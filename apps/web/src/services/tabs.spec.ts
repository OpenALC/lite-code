import { beforeEach, describe, expect, it } from 'vitest'

import {
  activateTab,
  closeTab,
  INITIAL_TABS_STATE,
  openTab,
  setDirty,
  tabsStore,
  type TabsState,
} from './tabs'

// 单例 store：每个用例前复位到初始状态（操作全部不可变，复位复用同一引用安全）
beforeEach(() => {
  tabsStore.set(INITIAL_TABS_STATE)
})

describe('初始状态', () => {
  it('P0 样例缓冲区对应一个已激活的 main.rs 标签', () => {
    const state = tabsStore.get()
    expect(state.tabs).toEqual([{ id: 'sample', title: 'main.rs', dirty: false }])
    expect(state.activeId).toBe('sample')
  })
})

describe('openTab', () => {
  it('新标签追加到末尾并激活', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    const state = tabsStore.get()
    expect(state.tabs.map(t => t.id)).toEqual(['sample', 'a.rs'])
    expect(state.activeId).toBe('a.rs')
  })

  it('重复打开已存在的 id：去重、只激活、保留原 dirty 状态', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    setDirty('a.rs', true)
    openTab({ id: 'a.rs', title: 'a.rs' })
    const state = tabsStore.get()
    expect(state.tabs.map(t => t.id)).toEqual(['sample', 'a.rs'])
    expect(state.tabs.find(t => t.id === 'a.rs')?.dirty).toBe(true)
    expect(state.activeId).toBe('a.rs')
  })

  it('连续打开多个：顺序保持，激活最后一个', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    openTab({ id: 'b.rs', title: 'b.rs' })
    const state = tabsStore.get()
    expect(state.tabs.map(t => t.id)).toEqual(['sample', 'a.rs', 'b.rs'])
    expect(state.activeId).toBe('b.rs')
  })

  it('关闭后可以重新打开同一 id', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    expect(closeTab('a.rs')).toEqual({ kind: 'closed' })
    openTab({ id: 'a.rs', title: 'a.rs' })
    expect(tabsStore.get().tabs.map(t => t.id)).toEqual(['sample', 'a.rs'])
  })
})

describe('activateTab', () => {
  it('切换激活标签', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    openTab({ id: 'b.rs', title: 'b.rs' })
    expect(activateTab('a.rs')).toBe(true)
    expect(tabsStore.get().activeId).toBe('a.rs')
  })

  it('不存在的 id：返回 false，状态不变', () => {
    const before: TabsState = tabsStore.get()
    expect(activateTab('ghost.rs')).toBe(false)
    expect(tabsStore.get()).toEqual(before)
  })

  it('激活已激活的标签：返回 true，不发通知', () => {
    const seen: TabsState[] = []
    const unsub = tabsStore.subscribe(s => seen.push(s))
    expect(activateTab('sample')).toBe(true)
    expect(seen).toEqual([])
    unsub()
  })
})

describe('setDirty', () => {
  it('标记与清除', () => {
    expect(setDirty('sample', true)).toBe(true)
    expect(tabsStore.get().tabs.find(t => t.id === 'sample')?.dirty).toBe(true)
    expect(setDirty('sample', false)).toBe(true)
    expect(tabsStore.get().tabs.find(t => t.id === 'sample')?.dirty).toBe(false)
  })

  it('不存在的 id：返回 false，状态不变', () => {
    const before: TabsState = tabsStore.get()
    expect(setDirty('ghost.rs', true)).toBe(false)
    expect(tabsStore.get()).toEqual(before)
  })

  it('dirty 值不变时不发通知', () => {
    const seen: TabsState[] = []
    const unsub = tabsStore.subscribe(s => seen.push(s))
    expect(setDirty('sample', false)).toBe(true)
    expect(seen).toEqual([])
    setDirty('sample', true)
    expect(seen.length).toBe(1)
    unsub()
  })
})

describe('closeTab · 关闭保护', () => {
  it('干净标签直接关闭', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    expect(closeTab('a.rs')).toEqual({ kind: 'closed' })
    expect(tabsStore.get().tabs.map(t => t.id)).toEqual(['sample'])
    expect(tabsStore.get().activeId).toBe('sample')
  })

  it('dirty 标签：返回 needsConfirm，不关闭、激活不变（无弹窗，视图层决定确认）', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    setDirty('a.rs', true)
    expect(closeTab('a.rs')).toEqual({ kind: 'needsConfirm', tabId: 'a.rs' })
    const state = tabsStore.get()
    expect(state.tabs.map(t => t.id)).toEqual(['sample', 'a.rs'])
    expect(state.activeId).toBe('a.rs')
  })

  it('dirty 标签 force 关闭：真正移除', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    setDirty('a.rs', true)
    expect(closeTab('a.rs', { force: true })).toEqual({ kind: 'closed' })
    expect(tabsStore.get().tabs.map(t => t.id)).toEqual(['sample'])
  })

  it('非激活的 dirty 标签同样受保护，激活保持不变', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    setDirty('sample', true)
    expect(closeTab('sample')).toEqual({ kind: 'needsConfirm', tabId: 'sample' })
    expect(tabsStore.get().activeId).toBe('a.rs')
  })

  it('不存在的 id：返回 notFound', () => {
    expect(closeTab('ghost.rs')).toEqual({ kind: 'notFound' })
  })
})

describe('closeTab · 激活顺延', () => {
  it('关闭激活标签：优先激活右邻', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    openTab({ id: 'b.rs', title: 'b.rs' })
    activateTab('a.rs')
    expect(closeTab('a.rs')).toEqual({ kind: 'closed' })
    expect(tabsStore.get().activeId).toBe('b.rs')
  })

  it('关闭末尾的激活标签：回退到左邻', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    expect(closeTab('a.rs')).toEqual({ kind: 'closed' })
    expect(tabsStore.get().activeId).toBe('sample')
  })

  it('关闭唯一标签：activeId 置 null', () => {
    expect(closeTab('sample', { force: true })).toEqual({ kind: 'closed' })
    const state = tabsStore.get()
    expect(state.tabs).toEqual([])
    expect(state.activeId).toBe(null)
  })

  it('关闭非激活标签：激活保持不变', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    expect(closeTab('sample')).toEqual({ kind: 'closed' })
    expect(tabsStore.get().activeId).toBe('a.rs')
  })
})

describe('subscribe（store.ts 模式）', () => {
  it('每次变更通知订阅者，携带最新状态', () => {
    const seen: TabsState[] = []
    const unsub = tabsStore.subscribe(s => seen.push(s))
    openTab({ id: 'a.rs', title: 'a.rs' })
    setDirty('a.rs', true)
    expect(seen.map(s => s.activeId)).toEqual(['a.rs', 'a.rs'])
    expect(seen[1]?.tabs.find(t => t.id === 'a.rs')?.dirty).toBe(true)
    unsub()
  })

  it('unsubscribe 后不再收到通知', () => {
    const seen: TabsState[] = []
    const unsub = tabsStore.subscribe(s => seen.push(s))
    openTab({ id: 'a.rs', title: 'a.rs' })
    unsub()
    openTab({ id: 'b.rs', title: 'b.rs' })
    expect(seen.length).toBe(1)
    expect(tabsStore.get().tabs.map(t => t.id)).toEqual(['sample', 'a.rs', 'b.rs'])
  })
})
