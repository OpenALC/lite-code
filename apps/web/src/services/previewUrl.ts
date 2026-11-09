// 预览地址校验：只允许 http(s)，缺协议时补 http://。
export interface PreviewUrlResult {
  url?: string
  error?: string
}

export function normalizePreviewUrl(raw: string): PreviewUrlResult {
  const value = raw.trim()
  if (!value) return { error: 'empty' }
  const hostPort = /^[^\s/:]+:\d+(?:[/?#].*)?$/.test(value)
  const hasScheme =
    /^[a-z][a-z0-9+.-]*:/i.test(value) && !hostPort
  const withScheme = hasScheme ? value : `http://${value}`
  let parsed: URL
  try {
    parsed = new URL(withScheme)
  } catch {
    return { error: 'invalid' }
  }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    return { error: 'scheme' }
  }
  return { url: parsed.toString() }
}
