import { describe, expect, it } from 'vitest'

import type { MarketPlugin } from './marketplace'
import {
  badgesForPlugin,
  MARKET_BADGE_KEYS,
  missingConfirmFields,
  staleSourcesNotice,
} from './marketplaceUi'

function marketPlugin(overrides: Partial<MarketPlugin> = {}): MarketPlugin {
  return {
    id: 'example.hello',
    version: '0.1.0',
    description: 'Demo plugin',
    runtime: 'node',
    engines: {},
    deps: [],
    permissions: ['filesystem.read'],
    source: 'official',
    fileSource: false,
    signatureState: 'signed',
    installedVersion: null,
    updateAvailable: false,
    ...overrides,
  }
}

describe('badgesForPlugin（§3.7 浏览列表徽标）', () => {
  it('已签名 https 源不打徽标', () => {
    expect(badgesForPlugin(marketPlugin())).toEqual([])
  })

  it('unsigned 条目打 unsigned 徽标', () => {
    const badges = badgesForPlugin(marketPlugin({ signatureState: 'unsigned' }))
    expect(badges).toEqual(['unsigned'])
    expect(MARKET_BADGE_KEYS.unsigned).toBe('marketplace.badges.unsigned')
  })

  it('file:// 源条目打 fileSource 徽标', () => {
    const badges = badgesForPlugin(marketPlugin({ fileSource: true }))
    expect(badges).toEqual(['fileSource'])
    expect(MARKET_BADGE_KEYS.fileSource).toBe('marketplace.badges.fileSource')
  })

  it('unsigned + file 源同时打两个徽标（unsigned 在前）', () => {
    const badges = badgesForPlugin(
      marketPlugin({ signatureState: 'unsigned', fileSource: true }),
    )
    expect(badges).toEqual(['unsigned', 'fileSource'])
  })

  it('M3 强制档 enforced 不打 unsigned 徽标', () => {
    expect(badgesForPlugin(marketPlugin({ signatureState: 'enforced' }))).toEqual([])
  })
})

describe('staleSourcesNotice（§3.3 失效源提示条）', () => {
  it('无失效源返回 null（不渲染提示条）', () => {
    expect(staleSourcesNotice([])).toBeNull()
  })

  it('有失效源返回文案键与插值参数', () => {
    expect(staleSourcesNotice(['mirror-a'])).toEqual({
      key: 'marketplace.staleSources',
      params: { sources: 'mirror-a', count: 1 },
    })
  })

  it('多失效源以逗号连接并计数', () => {
    expect(staleSourcesNotice(['mirror-a', 'mirror-b'])).toEqual({
      key: 'marketplace.staleSources',
      params: { sources: 'mirror-a, mirror-b', count: 2 },
    })
  })
})

describe('missingConfirmFields（§3.7 确认对话框字段齐备校验）', () => {
  it('完整条目返回空数组（字段齐备）', () => {
    expect(missingConfirmFields(marketPlugin())).toEqual([])
  })

  it('permissions 允许为空列表（对话框显示「无」）', () => {
    expect(missingConfirmFields(marketPlugin({ permissions: [] }))).toEqual([])
  })

  it('缺 id/version/runtime/source 逐项列出', () => {
    const plugin = marketPlugin({
      id: ' ',
      version: '',
      runtime: '  ',
      source: '',
    })
    expect(missingConfirmFields(plugin)).toEqual(['id', 'version', 'runtime', 'source'])
  })

  it('非法 signatureState 计入缺失', () => {
    const plugin = marketPlugin({ signatureState: 'bogus' as never })
    expect(missingConfirmFields(plugin)).toEqual(['signatureState'])
  })

  it('permissions 非数组计入缺失', () => {
    const plugin = marketPlugin({ permissions: 'filesystem.read' as never })
    expect(missingConfirmFields(plugin)).toEqual(['permissions'])
  })
})
