import { beforeEach, describe, expect, it } from 'vitest'

import type { LspDiagnostic } from './bridge'
import {
  clearDiagnostics,
  diagnosticsFor,
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
