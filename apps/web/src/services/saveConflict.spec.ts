// issue #24：保存前外部修改冲突判定（纯函数）。
import { describe, expect, it } from 'vitest'

import {
  classifySaveConflict,
  conflictMessageKey,
  needsConfirmation,
} from './saveConflict'

describe('保存冲突判定（classifySaveConflict）', () => {
  it('磁盘内容与基线一致 → clean（直接写）', () => {
    expect(classifySaveConflict({ kind: 'content', content: 'a' }, 'a')).toBe('clean')
  })

  it('磁盘内容与基线不同 → changed（外部已修改）', () => {
    expect(classifySaveConflict({ kind: 'content', content: 'disk' }, 'baseline')).toBe('changed')
    // 空文件也是合法基线：基线 '' 而磁盘有内容 → changed。
    expect(classifySaveConflict({ kind: 'content', content: 'x' }, '')).toBe('changed')
  })

  it('文件已被删除 → deleted（保存即重建）', () => {
    expect(classifySaveConflict({ kind: 'error', error: { code: 'not_found' } }, 'a')).toBe(
      'deleted',
    )
  })

  it('其它读取失败 → read-failed（无法判定）', () => {
    expect(classifySaveConflict({ kind: 'error', error: { code: 'permission_denied' } }, 'a')).toBe(
      'read-failed',
    )
    expect(classifySaveConflict({ kind: 'error', error: { code: 'internal' } }, 'a')).toBe(
      'read-failed',
    )
  })

  it('浏览器 dev / fallback（snapshot=null）→ clean（不做判定）', () => {
    expect(classifySaveConflict(null, 'anything')).toBe('clean')
  })
})

describe('确认与文案 key', () => {
  it('除 clean 外都需要用户确认', () => {
    expect(needsConfirmation('clean')).toBe(false)
    for (const verdict of ['changed', 'deleted', 'read-failed'] as const) {
      expect(needsConfirmation(verdict)).toBe(true)
    }
  })

  it('文案 key 闭集映射（i18n 中英对称由 i18n.spec 把守）', () => {
    expect(conflictMessageKey('changed')).toBe('editor.conflictChanged')
    expect(conflictMessageKey('deleted')).toBe('editor.conflictDeleted')
    expect(conflictMessageKey('read-failed')).toBe('editor.conflictUnreadable')
    expect(conflictMessageKey('clean')).toBe('')
  })
})
