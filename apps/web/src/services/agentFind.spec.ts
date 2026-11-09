import { describe, expect, it } from 'vitest'

import { cycleFindIndex, findMessageMatches } from './agentFind'

describe('Agent 会话内查找', () => {
  it('匹配消息正文与思考内容，忽略大小写', () => {
    const messages = [
      { text: '修复 Login 问题' },
      { thinking: '先看 login 流程' },
      { text: '无关内容' },
    ]

    expect(findMessageMatches(messages, 'login')).toEqual([0, 1])
    expect(findMessageMatches(messages, '  LOGIN ')).toEqual([0, 1])
    expect(findMessageMatches(messages, '无关')).toEqual([2])
  })

  it('空查询与全空白查询返回空列表', () => {
    expect(findMessageMatches([{ text: 'a' }], '')).toEqual([])
    expect(findMessageMatches([{ text: 'a' }], '   ')).toEqual([])
  })

  it('循环索引在边界回绕，空列表归零', () => {
    expect(cycleFindIndex(0, 3, 1)).toBe(1)
    expect(cycleFindIndex(2, 3, 1)).toBe(0)
    expect(cycleFindIndex(0, 3, -1)).toBe(2)
    expect(cycleFindIndex(1, 0, 1)).toBe(0)
  })
})
