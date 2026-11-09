import { beforeEach, describe, expect, it } from 'vitest'

import { resolveEditorStrategy } from './largeFile'
import {
  activateTab,
  activeTab,
  adjacentTabId,
  clearClosedTabs,
  closeTab,
  INITIAL_TABS_STATE,
  markSaved,
  moveTabTo,
  openTab,
  relocateTabs,
  removeTabsUnder,
  renameTab,
  reopenClosedTab,
  setDirty,
  tabsStore,
  updateContent,
  type TabsState,
} from './tabs'

// 单例 store：每个用例前复位到初始状态（操作全部不可变，复位复用同一引用安全）
beforeEach(() => {
  tabsStore.set(INITIAL_TABS_STATE)
  clearClosedTabs()
})

describe('初始状态', () => {
  it('P0 样例缓冲区对应一个已激活的 main.rs 标签（不可保存）', () => {
    const state = tabsStore.get()
    expect(state.tabs).toHaveLength(1)
    expect(state.tabs[0]).toMatchObject({
      id: 'sample',
      title: 'main.rs',
      path: null,
      dirty: false,
    })
    expect(state.tabs[0].content).toContain('fn main()')
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

describe('renameTab', () => {
  it('只更新标题，内容与 dirty 不变', () => {
    openTab({ id: 'a.rs', title: 'a.rs', content: 'x' })
    expect(renameTab('a.rs', '设置')).toBe(true)
    const tab = tabsStore.get().tabs.find(t => t.id === 'a.rs')
    expect(tab?.title).toBe('设置')
    expect(tab?.content).toBe('x')
    expect(tab?.dirty).toBe(false)
  })

  it('标题相同或 id 不存在时不发通知', () => {
    const seen: TabsState[] = []
    const unsub = tabsStore.subscribe(s => seen.push(s))
    expect(renameTab('sample', 'main.rs')).toBe(false)
    expect(renameTab('ghost.rs', 'x')).toBe(false)
    expect(seen).toEqual([])
    unsub()
  })
})

describe('setDirty', () => {  it('标记与清除', () => {
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

describe('内容与保存', () => {
  it('openTab 携带内容/路径，初始干净', () => {
    openTab({ id: 'a.rs', title: 'a.rs', content: 'fn a() {}' })
    const tab = tabsStore.get().tabs.find(t => t.id === 'a.rs')
    expect(tab).toMatchObject({ path: 'a.rs', content: 'fn a() {}', dirty: false })
  })

  it('openTab 可携带大文件只读策略', () => {
    openTab({
      id: 'big.txt',
      title: 'big.txt',
      content: 'x',
      strategy: resolveEditorStrategy(25 * 1024 * 1024),
    })
    const tab = tabsStore.get().tabs.find(t => t.id === 'big.txt')
    expect(tab?.strategy.tier).toBe('readonly')
    expect(tab?.strategy.readOnly).toBe(true)
  })

  it('updateContent 更新内容并置 dirty；同值不通知', () => {
    openTab({ id: 'a.rs', title: 'a.rs', content: 'v1' })
    const seen: TabsState[] = []
    const unsub = tabsStore.subscribe(s => seen.push(s))
    expect(updateContent('a.rs', 'v2')).toBe(true)
    expect(tabsStore.get().tabs.find(t => t.id === 'a.rs')).toMatchObject({
      content: 'v2',
      dirty: true,
    })
    expect(seen.length).toBe(1)
    updateContent('a.rs', 'v2')
    expect(seen.length).toBe(1)
    unsub()
  })

  it('回到已保存内容自动清除 dirty；保存后基线随之更新', () => {
    openTab({ id: 'a.rs', title: 'a.rs', content: 'v1' })
    updateContent('a.rs', 'v2')
    updateContent('a.rs', 'v1')
    expect(tabsStore.get().tabs.find(t => t.id === 'a.rs')?.dirty).toBe(false)

    updateContent('a.rs', 'v2')
    markSaved('a.rs')
    updateContent('a.rs', 'v1')
    expect(tabsStore.get().tabs.find(t => t.id === 'a.rs')?.dirty).toBe(true)
  })

  it('markSaved 清除 dirty 但保留内容', () => {
    openTab({ id: 'a.rs', title: 'a.rs', content: 'v1' })
    updateContent('a.rs', 'v2')
    expect(markSaved('a.rs')).toBe(true)
    expect(tabsStore.get().tabs.find(t => t.id === 'a.rs')).toMatchObject({
      content: 'v2',
      dirty: false,
    })
  })

  it('activeTab 返回当前标签或 null', () => {
    expect(activeTab()?.id).toBe('sample')
    closeTab('sample', { force: true })
    expect(activeTab()).toBe(null)
  })

  it('不存在的 id：更新/保存均返回 false', () => {
    expect(updateContent('ghost.rs', 'x')).toBe(false)
    expect(markSaved('ghost.rs')).toBe(false)
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

describe('文件系统移动/重命名后的标签同步', () => {
  it('目录前缀更新后代标签的 id / path，激活 id 跟随', () => {
    openTab({ id: 'D:/proj/src/a.rs', title: 'a.rs', content: 'a' })
    openTab({ id: 'D:/proj/b.rs', title: 'b.rs', content: 'b' })
    activateTab('D:/proj/src/a.rs')
    expect(relocateTabs('D:/proj/src', 'D:/proj/lib')).toBe(true)

    const state = tabsStore.get()
    expect(state.tabs.find(t => t.path === 'D:/proj/lib/a.rs')).toMatchObject({
      id: 'D:/proj/lib/a.rs',
      title: 'a.rs',
    })
    expect(state.tabs.find(t => t.path === 'D:/proj/b.rs')?.id).toBe('D:/proj/b.rs')
    expect(state.activeId).toBe('D:/proj/lib/a.rs')
  })

  it('重命名单个文件：标题随新文件名更新，内容与 dirty 保留', () => {
    openTab({ id: 'D:/proj/a.rs', title: 'a.rs', content: 'v1' })
    updateContent('D:/proj/a.rs', 'v2')
    expect(relocateTabs('D:/proj/a.rs', 'D:/proj/b.rs')).toBe(true)

    const tab = tabsStore.get().tabs.find(t => t.path === 'D:/proj/b.rs')
    expect(tab).toMatchObject({
      id: 'D:/proj/b.rs',
      title: 'b.rs',
      content: 'v2',
      dirty: true,
    })
  })

  it('组件边界外不更新；无变化返回 false 且不发通知', () => {
    openTab({ id: 'D:/proj/src2/a.rs', title: 'a.rs', content: 'a' })
    const seen: TabsState[] = []
    const unsub = tabsStore.subscribe(s => seen.push(s))
    expect(relocateTabs('D:/proj/src', 'D:/proj/lib')).toBe(false)
    expect(tabsStore.get().tabs.find(t => t.path === 'D:/proj/src2/a.rs')?.id).toBe(
      'D:/proj/src2/a.rs',
    )
    expect(seen).toEqual([])
    unsub()
  })
})

describe('文件系统删除后的标签清理', () => {
  it('关闭目录下全部标签，保留同前缀边界外的标签', () => {
    openTab({ id: 'D:/proj/src/a.rs', title: 'a.rs', content: 'a' })
    openTab({ id: 'D:/proj/src/deep/b.rs', title: 'b.rs', content: 'b' })
    openTab({ id: 'D:/proj/src2/c.rs', title: 'c.rs', content: 'c' })

    expect(removeTabsUnder('D:/proj/src')).toBe(2)
    expect(tabsStore.get().tabs.map(t => t.path)).toEqual([null, 'D:/proj/src2/c.rs'])
  })

  it('dirty 标签强制关闭，激活顺延到相邻标签', () => {
    openTab({ id: 'D:/proj/src/a.rs', title: 'a.rs', content: 'a' })
    openTab({ id: 'D:/proj/b.rs', title: 'b.rs', content: 'b' })
    activateTab('D:/proj/src/a.rs')
    setDirty('D:/proj/src/a.rs', true)

    expect(removeTabsUnder('D:/proj/src')).toBe(1)
    const state = tabsStore.get()
    expect(state.tabs.map(t => t.id)).toEqual(['sample', 'D:/proj/b.rs'])
    expect(state.activeId).toBe('D:/proj/b.rs')
  })

  it('删除单文件只关闭精确路径，不误伤同名前缀', () => {
    openTab({ id: 'D:/proj/a.rs', title: 'a.rs', content: 'a' })
    openTab({ id: 'D:/proj/a.rs.bak', title: 'a.rs.bak', content: 'b' })
    expect(removeTabsUnder('D:/proj/a.rs')).toBe(1)
    expect(tabsStore.get().tabs.map(t => t.path)).toEqual([null, 'D:/proj/a.rs.bak'])
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

describe('标签拖拽排序', () => {
  it('向前/向后移动，激活标签与内容不变', () => {
    openTab({ id: 'a.rs', title: 'a.rs', content: 'a' })
    openTab({ id: 'b.rs', title: 'b.rs', content: 'b' })
    openTab({ id: 'c.rs', title: 'c.rs', content: 'c' })
    activateTab('b.rs')

    expect(moveTabTo('c.rs', 'a.rs', 'before')).toBe(true)
    expect(tabsStore.get().tabs.map(t => t.id)).toEqual(['sample', 'c.rs', 'a.rs', 'b.rs'])
    expect(tabsStore.get().activeId).toBe('b.rs')
    expect(tabsStore.get().tabs.find(t => t.id === 'c.rs')?.content).toBe('c')

    expect(moveTabTo('c.rs', 'b.rs', 'after')).toBe(true)
    expect(tabsStore.get().tabs.map(t => t.id)).toEqual(['sample', 'a.rs', 'b.rs', 'c.rs'])
  })

  it('拖到自身或未知 id 时不改变状态、不发通知', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    const seen: TabsState[] = []
    const unsub = tabsStore.subscribe(s => seen.push(s))

    expect(moveTabTo('a.rs', 'a.rs', 'before')).toBe(false)
    expect(moveTabTo('missing', 'a.rs', 'after')).toBe(false)
    expect(moveTabTo('a.rs', 'missing', 'after')).toBe(false)
    expect(seen).toEqual([])
    unsub()
  })

  it('原位前后移动按“位置未变”处理', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    openTab({ id: 'b.rs', title: 'b.rs' })

    expect(moveTabTo('a.rs', 'b.rs', 'before')).toBe(false)
    expect(moveTabTo('b.rs', 'a.rs', 'after')).toBe(false)
  })
})

describe('重新打开已关闭的标签', () => {
  it('恢复内容、dirty 与激活状态', () => {
    openTab({ id: 'a.rs', title: 'a.rs', content: 'a' })
    updateContent('a.rs', 'changed')

    expect(closeTab('a.rs', { force: true }).kind).toBe('closed')
    const restored = reopenClosedTab()

    expect(restored).toMatchObject({ id: 'a.rs', title: 'a.rs', content: 'changed', dirty: true })
    expect(tabsStore.get().activeId).toBe('a.rs')
    expect(tabsStore.get().tabs.map(t => t.id)).toEqual(['sample', 'a.rs'])
  })

  it('按 LIFO 顺序恢复，栈空后返回 null', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    openTab({ id: 'b.rs', title: 'b.rs' })
    closeTab('a.rs', { force: true })
    closeTab('b.rs', { force: true })

    expect(reopenClosedTab()?.id).toBe('b.rs')
    expect(reopenClosedTab()?.id).toBe('a.rs')
    expect(reopenClosedTab()).toBeNull()
  })

  it('已存在同 id 时只激活，不重复创建', () => {
    openTab({ id: 'a.rs', title: 'a.rs', content: 'old' })
    closeTab('a.rs', { force: true })
    openTab({ id: 'a.rs', title: 'a.rs', content: 'new' })

    expect(reopenClosedTab()?.content).toBe('new')
    expect(tabsStore.get().tabs.filter(t => t.id === 'a.rs')).toHaveLength(1)
  })

  it('文件删除导致的关闭不进入重开栈', () => {
    openTab({ id: 'D:/proj/src/a.rs', title: 'a.rs' })
    removeTabsUnder('D:/proj/src')

    expect(reopenClosedTab()).toBeNull()
  })
})

describe('Ctrl+Tab 循环切换', () => {
  it('正向/反向循环并跳过边界', () => {
    openTab({ id: 'a.rs', title: 'a.rs' })
    openTab({ id: 'b.rs', title: 'b.rs' })
    activateTab('a.rs')

    expect(adjacentTabId(tabsStore.get(), 1)).toBe('b.rs')
    expect(adjacentTabId(tabsStore.get(), -1)).toBe('sample')

    activateTab('sample')
    expect(adjacentTabId(tabsStore.get(), -1)).toBe('b.rs')
  })

  it('少于两个标签或激活项未知时返回 null（未知时回退首个）', () => {
    expect(adjacentTabId(tabsStore.get(), 1)).toBeNull()

    openTab({ id: 'a.rs', title: 'a.rs' })
    const state = tabsStore.get()
    expect(adjacentTabId({ ...state, activeId: 'missing' }, 1)).toBe('sample')
  })
})
