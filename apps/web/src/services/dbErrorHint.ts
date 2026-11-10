// 数据库连接失败的错误分流提示（docs/plans/db-mysql-pg.md §3.3 / §6 M3）。
// 纯函数：错误 → i18n 提示键；UI 只负责 t(key) 渲染，不在这里碰 i18n 实例。
//
// 分流表：
// - db_unavailable + Node 缺失 → db.errorHint.nodeRuntime（引导 LC 运行时管理）
// - db_unavailable 其它（网络/认证/驱动）→ db.errorHint.connection（提示检查
//   host/port/凭证环境变量；驱动原文已脱敏，随错误消息原样展示）
// - db_rejected / db_read_only → null（现有展示即可）
//
// Node 缺失判别：Rust 侧固定报 db_unavailable 且消息含 "Node"
// （lc-core service.rs ensure_sidecar："Node 运行时不可用（未探测到可用的 node）"，
// 桌面侧单测断言该标记）；驱动/网络/认证类错误消息不含 "Node"。

import type { DbErrorCode } from './dbTypes'

export type DbConnectErrorHintKey =
  | 'db.errorHint.nodeRuntime'
  | 'db.errorHint.connection'

export interface ConnectError {
  code: DbErrorCode
  message: string
}

/** 连接错误 → 分流提示键；null 表示无需分流提示（维持现有错误展示）。 */
export function connectErrorHintKey(
  error: ConnectError,
): DbConnectErrorHintKey | null {
  if (error.code !== 'db_unavailable') return null
  return error.message.includes('Node')
    ? 'db.errorHint.nodeRuntime'
    : 'db.errorHint.connection'
}
