import { describe, expect, it } from 'vitest'

import { languageForPath } from './editorLanguage'

describe('languageForPath', () => {
  it('按扩展名映射常见语言', () => {
    expect(languageForPath('D:/p/src/main.rs')).toBe('rust')
    expect(languageForPath('D:/p/app.ts')).toBe('typescript')
    expect(languageForPath('D:/p/data.JSON')).toBe('json')
    expect(languageForPath('/home/u/a.md')).toBe('markdown')
  })

  it('无扩展名 / 隐藏文件 / 未知扩展名回退 plaintext', () => {
    expect(languageForPath('D:/p/Makefile')).toBe('plaintext')
    expect(languageForPath('D:/p/.gitignore')).toBe('plaintext')
    expect(languageForPath('D:/p/a.unknown')).toBe('plaintext')
    expect(languageForPath('D:/p/trailing.')).toBe('plaintext')
    expect(languageForPath(null)).toBe('plaintext')
  })
})
