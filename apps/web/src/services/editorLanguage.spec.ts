import { afterEach, describe, expect, it } from 'vitest'

import { languageForPath, setPluginLanguages } from './editorLanguage'

afterEach(() => {
  setPluginLanguages([])
})

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

describe('languageForPath 插件语言（docs/EXTENSIONS.md §1）', () => {
  it('插件扩展名并入检测；大小写不敏感', () => {
    setPluginLanguages([
      { id: 'foo-lang', extensions: ['.foo', '.Foo'] },
      { id: 'bar', extensions: ['.bar'] },
    ])
    expect(languageForPath('D:/p/a.foo')).toBe('foo-lang')
    expect(languageForPath('D:/p/a.FOO')).toBe('foo-lang')
    expect(languageForPath('/home/u/b.bar')).toBe('bar')
    // 未声明的扩展名仍回退 plaintext。
    expect(languageForPath('D:/p/a.baz')).toBe('plaintext')
  })

  it('内置优先：插件声明的 .rs 不覆盖内置映射', () => {
    setPluginLanguages([
      { id: 'mine', extensions: ['.rs', '.mine'] },
    ])
    expect(languageForPath('D:/p/main.rs')).toBe('rust')
    expect(languageForPath('D:/p/a.mine')).toBe('mine')
  })

  it('插件之间先声明者胜出（Rust 校验去重的兜底）', () => {
    setPluginLanguages([
      { id: 'first', extensions: ['.x'] },
      { id: 'second', extensions: ['.x'] },
    ])
    expect(languageForPath('D:/p/a.x')).toBe('first')
  })

  it('setPluginLanguages 全量替换：清空后插件扩展名回落 plaintext', () => {
    setPluginLanguages([{ id: 'foo-lang', extensions: ['.foo'] }])
    expect(languageForPath('D:/p/a.foo')).toBe('foo-lang')
    setPluginLanguages([])
    expect(languageForPath('D:/p/a.foo')).toBe('plaintext')
  })
})
