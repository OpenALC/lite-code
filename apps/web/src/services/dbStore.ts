// 数据库面板状态、reducer 与控制器（docs/MILESTONES.md P5）。
// 框架无关：不可变 reducer + 依赖注入的 bridge 控制器（UI.md §8）。
//
// 事件来源是 core:event 的 db:status；命令结果用于更新连接列表与查询结果。
// 数据库内容不可信，UI 只做展示（untrusted 提示条由视图层渲染）。

import type {
  AckResult,
  BridgeResult,
  DbConnectionResult,
  DbExecCommandResult,
  DbExplainCommandResult,
  DbListResult,
  DbQueryCommandResult,
  DbSchemaResult,
} from './bridge'
import {
  isDbStatusEvent,
  type DbConnectionConfig,
  type DbConnectionInfo,
  type DbErrorCode,
  type DbExplainResult,
  type DbExecResult,
  type DbKind,
  type DbQueryResult,
  type DbSchemaNode,
  type DbConnectionState,
  type UiCoreEvent,
} from './dbTypes'

export type Translate = (key: string, params?: Record<string, unknown>) => string

export interface DbState {
  connections: DbConnectionInfo[]
  activeConnectionId: string | null
  schemaNodes: DbSchemaNode[]
  sql: string
  result: DbQueryResult | null
  execResult: DbExecResult | null
  explainResult: DbExplainResult | null
  loading: boolean
  error: { code: DbErrorCode; message: string } | null
  readOnly: boolean
}

export function createDbState(): DbState {
  return {
    connections: [],
    activeConnectionId: null,
    schemaNodes: [],
    sql: '',
    result: null,
    execResult: null,
    explainResult: null,
    loading: false,
    error: null,
    readOnly: false,
  }
}

/** 连接显示名：名称 → database → connection_id，始终返回非空串。 */
export function connectionLabel(connection: DbConnectionInfo): string {
  const name = connection.name.trim()
  if (name.length > 0) return name
  const database = connection.database?.trim() ?? ''
  if (database.length > 0) return database
  return connection.connection_id
}

export function dbKindLabel(kind: DbKind, t: Translate): string {
  return t(`db.kinds.${kind}`)
}

export function connectionStateLabel(state: DbConnectionState, t: Translate): string {
  return t(`db.states.${state}`)
}

/** 连接按钮是否可用：当前只实现了 SQLite；connecting 时禁止重复点击。 */
export function canConnect(connection: DbConnectionInfo): boolean {
  return connection.kind === 'sqlite' && connection.state !== 'connecting'
}

export function activeConnection(state: DbState): DbConnectionInfo | null {
  if (state.activeConnectionId === null) return null
  return (
    state.connections.find(
      connection => connection.connection_id === state.activeConnectionId,
    ) ?? null
  )
}

export interface DbResultSummary {
  /** 服务端报告的逻辑行数（不是样本长度）。 */
  rows: number
  /** 本次实际下发的样本行数。 */
  sampleRows: number
  columns: number
  elapsedMs: number
  truncated: boolean
}

export function resultSummary(result: DbQueryResult | null): DbResultSummary {
  if (!result) {
    return { rows: 0, sampleRows: 0, columns: 0, elapsedMs: 0, truncated: false }
  }
  return {
    rows: result.row_count,
    sampleRows: result.rows.length,
    columns: result.columns.length,
    elapsedMs: result.elapsed_ms,
    truncated: result.truncated,
  }
}

/** 数据库内容是否不可信；只有明确的 untrusted=true 才显示警示条。 */
export function isUntrusted(result: DbQueryResult | null): boolean {
  return result !== null && result.untrusted === true
}

/** 点击 schema 表/视图时插入的查询语句。 */
export function selectSqlForTable(path: readonly string[]): string {
  const parts = path.map(part => part.trim()).filter(part => part.length > 0)
  if (parts.length === 0) return ''
  return `SELECT * FROM ${parts.join('.')}`
}

/** 保留编辑器里已有 SQL，把新查询追加到末尾。 */
export function sqlWithTableQuery(
  current: string,
  path: readonly string[],
): string {
  const statement = selectSqlForTable(path)
  if (statement.length === 0) return current
  if (current.trim().length === 0) return statement
  return `${current.replace(/\s+$/, '')}\n${statement}`
}

