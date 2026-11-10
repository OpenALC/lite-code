// P5 数据库 DTO 与运行时守卫（契约 v0.9，docs/MILESTONES.md P5）。
//
// 类型单一来源：@lc-proto（crates/lc-proto codegen 产物）。本文件只保留
// 枚举数组与跨 IPC 边界的形状校验；字段名与 Rust serde 严格一致。

import type {
  CoreEvent,
  DbColumn,
  DbConnectionConfig,
  DbConnectionInfo,
  DbConnectionState,
  DbExecResult,
  DbExplainResult,
  DbKind,
  DbQueryResult,
  DbSchemaNode,
  DbSchemaNodeKind,
  ErrorCode,
} from '@lc-proto'

export type {
  DbColumn,
  DbConnectionConfig,
  DbConnectionInfo,
  DbConnectionState,
  DbExecResult,
  DbExplainResult,
  DbKind,
  DbQueryResult,
  DbSchemaNode,
  DbSchemaNodeKind,
}

export const DB_KINDS = ['postgres', 'mysql', 'mssql', 'sqlite'] as const

export const DB_CONNECTION_STATES = [
  'disconnected',
  'connecting',
  'connected',
  'error',
] as const

export const DB_SCHEMA_NODE_KINDS = [
  'database',
  'schema',
  'table',
  'view',
  'column',
] as const

/** 契约 v0.6 的 db:status 事件（CoreEvent 联合已包含）。 */
export type DbStatusEvent = Extract<CoreEvent, { type: 'db:status' }>

/** core:event 的 UI 侧完整事件类型。 */
export type UiCoreEvent = CoreEvent

/** 数据库错误码（ErrorCode 联合已包含 db_*）。 */
export type DbErrorCode = ErrorCode

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function isString(value: unknown): value is string {
  return typeof value === 'string'
}

function isNonNegativeInt(value: unknown): value is number {
  return (
    typeof value === 'number' &&
    Number.isFinite(value) &&
    Number.isInteger(value) &&
    value >= 0
  )
}

/** Rust Option<String> 可能省略字段，也可能序列化为 null。 */
function isOptionalString(value: unknown): value is string | null | undefined {
  return value === null || value === undefined || typeof value === 'string'
}

/** 契约 0.9 起 port 随 info 下发（u32，0 缺省省略）；旧核心可能整体缺省。 */
function isOptionalNonNegativeInt(
  value: unknown,
): value is number | null | undefined {
  return value === null || value === undefined || isNonNegativeInt(value)
}

export function isDbKind(value: unknown): value is DbKind {
  return typeof value === 'string' && (DB_KINDS as readonly string[]).includes(value)
}

export function isDbConnectionState(value: unknown): value is DbConnectionState {
  return (
    typeof value === 'string' &&
    (DB_CONNECTION_STATES as readonly string[]).includes(value)
  )
}

export function isDbSchemaNodeKind(value: unknown): value is DbSchemaNodeKind {
  return (
    typeof value === 'string' &&
    (DB_SCHEMA_NODE_KINDS as readonly string[]).includes(value)
  )
}

export function isDbConnectionInfo(value: unknown): value is DbConnectionInfo {
  if (!isRecord(value)) return false
  return (
    isString(value.connection_id) &&
    isString(value.name) &&
    isDbKind(value.kind) &&
    typeof value.read_only === 'boolean' &&
    isDbConnectionState(value.state) &&
    isOptionalString(value.host) &&
    isOptionalNonNegativeInt(value.port) &&
    isOptionalString(value.user) &&
    isOptionalString(value.secretRef) &&
    isOptionalString(value.database) &&
    isOptionalString(value.error)
  )
}

export function isDbColumn(value: unknown): value is DbColumn {
  if (!isRecord(value)) return false
  return (
    isString(value.name) &&
    isString(value.type_name) &&
    typeof value.nullable === 'boolean'
  )
}

export function isDbSchemaNode(value: unknown): value is DbSchemaNode {
  if (!isRecord(value)) return false
  return (
    isString(value.name) &&
    isDbSchemaNodeKind(value.kind) &&
    isOptionalString(value.detail) &&
    Array.isArray(value.children) &&
    value.children.every(isDbSchemaNode)
  )
}

export function isDbQueryResult(value: unknown): value is DbQueryResult {
  if (!isRecord(value)) return false
  const rows = value.rows
  return (
    Array.isArray(value.columns) &&
    value.columns.every(isDbColumn) &&
    Array.isArray(rows) &&
    rows.every(
      row =>
        Array.isArray(row) &&
        row.every(cell => cell === null || typeof cell === 'string'),
    ) &&
    isNonNegativeInt(value.row_count) &&
    typeof value.truncated === 'boolean' &&
    isOptionalString(value.handle) &&
    isNonNegativeInt(value.elapsed_ms) &&
    typeof value.untrusted === 'boolean'
  )
}

export function isDbExecResult(value: unknown): value is DbExecResult {
  if (!isRecord(value)) return false
  return (
    isNonNegativeInt(value.affected_rows) &&
    isNonNegativeInt(value.elapsed_ms)
  )
}

export function isDbExplainResult(value: unknown): value is DbExplainResult {
  if (!isRecord(value)) return false
  return isString(value.plan) && isNonNegativeInt(value.elapsed_ms)
}

export function isDbConnectionConfig(value: unknown): value is DbConnectionConfig {
  if (!isRecord(value)) return false
  return (
    isString(value.connection_id) &&
    isString(value.name) &&
    isDbKind(value.kind) &&
    isString(value.host) &&
    typeof value.port === 'number' &&
    Number.isInteger(value.port) &&
    value.port >= 0 &&
    value.port <= 65535 &&
    isString(value.database) &&
    isString(value.user) &&
    isString(value.secret_ref) &&
    typeof value.read_only === 'boolean' &&
    isOptionalString(value.extra)
  )
}

export function isDbStatusEvent(value: unknown): value is DbStatusEvent {
  return (
    isRecord(value) &&
    value.type === 'db:status' &&
    isDbConnectionInfo(value.connection)
  )
}
