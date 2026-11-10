import { beforeEach, describe, expect, it } from 'vitest'

import { hasLspLanguage, lspLanguages, setLspLanguages, subscribeLspLanguages } from './lspServers'

beforeEach(() => setLspLanguages([]))

describe('lspServers', () => {
  it('set 之后 has / list 命中，重复与空 id 去重', () => {
    setLspLanguages(['typescript', 'javascript', 'typescript', '', null as never])
    expect(lspLanguages()).toEqual(['typescript', 'javascript'])
    expect(hasLspLanguage('typescript')).toBe(true)
    expect(hasLspLanguage('rust')).toBe(false)
  })

  it('空列表表示无 server（启动失败 / 未启动）', () => {
    setLspLanguages(['rust'])
    setLspLanguages([])
    expect(lspLanguages()).toEqual([])
    expect(hasLspLanguage('rust')).toBe(false)
  })

  it('subscribe 立即收到后续变化，退订后停止', () => {
    const seen: string[][] = []
    const unsub = subscribeLspLanguages(ids => seen.push([...ids]))
    setLspLanguages(['go'])
    setLspLanguages(['go', 'rust'])
    unsub()
    setLspLanguages(['python'])
    expect(seen).toEqual([['go'], ['go', 'rust']])
  })
})