/** 生成不与现有连接冲突的稳定前缀 id（id 由前端生成，避免保存前为空）。 */
export function nextConnectionId(
  connections: readonly DbConnectionInfo[],
  now: number = Date.now(),
): string {
  const stamp = Math.max(0, Math.trunc(now)).toString(36)
  const base = `conn-${stamp}`
  const used = new Set(connections.map(connection => connection.connection_id))
  if (!used.has(base)) return base
  let seq = 2
  while (used.has(`${base}-${seq}`)) seq += 1
  return `${base}-${seq}`
}

function readOnlyOf(
  connections: readonly DbConnectionInfo[],
  activeConnectionId: string | null,
): boolean {
  if (activeConnectionId === null) return false
  return (
    connections.find(
      connection => connection.connection_id === activeConnectionId,
    )?.read_only === true
  )
}

function clearResults(): Pick<
  DbState,
  'schemaNodes' | 'result' | 'execResult' | 'explainResult'
> {
  return {
    schemaNodes: [],
    result: null,
    execResult: null,
    explainResult: null,
  }
}

/** 连接列表刷新：保留仍存在的选中连接，否则回落到第一条并清空旧结果。 */
export function dbConnectionsLoaded(
  state: DbState,
  connections: DbConnectionInfo[],
): DbState {
  const ids = new Set(connections.map(connection => connection.connection_id))
  const activeConnectionId =
    state.activeConnectionId !== null && ids.has(state.activeConnectionId)
      ? state.activeConnectionId
      : (connections[0]?.connection_id ?? null)
  const activeChanged = activeConnectionId !== state.activeConnectionId
  return {
    ...state,
    connections,
    activeConnectionId,
    readOnly: readOnlyOf(connections, activeConnectionId),
    ...(activeChanged ? clearResults() : {}),
  }
}

export function dbConnectionSelected(
  state: DbState,
  connectionId: string | null,
): DbState {
  if (state.activeConnectionId === connectionId) return state
  const target =
    connectionId === null
      ? null
      : (state.connections.find(
          connection => connection.connection_id === connectionId,
        ) ?? null)
  return {
    ...state,
    activeConnectionId: target?.connection_id ?? null,
    readOnly: target?.read_only === true,
    ...clearResults(),
  }
}

/** schema 只接受当前选中连接的结果，避免快速切换时旧响应覆盖新连接。 */
export function dbSchemaLoaded(
  state: DbState,
  connectionId: string,
  nodes: DbSchemaNode[],
): DbState {
  if (state.activeConnectionId !== connectionId) return state
  return { ...state, schemaNodes: nodes }
}

export function dbSqlChanged(state: DbState, sql: string): DbState {
  if (state.sql === sql) return state
  return { ...state, sql }
}

export function dbBusy(state: DbState, loading: boolean): DbState {
  if (state.loading === loading) return state
  return { ...state, loading }
}

export function dbFailed(
  state: DbState,
  error: { code: DbErrorCode; message: string },
): DbState {
  return { ...state, loading: false, error: { code: error.code, message: error.message } }
}

export function dbQuerySucceeded(state: DbState, result: DbQueryResult): DbState {
  return {
    ...state,
    loading: false,
    error: null,
    result,
    execResult: null,
    explainResult: null,
  }
}

export function dbExecuteSucceeded(state: DbState, result: DbExecResult): DbState {
  return {
    ...state,
    loading: false,
    error: null,
    result: null,
    execResult: result,
    explainResult: null,
  }
}

export function dbExplainSucceeded(state: DbState, result: DbExplainResult): DbState {
  return {
    ...state,
    loading: false,
    error: null,
    result: null,
    execResult: null,
    explainResult: result,
  }
}

/** 命令结果 / db:status 事件共用的连接 upsert。 */
export function dbConnectionUpserted(
  state: DbState,
  connection: DbConnectionInfo,
): DbState {
  const index = state.connections.findIndex(
    item => item.connection_id === connection.connection_id,
  )
  const connections =
    index < 0
      ? [...state.connections, connection]
      : state.connections.map((item, i) => (i === index ? connection : item))
  const activeConnectionId = state.activeConnectionId ?? connection.connection_id
  const wasActive = state.activeConnectionId === connection.connection_id
  const shouldClear = wasActive && connection.state !== 'connected'
  return {
    ...state,
    connections,
    activeConnectionId,
    readOnly: readOnlyOf(connections, activeConnectionId),
    ...(shouldClear ? clearResults() : {}),
  }
}

