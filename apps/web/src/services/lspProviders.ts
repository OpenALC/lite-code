// LSP DTO → Monaco provider 结果的纯映射（docs/EXTENSIONS.md §3）。
// 只依赖类型，不触碰 monaco 运行时；CodeEditor.vue 负责接线。
// 错误降级约定：bridge 返回 ok:false 时接线层传入空输入，
// 映射结果即"无补全 / 无悬停 / 无定义"，Monaco 回落内置行为。
import type { LspCompletionItem, LspHover, LspLocation } from '@lc-proto'
import type * as monacoNs from 'monaco-editor'

/** LSP CompletionItemKind（1 起）→ monaco CompletionItemKind（0 起）。 */
const KIND_TO_MONACO: Record<number, number> = {
  1: 18, // Text
  2: 0, // Method
  3: 1, // Function
  4: 2, // Constructor
  5: 3, // Field
  6: 4, // Variable
  7: 5, // Class
  8: 7, // Interface
  9: 8, // Module
  10: 9, // Property
  11: 12, // Unit
  12: 13, // Value
  13: 15, // Enum
  14: 17, // Keyword
  15: 28, // Snippet
  16: 19, // Color
  17: 20, // File
  18: 21, // Reference
  19: 23, // Folder
  20: 16, // EnumMember
  21: 14, // Constant
  22: 6, // Struct
  23: 10, // Event
  24: 11, // Operator
  25: 24, // TypeParameter
}

/** monaco CompletionItemKind.Text：未知 / 缺省 kind 的回退。 */
export const DEFAULT_MONACO_KIND = 18

export function lspKindToMonacoKind(kind: number | null | undefined): number {
  if (typeof kind !== 'number' || !Number.isFinite(kind)) return DEFAULT_MONACO_KIND
  return KIND_TO_MONACO[kind] ?? DEFAULT_MONACO_KIND
}

/** 当前词区间：Monaco `getWordUntilPosition` 的形状（只需列）。 */
export interface WordRange {
  startColumn: number
  endColumn: number
}

/**
 * LSP CompletionItem → monaco suggestion。
 * - textEdit.newText 优先于 insertText，insertText 缺省用 label（plain insert）；
 * - 替换区间取当前词（textEdit 的 range 语义 v0 不单独处理）；
 * - label 非字符串 / 为空的项跳过（IPC 载荷不完全可信）。
 */
export function toCompletionSuggestions(
  items: readonly LspCompletionItem[] | null | undefined,
  position: Pick<monacoNs.Position, 'lineNumber'>,
  word: WordRange,
): monacoNs.languages.CompletionItem[] {
  if (!Array.isArray(items)) return []
  const suggestions: monacoNs.languages.CompletionItem[] = []
  for (const item of items) {
    if (!item || typeof item.label !== 'string' || item.label.length === 0) continue
    const insertText =
      typeof item.textEdit?.newText === 'string' && item.textEdit.newText.length > 0
        ? item.textEdit.newText
        : typeof item.insertText === 'string' && item.insertText.length > 0
          ? item.insertText
          : item.label
    suggestions.push({
      label: item.label,
      kind: lspKindToMonacoKind(item.kind),
      ...(typeof item.detail === 'string' && item.detail.length > 0
        ? { detail: item.detail }
        : {}),
      insertText,
      range: {
        startLineNumber: position.lineNumber,
        endLineNumber: position.lineNumber,
        startColumn: word.startColumn,
        endColumn: word.endColumn,
      },
    })
  }
  return suggestions
}

/** 无补全结果（错误降级 / 无 server 命中）时的空 CompletionList。 */
export function emptyCompletionList(): monacoNs.languages.CompletionList {
  return { suggestions: [] }
}

/** 转义 markdown 特殊字符：纯文本段进 IMarkdownString 前用它保住字面量。 */
export function escapeMarkdownPlainText(text: string): string {
  return text.replace(/([\\`*_[\]()#>+-])/g, '\\$1')
}

/**
 * LSP hover → monaco Hover；contents 为空（无悬停 / 错误降级）返回 null。
 * markdown 段原样保留；纯文本段转义后按 markdown 渲染，避免 `_` `*` 被误读。
 */
export function toHover(hover: LspHover | null | undefined): monacoNs.languages.Hover | null {
  const contents = hover?.contents
  if (!Array.isArray(contents) || contents.length === 0) return null
  return {
    contents: contents.map(segment => ({
      value: segment.isMarkdown ? segment.value : escapeMarkdownPlainText(segment.value),
    })),
  }
}

/**
 * LSP Location → monaco Location 形状（uri 由调用方构造，保持纯函数可测）。
 * 行列收敛到 ≥1（契约是 1-based，这里防御 IPC 异常值）。
 */
export function toDefinitionLocations<T>(
  locations: readonly LspLocation[] | null | undefined,
  toUri: (path: string) => T,
): { uri: T; range: monacoNs.IRange }[] {
  if (!Array.isArray(locations)) return []
  return locations
    .filter(location => typeof location?.path === 'string' && location.path.length > 0)
    .map(location => ({
      uri: toUri(location.path),
      range: {
        startLineNumber: Math.max(1, location.line),
        startColumn: Math.max(1, location.column),
        endLineNumber: Math.max(1, location.line),
        endColumn: Math.max(1, location.column),
      },
    }))
}
