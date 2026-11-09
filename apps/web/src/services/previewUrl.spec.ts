import { describe, expect, it } from 'vitest'

import { normalizePreviewUrl } from './previewUrl'

describe('Agent 预览地址', () => {
  it('保留合法 http(s) 地址', () => {
    expect(normalizePreviewUrl('http://localhost:5173/')).toEqual({
      url: 'http://localhost:5173/',
    })
    expect(normalizePreviewUrl('https://example.com')).toEqual({
      url: 'https://example.com/',
    })
  })

  it('缺协议时补 http', () => {
    expect(normalizePreviewUrl('localhost:3000')).toEqual({
      url: 'http://localhost:3000/',
    })
  })

  it('拒绝非 http(s) 协议', () => {
    expect(normalizePreviewUrl('javascript:alert(1)')).toEqual({ error: 'scheme' })
    expect(normalizePreviewUrl('file:///etc/hosts')).toEqual({ error: 'scheme' })
    expect(normalizePreviewUrl('data:text/html,hi')).toEqual({ error: 'scheme' })
  })

  it('空值与非法地址报错', () => {
    expect(normalizePreviewUrl('   ')).toEqual({ error: 'empty' })
    expect(normalizePreviewUrl('http://')).toEqual({ error: 'invalid' })
  })
})