export function dbConnectionDeleted(
  state: DbState,
  connectionId: string,
): DbState {
  const connections = state.connections.filter(
    connection => connection.connection_id !== connectionId,
  )
  if (connections.length === state.connections.length) return state
  if (state.activeConnectionId !== connectionId) {
    return { ...state, connections }
  }
  const activeConnectionId = connections[0]?.connection_id ?? null
  return {
    ...state,
    connections,
    activeConnectionId,
    readOnly: readOnlyOf(connections, activeConnectionId),
    ...clearResults(),
  }
}

/** 应用一条 core 事件；非 db:status 事件原样返回，便于多 reducer 共用事件流。 */
export function applyDbEvent(state: DbState, event: UiCoreEvent): DbState {
  if (!isDbStatusEvent(event)) return state
  return dbConnectionUpserted(state, event.connection)
}

// ==== 控制器：命令 + 事件流 → store ====

export interface DbControllerDeps {
  dbList: () => Promise<BridgeResult<DbListResult>>
  dbConnect: (connectionId: string) => Promise<BridgeResult<DbConnectionResult>>
  dbDisconnect: (connectionId: string) => Promise<BridgeResult<DbConnectionResult>>
  dbSchema: (connectionId: string) => Promise<BridgeResult<DbSchemaResult>>
  dbQuery: (
    connectionId: string,
    sql: string,
    limit?: number,
  ) => Promise<BridgeResult<DbQueryCommandResult>>
  dbExecute: (
    connectionId: string,
    sql: string,
  ) => Promise<BridgeResult<DbExecCommandResult>>
  dbExplain: (
    connectionId: string,
    sql: string,
  ) => Promise<BridgeResult<DbExplainCommandResult>>
  dbSaveConnection: (
    connection: DbConnectionConfig,
  ) => Promise<BridgeResult<DbConnectionResult>>
  dbDeleteConnection: (connectionId: string) => Promise<BridgeResult<AckResult>>
  onCoreEvent: (handler: (event: UiCoreEvent) => void) => () => void
}

export interface DbController {
  getState(): DbState
  subscribe(listener: (state: DbState) => void): () => void
  refresh(): Promise<boolean>
  /** 选中连接；已连接时惰性加载 schema。 */
  select(connectionId: string | null): Promise<void>
  connect(connectionId?: string): Promise<boolean>
  disconnect(connectionId?: string): Promise<boolean>
  loadSchema(connectionId?: string): Promise<boolean>
  setSql(sql: string): void
  query(limit?: number): Promise<boolean>
  execute(): Promise<boolean>
  explain(): Promise<boolean>
  saveConnection(connection: DbConnectionConfig): Promise<DbConnectionInfo | null>
  deleteConnection(connectionId: string): Promise<boolean>
  clearError(): void
  dispose(): void
}

