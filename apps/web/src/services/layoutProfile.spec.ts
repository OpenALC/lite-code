import { describe, expect, it } from 'vitest'

import {
  DEFAULT_LAYOUT,
  LAYOUT_LIMITS,
  normalizeLayout,
  resolveProfile,
} from './layoutProfile'

describe('resolveProfile', () => {
  it('editor 模式宽窗口：Monaco 居中，Agent 面板停靠', () => {
    const p = resolveProfile('editor', 1600)
    expect(p.centerView).toBe('editor')
    expect(p.agentPanel).toBe('docked')
    expect(p.tabsVisible).toBe(true)
    expect(p.bottomPanelVisible).toBe(true)
  })

  it('editor 模式窄窗口：Agent 面板收为图标，不挤压编辑器', () => {
    const p = resolveProfile('editor', 1080)
    expect(p.agentPanel).toBe('icon')
    expect(p.centerView).toBe('editor')
  })

  it('阈值边界：1100px 停靠，1099px 收图标', () => {
    expect(resolveProfile('editor', 1100).agentPanel).toBe('docked')
    expect(resolveProfile('editor', 1099).agentPanel).toBe('icon')
  })

  it('agent 模式：中央只显示占位，标签页与底部面板隐藏', () => {
    const p = resolveProfile('agent', 1600)
    expect(p.centerView).toBe('agent-placeholder')
    expect(p.tabsVisible).toBe(false)
    expect(p.bottomPanelVisible).toBe(false)
  })

  it('agent 模式窄窗口：Agent 面板仍按宽度收图标', () => {
    expect(resolveProfile('agent', 800).agentPanel).toBe('icon')
  })
})

describe('布局偏好', () => {
  it('默认尺寸进入 profile', () => {
    const p = resolveProfile('editor', 1600)
    expect(p.sidebarWidth).toBe(DEFAULT_LAYOUT.sidebarWidth)
    expect(p.agentWidth).toBe(DEFAULT_LAYOUT.agentWidth)
    expect(p.bottomHeight).toBe(DEFAULT_LAYOUT.bottomHeight)
    expect(p.sidebarVisible).toBe(true)
  })

  it('用户折叠 Agent / 底部面板', () => {
    const p = resolveProfile('editor', 1600, {
      ...DEFAULT_LAYOUT,
      agentCollapsed: true,
      bottomCollapsed: true,
    })
    expect(p.agentPanel).toBe('icon')
    expect(p.bottomPanelVisible).toBe(false)
  })

  it('尺寸越界被 clamp，NaN 回落最小值', () => {
    const layout = normalizeLayout({
      sidebarWidth: 9999,
      agentWidth: 1,
      bottomHeight: Number.NaN,
    })
    expect(layout.sidebarWidth).toBe(LAYOUT_LIMITS.sidebar[1])
    expect(layout.agentWidth).toBe(LAYOUT_LIMITS.agent[0])
    expect(layout.bottomHeight).toBe(LAYOUT_LIMITS.bottom[0])
  })
})
