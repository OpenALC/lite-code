// 保存前的外部修改冲突判定（issue #24，docs/UI.md 编辑器语义）。
//
// 数据流：CodeEditor.saveActive() 在写盘前用 bridge.readFile 读一次磁盘，
// 与标签的 savedContent（最近一次落盘/打开时的内容基线）比对：
//   - disk 与基线一致 → clean，直接写；
//   - disk 不一致      → changed，弹「文件已在磁盘上被修改」三选（覆盖/重新加载/取消）；
//   - 文件已被删除     → deleted，保存即重建（提示后允许）；
//   - 其它读取失败     → read-failed，无法判定，交由用户决定。
// 浏览器 dev / fallback 场景不做判定（readFile 返回占位内容，不能当真相）。
//
// 本模块只做纯判定，便于单测；副作用（弹层/写盘）留在组件层。

/** 磁盘读取失败的错误码（契约 ErrorCode 的子集语义，只用到 not_found）。 */
export interface DiskReadError {
  code: string
}

/** 保存前对磁盘状态的分类。 */
export type SaveConflictVerdict = 'clean' | 'changed' | 'deleted' | 'read-failed'

/** 磁盘读取快照：成功带 content；失败带错误码。 */
export type DiskSnapshot =
  | { kind: 'content'; content: string }
  | { kind: 'error'; error: DiskReadError }

/**
 * 判定保存是否可以静默进行。
 * @param snapshot 写盘前读取的磁盘快照；null 表示浏览器 dev（不做判定）。
 * @param baseline 标签的 savedContent（内容基线）。
 */
export function classifySaveConflict(
  snapshot: DiskSnapshot | null,
  baseline: string,
): SaveConflictVerdict {
  if (snapshot === null) return 'clean'
  if (snapshot.kind === 'content') {
    return snapshot.content === baseline ? 'clean' : 'changed'
  }
  if (snapshot.error.code === 'not_found') return 'deleted'
  return 'read-failed'
}

/** 需要弹冲突确认的判定（clean 之外都要用户拍板，deleted 允许「保存即重建」）。 */
export function needsConfirmation(verdict: SaveConflictVerdict): boolean {
  return verdict !== 'clean'
}

/** 冲突提示对应的 i18n key 前缀（ui 层拼文案，纯函数留测试入口）。 */
export function conflictMessageKey(verdict: SaveConflictVerdict): string {
  switch (verdict) {
    case 'changed':
      return 'editor.conflictChanged'
    case 'deleted':
      return 'editor.conflictDeleted'
    case 'read-failed':
      return 'editor.conflictUnreadable'
    default:
      return ''
  }
}
