import { describe, expect, it } from 'vitest'

import { resolveProfile } from './layoutProfile'

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
