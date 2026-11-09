import { describe, expect, it } from 'vitest'

import {
  isSkippedDir,
  relativePath,
  searchFiles,
  type FileIndex,
} from './fileIndex'

function index(files: string[]): FileIndex {
  return { root: 'D:/code/project', files, truncated: false }
}

describe('工作区文件索引', () => {
  it('计算相对路径，兼容 Windows 分隔符', () => {
    expect(relativePath('D:\\code\\project', 'D:\\code\\project\\src\\main.rs')).toBe(
      'src/main.rs',
    )
    expect(relativePath('/home/me/project', '/home/me/project/src/a.ts')).toBe('src/a.ts')
    expect(relativePath('D:/code/project', 'D:/other/file')).toBe('D:/other/file')
  })

  it('跳过大目录与构建产物', () => {
    expect(isSkippedDir('node_modules')).toBe(true)
    expect(isSkippedDir('target')).toBe(true)
    expect(isSkippedDir('vendor')).toBe(true)
    expect(isSkippedDir('.claude')).toBe(true)
    expect(isSkippedDir('.github')).toBe(false)
    expect(isSkippedDir('src')).toBe(false)
  })

  it('空查询返回排序后的前几条', () => {
    expect(searchFiles(index(['a.ts', 'b.ts']), '', 1)).toEqual(['a.ts'])
  })

  it('优先匹配文件名前缀，其次文件名包含，最后路径包含', () => {
    const files = ['src/session-helper.ts', 'src/agent/session.ts', 'docs/other-session.md']
    expect(searchFiles(index(files), 'session')).toEqual([
      'src/agent/session.ts',
      'src/session-helper.ts',
      'docs/other-session.md',
    ])
  })

  it('限制结果数量并忽略大小写', () => {
    const files = ['README.md', 'src/readme-helper.ts', 'docs/other.md']
    expect(searchFiles(index(files), 'README', 1)).toEqual(['README.md'])
  })
})
