// 连接表单草稿与 DbConnectionConfig 的组装/反解纯函数（docs/plans/db-mysql-pg.md §4）。
// UI 不收集明文密码：密码框只收环境变量名，保存时拼 `env:` 前缀进 secret_ref，
// 编辑时反解回变量名（契约：secret_ref 为空串表示 SQLite 或无认证）。
//
// 注意：列表/编辑数据源 DbConnectionInfo 自契约 0.9 起携带 port/user/secretRef，
// draftFromInfo 优先采用；字段缺失（旧核心）时回落 kind 默认端口与空串。

import type { DbConnectionConfig, DbConnectionInfo, DbKind } from './dbTypes'

/** 远程 kind 的默认端口（mssql 与 pg/mysql 共用同一表单模板）。 */
export const REMOTE_PORT_DEFAULTS: Record<Exclude<DbKind, 'sqlite'>, number> = {
  postgres: 5432,
  mysql: 3306,
  mssql: 1433,
}

export interface ConnectionDraft {
  name: string
  kind: DbKind
  /** 端口在草稿里用字符串承载，空串表示"用 kind 默认端口"。 */
  port: string
  host: string
  database: string
  /** 可空：PG 回落 OS 用户。 */
  user: string
  /** 密码环境变量名（不含 env: 前缀），空串表示无密码。 */
  passwordEnv: string
  readOnly: boolean
}

export function isRemoteKind(kind: DbKind): boolean {
  return kind !== 'sqlite'
}

export function defaultPort(kind: DbKind): number {
  return kind === 'sqlite' ? 0 : REMOTE_PORT_DEFAULTS[kind]
}

export function createDraft(kind: DbKind = 'sqlite'): ConnectionDraft {
  return {
    name: '',
    kind,
    host: '',
    port: kind === 'sqlite' ? '' : String(REMOTE_PORT_DEFAULTS[kind]),
    database: '',
    user: '',
    passwordEnv: '',
    readOnly: false,
  }
}

const ENV_PREFIX = 'env:'

/** secret_ref → 环境变量名：`env:PGPASS` 反解为 `PGPASS`；空串/非 env: 引用返回空串。 */
export function secretRefToEnvName(secretRef: string): string {
  return secretRef.startsWith(ENV_PREFIX) ? secretRef.slice(ENV_PREFIX.length) : ''
}

/** 环境变量名 → secret_ref：空串表示无密码；其余一律拼 `env:` 前缀。 */
export function envNameToSecretRef(envName: string): string {
  const name = envName.trim()
  return name.length === 0 ? '' : `${ENV_PREFIX}${name}`
}

/** 解析端口输入：空串回落 kind 默认端口；SQLite 恒为 0；非法返回 null。 */
export function parsePort(input: string, kind: DbKind): number | null {
  if (kind === 'sqlite') return 0
  const trimmed = input.trim()
  if (trimmed.length === 0) return defaultPort(kind)
  if (!/^\d+$/.test(trimmed)) return null
  const port = Number(trimmed)
  if (!Number.isInteger(port) || port < 1 || port > 65535) return null
  return port
}

export type DraftErrorKey =
  | 'db.nameRequired'
  | 'db.pathRequired'
  | 'db.databaseRequired'
  | 'db.hostRequired'
  | 'db.portInvalid'

/** 表单校验：sqlite 数据库路径沿用 db.pathRequired；远程 kind 用 db.databaseRequired。 */
export function validateDraft(draft: ConnectionDraft): DraftErrorKey | null {
  if (draft.name.trim().length === 0) return 'db.nameRequired'
  if (isRemoteKind(draft.kind)) {
    if (draft.host.trim().length === 0) return 'db.hostRequired'
    if (parsePort(draft.port, draft.kind) === null) return 'db.portInvalid'
  }
  if (draft.database.trim().length === 0) {
    return isRemoteKind(draft.kind) ? 'db.databaseRequired' : 'db.pathRequired'
  }
  return null
}

/** 草稿 → 契约配置：远程字段只在远程 kind 下写入，sqlite 保持 host/port/user/secret_ref 为空。 */
export function buildConfig(
  draft: ConnectionDraft,
  connectionId: string,
): DbConnectionConfig {
  const remote = isRemoteKind(draft.kind)
  return {
    connection_id: connectionId,
    name: draft.name.trim(),
    kind: draft.kind,
    host: remote ? draft.host.trim() : '',
    port: remote ? (parsePort(draft.port, draft.kind) ?? 0) : 0,
    database: draft.database.trim(),
    user: remote ? draft.user.trim() : '',
    secret_ref: remote ? envNameToSecretRef(draft.passwordEnv) : '',
    read_only: draft.readOnly,
  }
}

/** 完整配置 → 草稿（编辑回填反解 secret_ref；调用方需持有 DbConnectionConfig）。 */
export function draftFromConfig(config: DbConnectionConfig): ConnectionDraft {
  const remote = isRemoteKind(config.kind)
  return {
    name: config.name,
    kind: config.kind,
    host: remote ? config.host : '',
    port: remote ? String(config.port > 0 ? config.port : defaultPort(config.kind)) : '',
    database: config.database,
    user: remote ? config.user : '',
    passwordEnv: remote ? secretRefToEnvName(config.secret_ref) : '',
    readOnly: config.read_only,
  }
}

/**
 * 列表数据源（DbConnectionInfo）→ 草稿。
 * 契约 0.9 起 info 携带 port/user/secretRef，优先采用；缺失/0/空（旧核心
 * 或 SQLite）时回落 kind 默认端口与空串，行为与 0.8 一致。
 */
export function draftFromInfo(connection: DbConnectionInfo): ConnectionDraft {
  const remote = isRemoteKind(connection.kind)
  const port =
    typeof connection.port === 'number' && connection.port > 0
      ? String(connection.port)
      : String(remote ? defaultPort(connection.kind) : 0)
  return {
    name: connection.name,
    kind: connection.kind,
    host: connection.host ?? '',
    port: remote ? port : '',
    database: connection.database ?? '',
    user: remote ? (connection.user ?? '') : '',
    passwordEnv: remote ? secretRefToEnvName(connection.secretRef ?? '') : '',
    readOnly: connection.read_only,
  }
}

/**
 * 远程连接列表摘要：`host:port/database`。
 * DbConnectionInfo 不带 port 时退化为 `host/database`；无 host（sqlite）返回 null。
 */
export function remoteSummary(target: {
  host?: string | null
  port?: number | null
  database?: string | null
}): string | null {
  const host = target.host?.trim() ?? ''
  if (host.length === 0) return null
  const port = typeof target.port === 'number' && target.port > 0 ? `:${target.port}` : ''
  const database = target.database?.trim() ?? ''
  return database.length > 0 ? `${host}${port}/${database}` : `${host}${port}`
}
