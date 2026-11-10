// 插件市场 UI 纯函数（docs/plans/plugin-marketplace.md §3.7/§3.8）：
// 浏览列表徽标判定、staleSources 提示条文案键、安装确认对话框字段齐备校验。
// 与 marketplace.ts 分离——这里不触碰 IPC，组件只接线（项目惯例：纯函数进单测）。
import type { MarketPlugin, SignatureState } from './marketplace'

/** 浏览列表可见徽标（§3.7「unsigned 与 file:// 源条目打可见徽标」）。 */
export type MarketBadge = 'unsigned' | 'fileSource'

export const MARKET_BADGE_KEYS: Record<MarketBadge, string> = {
  unsigned: 'marketplace.badges.unsigned',
  fileSource: 'marketplace.badges.fileSource',
}

/** 返回条目应展示的徽标（顺序固定：unsigned 在前，本地源在后）。 */
export function badgesForPlugin(plugin: MarketPlugin): MarketBadge[] {
  const badges: MarketBadge[] = []
  if (plugin.signatureState === 'unsigned') badges.push('unsigned')
  if (plugin.fileSource) badges.push('fileSource')
  return badges
}

/** staleSources 顶部提示条：无失效源返回 null（不渲染），否则给出文案键与插值参数。 */
export interface StaleSourcesNotice {
  key: 'marketplace.staleSources'
  params: { sources: string; count: number }
}

export function staleSourcesNotice(staleSources: string[]): StaleSourcesNotice | null {
  if (staleSources.length === 0) return null
  return {
    key: 'marketplace.staleSources',
    params: { sources: staleSources.join(', '), count: staleSources.length },
  }
}

/** §3.7 安装确认对话框必显字段（id/version/runtime/来源源名/permissions/signatureState）。 */
export type ConfirmField =
  | 'id'
  | 'version'
  | 'runtime'
  | 'source'
  | 'signatureState'
  | 'permissions'

const SIGNATURE_STATES: readonly SignatureState[] = ['signed', 'unsigned', 'enforced']

/**
 * 字段齐备校验：返回缺失字段列表（空数组 = 齐备，可渲染确认对话框）。
 * permissions 允许为空列表（对话框显示「无」），但必须是数组；
 * signatureState 必须是 signed/unsigned/enforced 三档之一（M3 预留 enforced）。
 */
export function missingConfirmFields(
  plugin: Pick<
    MarketPlugin,
    'id' | 'version' | 'runtime' | 'source' | 'signatureState' | 'permissions'
  >,
): ConfirmField[] {
  const missing: ConfirmField[] = []
  if (typeof plugin.id !== 'string' || plugin.id.trim() === '') missing.push('id')
  if (typeof plugin.version !== 'string' || plugin.version.trim() === '') {
    missing.push('version')
  }
  if (typeof plugin.runtime !== 'string' || plugin.runtime.trim() === '') {
    missing.push('runtime')
  }
  if (typeof plugin.source !== 'string' || plugin.source.trim() === '') {
    missing.push('source')
  }
  if (!SIGNATURE_STATES.includes(plugin.signatureState)) missing.push('signatureState')
  if (!Array.isArray(plugin.permissions)) missing.push('permissions')
  return missing
}
