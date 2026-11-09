// Agent 会话内查找：按消息文本/思考内容做大小写不敏感的子串匹配。
export interface FindableMessage {
  text?: string
  thinking?: string
}

export function findMessageMatches(
  messages: readonly FindableMessage[],
  query: string,
): number[] {
  const term = query.trim().toLocaleLowerCase()
  if (!term) return []
  const matches: number[] = []
  messages.forEach((message, index) => {
    const haystack = `${message.text ?? ''}\n${message.thinking ?? ''}`.toLocaleLowerCase()
    if (haystack.includes(term)) matches.push(index)
  })
  return matches
}

/** 在匹配列表中循环移动，空列表返回 0。 */
export function cycleFindIndex(current: number, length: number, direction: 1 | -1): number {
  if (length <= 0) return 0
  return (current + direction + length) % length
}
