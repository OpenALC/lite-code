import { describe, expect, it } from 'vitest'

import {
  DEFAULT_MONACO_KIND,
  emptyCompletionList,
  escapeMarkdownPlainText,
  lspKindToMonacoKind,
  toCompletionSuggestions,
  toDefinitionLocations,
  toHover,
} from './lspProviders'

describe('lspKindToMonacoKind', () => {
  it('LSP 1 起的 kind 映射到 monaco 0 起的枚举', () => {
    expect(lspKindToMonacoKind(2)).toBe(0) // Method
    expect(lspKindToMonacoKind(3)).toBe(1) // Function
    expect(lspKindToMonacoKind(6)).toBe(4) // Variable
    expect(lspKindToMonacoKind(7)).toBe(5) // Class
    expect(lspKindToMonacoKind(14)).toBe(17) // Keyword
    expect(lspKindToMonacoKind(22)).toBe(6) // Struct
    expect(lspKindToMonacoKind(25)).toBe(24) // TypeParameter
  })

  it('未知 / 缺省 / 越界 kind 回退 Text', () => {
    expect(lspKindToMonacoKind(undefined)).toBe(DEFAULT_MONACO_KIND)
    expect(lspKindToMonacoKind(null)).toBe(DEFAULT_MONACO_KIND)
    expect(lspKindToMonacoKind(0)).toBe(DEFAULT_MONACO_KIND)
    expect(lspKindToMonacoKind(99)).toBe(DEFAULT_MONACO_KIND)
    expect(lspKindToMonacoKind(Number.NaN)).toBe(DEFAULT_MONACO_KIND)
  })
})

describe('toCompletionSuggestions', () => {
  const position = { lineNumber: 3, column: 9 }
  const word = { startColumn: 7, endColumn: 9 }

  it('映射 label/kind/detail 与词区间，insertText 缺省用 label', () => {
    const suggestions = toCompletionSuggestions(
      [{ label: 'alpha', kind: 6, detail: 'first' }],
      position,
      word,
    )
    expect(suggestions).toEqual([
      {
        label: 'alpha',
        kind: 4,
        detail: 'first',
        insertText: 'alpha',
        range: {
          startLineNumber: 3,
          endLineNumber: 3,
          startColumn: 7,
          endColumn: 9,
        },
      },
    ])
  })

  it('textEdit.newText 优先于 insertText，insertText 优先于 label', () => {
    const suggestions = toCompletionSuggestions(
      [
        { label: 'a', insertText: 'a()', textEdit: { newText: 'a!<>' } },
        { label: 'b', insertText: 'b()' },
      ],
      position,
      word,
    )
    expect(suggestions[0].insertText).toBe('a!<>')
    expect(suggestions[1].insertText).toBe('b()')
  })

  it('空 textEdit.newText / 空 insertText 回退 label；无 detail 不输出 detail 字段', () => {
    const suggestions = toCompletionSuggestions(
      [
        { label: 'x', insertText: '', textEdit: { newText: '' } },
        { label: 'y', detail: '' },
      ],
      position,
      word,
    )
    expect(suggestions[0].insertText).toBe('x')
    expect('detail' in suggestions[0]).toBe(false)
    expect('detail' in suggestions[1]).toBe(false)
  })

  it('空输入与非法项：null/undefined → []，坏 label 被跳过', () => {
    expect(toCompletionSuggestions(null, position, word)).toEqual([])
    expect(toCompletionSuggestions(undefined, position, word)).toEqual([])
    expect(toCompletionSuggestions([], position, word)).toEqual([])
    const suggestions = toCompletionSuggestions(
      // @ts-expect-error 模拟不可信 IPC 载荷
      [{ label: '' }, { label: 42 }, null, { label: 'ok' }],
      position,
      word,
    )
    expect(suggestions.map(item => item.label)).toEqual(['ok'])
  })

  it('emptyCompletionList 与错误降级形状一致', () => {
    expect(emptyCompletionList()).toEqual({ suggestions: [] })
  })
})

describe('escapeMarkdownPlainText', () => {
  it('转义 markdown 特殊字符但保留普通文本', () => {
    expect(escapeMarkdownPlainText('a_b*c')).toBe('a\\_b\\*c')
    expect(escapeMarkdownPlainText('fn main() {}')).toBe('fn main\\(\\) {}')
    expect(escapeMarkdownPlainText('plain')).toBe('plain')
  })
})

describe('toHover', () => {
  it('markdown 段原样保留，纯文本段转义', () => {
    const hover = toHover({
      contents: [
        { value: '**bold**', isMarkdown: true },
        { value: 'a_b', isMarkdown: false },
      ],
    })
    expect(hover).toEqual({
      contents: [{ value: '**bold**' }, { value: 'a\\_b' }],
    })
  })

  it('空 contents / null / undefined → null（无悬停或错误降级）', () => {
    expect(toHover({ contents: [] })).toBeNull()
    expect(toHover(null)).toBeNull()
    expect(toHover(undefined)).toBeNull()
  })
})

describe('toDefinitionLocations', () => {
  it('映射为 uri + 1-based 区间', () => {
    const locations = toDefinitionLocations(
      [
        { path: 'D:\\proj\\a.ts', line: 12, column: 3 },
        { path: '/proj/b.ts', line: 1, column: 1 },
      ],
      path => `uri:${path}`,
    )
    expect(locations).toEqual([
      {
        uri: 'uri:D:\\proj\\a.ts',
        range: {
          startLineNumber: 12,
          startColumn: 3,
          endLineNumber: 12,
          endColumn: 3,
        },
      },
      {
        uri: 'uri:/proj/b.ts',
        range: {
          startLineNumber: 1,
          startColumn: 1,
          endLineNumber: 1,
          endColumn: 1,
        },
      },
    ])
  })

  it('行/列异常值收敛到 ≥1；空 path 与空输入返回 []', () => {
    const locations = toDefinitionLocations(
      [{ path: 'a.ts', line: 0, column: -4 }],
      path => path,
    )
    expect(locations[0].range.startLineNumber).toBe(1)
    expect(locations[0].range.startColumn).toBe(1)
    expect(toDefinitionLocations(null, path => path)).toEqual([])
    expect(
      toDefinitionLocations(
        // @ts-expect-error 模拟不可信 IPC 载荷
        [{ path: '' }, null],
        path => path,
      ),
    ).toEqual([])
  })
})
