import { describe, expect, it } from 'vitest'

import {
  removeWorkspace,
  samePath,
  upsertWorkspace,
  workspaceName,
  type WorkspaceEntry,
} from './workspaceStore'

function entry(root: string, addedAt = 1): WorkspaceEntry {
  return { root, name: workspaceName(root), addedAt }
}

describe('Agent 工作区列表', () => {
  it('从路径取目录名，兼容 Windows 反斜杠', () => {
    expect(workspaceName('D:\\code\\lite-code')).toBe('lite-code')
    expect(workspaceName('/home/me/project')).toBe('project')
    expect(workspaceName('project')).toBe('project')
  })

  it('新增工作区并保持顺序；重复路径不新增', () => {
    const first = entry('D:/code/a', 1)
    const list = upsertWorkspace([], first.root, 1)
    expect(list).toEqual([first])

    const same = upsertWorkspace(list, 'D:\\code\\a', 2)
    expect(same).toHaveLength(1)
    expect(same[0]?.name).toBe('a')

    const next = upsertWorkspace(same, '/home/me/b', 3)
    expect(next.map(item => item.name)).toEqual(['a', 'b'])
  })

  it('路径比较忽略 Windows 盘符大小写，但 Linux 保留大小写', () => {
    expect(samePath('D:\\Code\\A', 'd:/code/a')).toBe(true)
    expect(samePath('/home/A', '/home/a')).toBe(false)
  })

  it('移除工作区只影响列表', () => {
    const list = [entry('D:/a'), entry('D:/b')]
    expect(removeWorkspace(list, 'd:\\A')).toEqual([entry('D:/b')])
  })
})
