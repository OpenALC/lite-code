// LayoutProfile 骨架（MILESTONES P0：切到 Agent 视图只显示占位）。
// 纯函数，无框架依赖——视图层根据 profile 决定渲染什么。
export type UiMode = 'editor' | 'agent'

/** 右侧 Agent 面板形态：常驻停靠 / 收为图标（窗口窄时，不挤压编辑器）。 */
export type AgentPanelState = 'docked' | 'icon'

export interface LayoutProfile {
  mode: UiMode
  agentPanel: AgentPanelState
  /** 中央区域：编辑器（editor 模式）或 Agent 占位（agent 模式，P3 实装会话流） */
  centerView: 'editor' | 'agent-placeholder'
  tabsVisible: boolean
  bottomPanelVisible: boolean
}

/** UI.md：Agent 面板是一等公民——常驻右侧、可折叠；窗口窄时收为图标。 */
export const NARROW_WIDTH_PX = 1100

export function resolveProfile(mode: UiMode, windowWidth: number): LayoutProfile {
  // Agent 面板是常驻一等公民，两种模式都存在；只有窗口宽度决定它是否收为图标
  const agentPanel: AgentPanelState = windowWidth < NARROW_WIDTH_PX ? 'icon' : 'docked'
  if (mode === 'agent') {
    return {
      mode,
      agentPanel,
      centerView: 'agent-placeholder',
      tabsVisible: false,
      bottomPanelVisible: false,
    }
  }
  return {
    mode,
    agentPanel,
    centerView: 'editor',
    tabsVisible: true,
    bottomPanelVisible: true,
  }
}
