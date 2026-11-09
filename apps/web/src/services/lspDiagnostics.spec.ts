import { beforeEach, describe, expect, it } from 'vitest'

import type { LspDiagnostic } from './bridge'
import {
  clearDiagnostics,
  diagnosticsFor,
  filterProblems,
  flattenDiagnostics,
  setFileDiagnostics,
  subscribeDiagnostics,
} from './lspDiagnostics'

const diagnostic = (message: string): LspDiagnostic => ({
  severity: 1,
  message,
  line: 1,
  column: 1,
  endLine: 1,
  endColumn: 2,
})

beforeEach(clearDiagnostics)

describe('lspDiagnostics', () => {
  it('set/read by path and empty array clears', () => {
    setFileDiagnostics('a.ts', 'lsp', [diagnostic('x')])
    expect(diagnosticsFor('a.ts')).toHaveLength(1)
    expect(diagnosticsFor('b.ts')).toEqual([])

    setFileDiagnostics('a.ts', 'lsp', [])
    expect(diagnosticsFor('a.ts')).toEqual([])
  })

  it('empty path is ignored', () => {
    setFileDiagnostics('', 'lsp', [diagnostic('x')])
    expect(diagnosticsFor('')).toEqual([])
  })

  it('merges multiple servers for the same path', () => {
    setFileDiagnostics('a.ts', 'server-a', [diagnostic('a')])
    setFileDiagnostics('a.ts', 'server-b', [diagnostic('b')])
    expect(diagnosticsFor('a.ts').map(item => item.message).sort()).toEqual(['a', 'b'])
    setFileDiagnostics('a.ts', 'server-a', [])
    expect(diagnosticsFor('a.ts').map(item => item.message)).toEqual(['b'])
  })

  it('flattens problems by path/line and keeps server info', () => {
    const second = { ...diagnostic('later'), line: 4 }
    setFileDiagnostics('b.ts', 'lsp', [diagnostic('b1')])
    setFileDiagnostics('a.ts', 'server-a', [{ ...diagnostic('a2'), line: 9 }, diagnostic('a1')])
    setFileDiagnostics('a.ts', 'server-b', [second])

    const items = flattenDiagnostics({
      'a.ts': { 'server-a': [{ ...diagnostic('a2'), line: 9 }, diagnostic('a1')], 'server-b': [second] },
      'b.ts': { lsp: [diagnostic('b1')] },
    })

    expect(items.map(item => `${item.path}:${item.diagnostic.line}:${item.diagnostic.message}`)).toEqual([
      'a.ts:1:a1',
      'a.ts:4:later',
      'a.ts:9:a2',
      'b.ts:1:b1',
    ])
    expect(items.find(item => item.diagnostic.message === 'later')?.serverId).toBe('server-b')
  })

  it('flattening empty state returns []', () => {
    expect(flattenDiagnostics({})).toEqual([])
  })

  it('filters problems by severity without mutating the source', () => {
    const items = [
      { path: 'a', serverId: 'lsp', diagnostic: { ...diagnostic('e'), severity: 1 } },
      { path: 'a', serverId: 'lsp', diagnostic: { ...diagnostic('w'), severity: 2 } },
      { path: 'a', serverId: 'lsp', diagnostic: { ...diagnostic('i'), severity: 3 } },
    ]

    expect(filterProblems(items, 'errors').map(item => item.diagnostic.message)).toEqual(['e'])
    expect(filterProblems(items, 'warnings').map(item => item.diagnostic.message)).toEqual(['w'])
    expect(filterProblems(items, 'all')).toHaveLength(3)
    expect(items).toHaveLength(3)
  })

  it('subscribers receive aggregated state and stop after unsubscribe', () => {
    const seen: number[] = []
    const unsub = subscribeDiagnostics(state => seen.push(Object.keys(state).length))
    setFileDiagnostics('a.ts', 'lsp', [diagnostic('x')])
    setFileDiagnostics('b.ts', 'lsp', [diagnostic('y')])
    unsub()
    setFileDiagnostics('c.ts', 'lsp', [diagnostic('z')])
    expect(seen).toEqual([1, 2])
  })
})
