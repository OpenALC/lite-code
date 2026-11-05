// LayoutProfile：模式 + 窗口宽度 + 用户布局偏好 → 视图层渲染参数。
// 纯函数，无框架依赖；视图层根据 profile 决定渲染什么（UI.md §8）。
export type UiMode = 'editor' | 'agent'

/** 右侧 Agent 面板形态：常驻停靠 / 收为图标（窗口窄或用户折叠）。 */
export type AgentPanelState = 'docked' | 'icon'

/** 可拖拽/持久化的面板尺寸与折叠状态。 */
export interface LayoutPrefs {
  sidebarWidth: number
  agentWidth: number
  bottomHeight: number
  agentCollapsed: boolean
  bottomCollapsed: boolean
}

export const LAYOUT_LIMITS = {
  sidebar: [160, 360],
  agent: [280, 640],
  bottom: [120, 400],
} as const

export const DEFAULT_LAYOUT: LayoutPrefs = {
  sidebarWidth: 220,
  agentWidth: 400,
  bottomHeight: 180,
  agentCollapsed: false,
  bottomCollapsed: false,
}

export interface LayoutProfile {
  mode: UiMode
  sidebarVisible: boolean
  sidebarWidth: number
  agentPanel: AgentPanelState
  agentWidth: number
  /** 中央区域：编辑器（editor 模式）或 Agent 占位（agent 模式，P3 实装会话流） */
  centerView: 'editor' | 'agent-placeholder'
  tabsVisible: boolean
  bottomPanelVisible: boolean
  bottomHeight: number
}

/** UI.md：Agent 面板是一等公民——常驻右侧、可折叠；窗口窄时收为图标。 */
export const NARROW_WIDTH_PX = 1100

export function clamp(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min
  return Math.min(max, Math.max(min, Math.round(value)))
}

/** 把任意（含旧版/损坏的）偏好收敛到合法范围。 */
export function normalizeLayout(prefs: Partial<LayoutPrefs> = {}): LayoutPrefs {
  return {
    sidebarWidth: clamp(
      prefs.sidebarWidth ?? DEFAULT_LAYOUT.sidebarWidth,
      LAYOUT_LIMITS.sidebar[0],
      LAYOUT_LIMITS.sidebar[1],
    ),
    agentWidth: clamp(
      prefs.agentWidth ?? DEFAULT_LAYOUT.agentWidth,
      LAYOUT_LIMITS.agent[0],
      LAYOUT_LIMITS.agent[1],
    ),
    bottomHeight: clamp(
      prefs.bottomHeight ?? DEFAULT_LAYOUT.bottomHeight,
      LAYOUT_LIMITS.bottom[0],
      LAYOUT_LIMITS.bottom[1],
    ),
    agentCollapsed: prefs.agentCollapsed === true,
    bottomCollapsed: prefs.bottomCollapsed === true,
  }
}

export function resolveProfile(
  mode: UiMode,
  windowWidth: number,
  prefs: Partial<LayoutPrefs> = DEFAULT_LAYOUT,
): LayoutProfile {
  const layout = normalizeLayout(prefs)
  const narrow = windowWidth < NARROW_WIDTH_PX
  const agentPanel: AgentPanelState =
    narrow || layout.agentCollapsed ? 'icon' : 'docked'
  const editorMode = mode === 'editor'
  return {
    mode,
    // P4 三栏：Agent 模式同样保留左栏（项目 + 任务），不再收成活动栏。
    sidebarVisible: true,
    sidebarWidth: layout.sidebarWidth,
    agentPanel,
    agentWidth: layout.agentWidth,
    centerView: editorMode ? 'editor' : 'agent-placeholder',
    tabsVisible: editorMode,
    bottomPanelVisible: editorMode && !layout.bottomCollapsed,
    bottomHeight: layout.bottomHeight,
  }
}