export function createDbController(deps: DbControllerDeps): DbController {
  let state = createDbState()
  const listeners = new Set<(state: DbState) => void>()
  let disposed = false

  function commit(next: DbState): void {
    state = next
    if (disposed) return
    for (const listener of [...listeners]) listener(state)
  }

  const offCore = deps.onCoreEvent(event => {
    if (disposed) return
    const next = applyDbEvent(state, event)
    if (next !== state) commit(next)
  })

  function current(): DbConnectionInfo | null {
    return activeConnection(state)
  }

  function connectedTarget(): DbConnectionInfo | null {
    const connection = current()
    return connection?.state === 'connected' ? connection : null
  }

  async function refresh(): Promise<boolean> {
    if (disposed) return false
    const res = await deps.dbList()
    if (disposed) return false
    if (!res.ok) {
      commit(dbFailed(state, res.error))
      return false
    }
    commit({ ...dbConnectionsLoaded(state, res.data.connections), error: null })
    return true
  }

  async function select(connectionId: string | null): Promise<void> {
    if (disposed) return
    const next = dbConnectionSelected(state, connectionId)
    if (next !== state) commit({ ...next, error: null })
    const connection = current()
    if (
      connection?.state === 'connected' &&
      state.schemaNodes.length === 0
    ) {
      await loadSchema(connection.connection_id)
    }
  }

  async function loadSchema(connectionId?: string): Promise<boolean> {
    if (disposed) return false
    const target = connectionId ?? current()?.connection_id ?? null
    if (target === null) return false
    if (!state.connections.some(connection => connection.connection_id === target)) {
      return false
    }
    commit(dbBusy(state, true))
    const res = await deps.dbSchema(target)
    if (disposed) return false
    if (!res.ok) {
      commit(dbFailed(state, res.error))
      return false
    }
    commit({
      ...dbBusy(dbSchemaLoaded(state, target, res.data.nodes), false),
      error: null,
    })
    return true
  }

  async function connect(connectionId?: string): Promise<boolean> {
    if (disposed) return false
    const target = connectionId ?? current()?.connection_id ?? null
    if (target === null) return false
    commit({ ...dbBusy(state, true), error: null })
    const res = await deps.dbConnect(target)
    if (disposed) return false
    if (!res.ok) {
      commit(dbFailed(state, res.error))
      return false
    }
    commit(dbConnectionUpserted(dbBusy(state, false), res.data.connection))
    if (res.data.connection.state === 'connected') await loadSchema(target)
    return true
  }

  async function disconnect(connectionId?: string): Promise<boolean> {
    if (disposed) return false
    const target = connectionId ?? current()?.connection_id ?? null
    if (target === null) return false
    commit({ ...dbBusy(state, true), error: null })
    const res = await deps.dbDisconnect(target)
    if (disposed) return false
    if (!res.ok) {
      commit(dbFailed(state, res.error))
      return false
    }
    commit(dbConnectionUpserted(dbBusy(state, false), res.data.connection))
    return true
  }

  function setSql(sql: string): void {
    if (disposed) return
    const next = dbSqlChanged(state, sql)
    if (next !== state) commit(next)
  }

  async function query(limit?: number): Promise<boolean> {
    if (disposed) return false
    const connection = connectedTarget()
    const sql = state.sql.trim()
    if (!connection || sql.length === 0) return false
    commit({ ...dbBusy(state, true), error: null })
    const res = await deps.dbQuery(connection.connection_id, sql, limit)
    if (disposed) return false
    if (!res.ok) {
      commit(dbFailed(state, res.error))
      return false
    }
    commit(dbQuerySucceeded(state, res.data.result))
    return true
  }

  async function execute(): Promise<boolean> {
    if (disposed) return false
    const connection = connectedTarget()
    const sql = state.sql.trim()
    if (!connection || sql.length === 0) return false
    if (state.readOnly) {
      commit(
        dbFailed(state, {
          code: 'db_read_only',
          message: 'read-only connection: execute is disabled',
        }),
      )
      return false
    }
    commit({ ...dbBusy(state, true), error: null })
    const res = await deps.dbExecute(connection.connection_id, sql)
    if (disposed) return false
    if (!res.ok) {
      commit(dbFailed(state, res.error))
      return false
    }
    commit(dbExecuteSucceeded(state, res.data.result))
    return true
  }

  async function explain(): Promise<boolean> {
    if (disposed) return false
    const connection = connectedTarget()
    const sql = state.sql.trim()
    if (!connection || sql.length === 0) return false
    commit({ ...dbBusy(state, true), error: null })
    const res = await deps.dbExplain(connection.connection_id, sql)
    if (disposed) return false
    if (!res.ok) {
      commit(dbFailed(state, res.error))
      return false
    }
    commit(dbExplainSucceeded(state, res.data.result))
    return true
  }

  async function saveConnection(
    connection: DbConnectionConfig,
  ): Promise<DbConnectionInfo | null> {
    if (disposed) return null
    const existed = state.connections.some(
      item => item.connection_id === connection.connection_id,
    )
    const res = await deps.dbSaveConnection(connection)
    if (disposed) return null
    if (!res.ok) {
      commit(dbFailed(state, res.error))
      return null
    }
    const saved = res.data.connection
    let next = dbConnectionUpserted(state, saved)
    if (!existed || state.activeConnectionId === saved.connection_id) {
      next = dbConnectionSelected(next, saved.connection_id)
    }
    commit({ ...next, error: null })
    return saved
  }

  async function deleteConnection(connectionId: string): Promise<boolean> {
    if (disposed) return false
    const res = await deps.dbDeleteConnection(connectionId)
    if (disposed) return false
    if (!res.ok) {
      commit(dbFailed(state, res.error))
      return false
    }
    commit({ ...dbConnectionDeleted(state, connectionId), error: null })
    return true
  }

  function clearError(): void {
    if (disposed || state.error === null) return
    commit({ ...state, error: null })
  }

  function getState(): DbState {
    return state
  }

  function subscribe(listener: (state: DbState) => void): () => void {
    listeners.add(listener)
    return () => {
      listeners.delete(listener)
    }
  }

  function dispose(): void {
    if (disposed) return
    disposed = true
    offCore()
    listeners.clear()
  }

  return {
    getState,
    subscribe,
    refresh,
    select,
    connect,
    disconnect,
    loadSchema,
    setSql,
    query,
    execute,
    explain,
    saveConnection,
    deleteConnection,
    clearError,
    dispose,
  }
}
