import { describe, expect, it } from 'vitest'

import { renderMarkdown } from './markdown'

describe('Agent 消息 Markdown 渲染', () => {
  it('转义模型输出中的 HTML，阻止标签注入', () => {
    const html = renderMarkdown('<img src=x onerror=alert(1)> & "quoted"')

    expect(html).toContain('&lt;img src=x onerror=alert(1)&gt;')
    expect(html).toContain('&amp;')
    expect(html).not.toContain('<img')
  })

  it('围栏代码块带语言标签与复制按钮，代码写入 data-lang', () => {
    const html = renderMarkdown('前文\n\n```rust\nfn main() {}\n```\n\n后文')

    expect(html).toContain('<div class="md-code-block">')
    expect(html).toContain('<span class="md-code-lang">rust</span>')
    expect(html).toContain('<button type="button" class="md-code-copy" data-md-copy>复制</button>')
    expect(html).toContain('<pre class="md-code" data-lang="rust"><code>fn main() {}</code></pre>')
    expect(html).toContain('<p>前文</p>')
    expect(html).toContain('<p>后文</p>')
  })

  it('复制按钮文案可由调用方本地化并转义', () => {
    const html = renderMarkdown('```ts\nconst a = 1\n```', { copyLabel: 'Copy "<x>"' })

    expect(html).toContain('>Copy &quot;&lt;x&gt;&quot;</button>')
    expect(html).not.toContain('<x>')
  })

  it('行内代码与加粗生效，内容仍被转义', () => {
    const html = renderMarkdown('用 `a < b` 判断 **重点**')

    expect(html).toContain('<code>a &lt; b</code>')
    expect(html).toContain('<strong>重点</strong>')
  })

  it('连续列表项合并为 ul，单换行保留为 br', () => {
    expect(renderMarkdown('- 一\n- 二')).toBe('<ul><li>一</li><li>二</li></ul>')
    expect(renderMarkdown('第一行\n第二行')).toBe('<p>第一行<br>第二行</p>')
  })

  it('空文本与未闭合围栏不会崩溃', () => {
    expect(renderMarkdown('')).toBe('')
    expect(renderMarkdown('```rust\n没写完')).toContain('```rust')
  })
})
