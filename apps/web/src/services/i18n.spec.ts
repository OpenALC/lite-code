import { describe, expect, it } from 'vitest'

import { catalogs, createT, flattenKeys, lookup } from './i18n'

describe('双语目录', () => {
  it('zh-CN 与 en 的 key 集合完全一致（UI.md §4：事后补 i18n 的成本是事前的五倍）', () => {
    expect(flattenKeys(catalogs['zh-CN'])).toEqual(flattenKeys(catalogs.en))
  })

  it('目录非空', () => {
    expect(flattenKeys(catalogs['zh-CN']).length).toBeGreaterThan(5)
  })
})

describe('lookup', () => {
  it('按 a.b.c 路径取值', () => {
    expect(lookup(catalogs['zh-CN'], 'app.title')).toBe('Lite Code')
    expect(lookup(catalogs.en, 'mode.editor')).toBe('Editor')
  })

  it('缺失返回 undefined，不抛错', () => {
    expect(lookup(catalogs['zh-CN'], 'no.such.key')).toBeUndefined()
    expect(lookup(catalogs['zh-CN'], 'app')).toBeUndefined() // 非叶子节点
  })
})

describe('createT', () => {
  it('当前语言命中', () => {
    const t = createT('zh-CN')
    expect(t('agent.placeholderTitle')).toBe('Agent 模式')
  })

  it('当前语言缺失时回退 en，再缺失返回 key 本身', () => {
    const t = createT('zh-CN')
    expect(t('totally.missing.key')).toBe('totally.missing.key')
  })
})
