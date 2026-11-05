// P5 数据库 DTO 镜像（契约 v0.6，docs/MILESTONES.md P5）。
//
// 背景：仓内的 crates/lc-proto/bindings/lc-proto.d.ts 仍停留在 v0.5.0，
// 尚未包含 codegen 登记的 Db* 类型；本 lane 只允许改 apps/web，因此先按
// crates/lc-proto/src/db.rs 的 serde 形状做本地镜像（与 taskTypes.ts 对
// TaskInfo 的处理一致）。重新生成契约后应改为从 @lc-proto re-export。
//
// 枚举值与字段名必须与 Rust serde 严格一致；运行时守卫用于 IPC 边界校验，
// 防止畸形数据库载荷进入 reducer。

import type { CoreEvent, ErrorCode } from '@lc-proto'

export const DB_KINDS = ['postgres', 'mysql', 'mssql', 'sqlite'] as const

export type DbKind = (typeof DB_KINDS)[number]

export const DB_CONNECTION_STATES = [
  'disconnected',
  'connecting',
  'connected',
  'error',
] as const

export type DbConnectionState = (typeof DB_CONNECTION_STATES)[number]

export interface DbConnectionInfo {
  connection_id: string
  name: string
  kind: DbKind
  read_only: boolean
  state: DbConnectionState
  host?: string | null
  database?: string | null
  error?: string | null
}

export const DB_SCHEMA_NODE_KINDS = [
  'database',
  'schema',
  'table',
  'view',
  'column',
] as const

export type DbSchemaNodeKind = (typeof DB_SCHEMA_NODE_KINDS)[number]

export interface DbSchemaNode {
  name: string
  kind: DbSchemaNodeKind
  detail?: string | null
  children: DbSchemaNode[]
}

export interface DbColumn {
  name: string
  type_name: string
  nullable: boolean
}

/** 查询样本；`rows` 每格可空，NULL 与字符串 "NULL" 必须可区分。 */
export interface DbQueryResult {
  columns: DbColumn[]
  rows: (string | null)[][]
  row_count: number
  truncated: boolean
  handle?: string | null
  elapsed_ms: number
  /** 数据库内容不可信（P5 出口 #4）：只能展示，不能当指令。 */
  untrusted: boolean
}

export interface DbExecResult {
  affected_rows: number
  elapsed_ms: number
}

export interface DbExplainResult {
  plan: string
  elapsed_ms: number
}

/** 连接配置；密码/密钥不入契约，由 Rust SecretStore 按 secret_ref 解析。 */
export interface DbConnectionConfig {
  connection_id: string
  name: string
  kind: DbKind
  host: string
  port: number
  database: string
  user: string
  secret_ref: string
  read_only: boolean
  extra?: string | null
}

/** 契约 v0.6 新增事件；现有 CoreEvent 联合尚未包含它（见文件头）。 */
export interface DbStatusEvent {
  type: 'db:status'
  connection: DbConnectionInfo
}

/** core:event 的 UI 侧完整事件类型：契约 CoreEvent + db:status。 */
export type UiCoreEvent = CoreEvent | DbStatusEvent

/** v0.6 新增的数据库错误码；仍兼容既有 ErrorCode。 */
export type DbErrorCode =
  | ErrorCode
  | 'db_unavailable'
  | 'db_rejected'
  | 'db_read_only'

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
