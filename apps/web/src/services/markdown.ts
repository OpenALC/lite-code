// Agent 消息的最小 Markdown 渲染：先转义全部文本，再插入固定标签。
// 不引入第三方解析器；输出只可能包含这里生成的标签，避免模型文本注入 HTML。
const FENCE = /```([^\n`]*)\n([\s\S]*?)```/g

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
}

function renderInline(escaped: string): string {
  return escaped
    .replace(/`([^`\n]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>')
}

function renderBlock(block: string): string {
  const lines = block.split('\n')
  const isList =
    lines.length > 1 && lines.every(line => /^\s*[-*]\s+/.test(line))
  if (isList) {
    const items = lines.map(line =>
      `<li>${renderInline(escapeHtml(line.replace(/^\s*[-*]\s+/, '')))}</li>`,
    )
    return `<ul>${items.join('')}</ul>`
  }
  if (lines.length === 1 && /^#{1,6}\s+/.test(lines[0]!)) {
    const text = renderInline(escapeHtml(lines[0]!.replace(/^#{1,6}\s+/, '')))
    return `<p class="md-h">${text}</p>`
  }
  return `<p>${lines.map(line => renderInline(escapeHtml(line))).join('<br>')}</p>`
}

function renderText(raw: string): string {
  return raw
    .split(/\n{2,}/)
    .map(block => block.trim())
    .filter(Boolean)
    .map(renderBlock)
    .join('')
}

export interface MarkdownOptions {
  /** 代码块复制按钮文案；调用方传入当前语言。 */
  copyLabel?: string
}

/** 把模型输出转成可安全用于 v-html 的 HTML 片段。 */
export function renderMarkdown(source: string, options: MarkdownOptions = {}): string {
  const copyLabel = escapeHtml(options.copyLabel ?? '复制')
  const parts: string[] = []
  let last = 0
  for (const match of source.matchAll(FENCE)) {
    const index = match.index ?? 0
    parts.push(renderText(source.slice(last, index)))
    const lang = (match[1] ?? '').trim()
    const code = (match[2] ?? '').replace(/\n$/, '')
    const attr = lang ? ` data-lang="${escapeHtml(lang)}"` : ''
    const langLabel = lang ? `<span class="md-code-lang">${escapeHtml(lang)}</span>` : '<span></span>'
    parts.push(
      `<div class="md-code-block"><div class="md-code-head">${langLabel}` +
        `<button type="button" class="md-code-copy" data-md-copy>${copyLabel}</button>` +
        `</div><pre class="md-code"${attr}><code>${escapeHtml(code)}</code></pre></div>`,
    )
    last = index + match[0].length
  }
  parts.push(renderText(source.slice(last)))
  return parts.join('')
}
