import { describe, expect, it } from 'vitest'

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
  activeConnection,
  applyDbEvent,
  canConnect,
  connectionLabel,
  connectionStateLabel,
  createDbController,
  createDbState,
  dbBusy,
  dbConnectionDeleted,
  dbConnectionSelected,
  dbConnectionUpserted,
  dbConnectionsLoaded,
  dbExecuteSucceeded,
  dbExplainSucceeded,
  dbFailed,
  dbKindLabel,
  dbQuerySucceeded,
  dbSchemaLoaded,
  dbSqlChanged,
  isUntrusted,
  nextConnectionId,
  resultSummary,
  selectSqlForTable,
  sqlWithTableQuery,
  type DbControllerDeps,
  type DbState,
  type Translate,
} from './dbStore'
import type {
  DbConnectionInfo,
  DbQueryResult,
  DbSchemaNode,
  UiCoreEvent,
} from './dbTypes'

function connection(overrides: Partial<DbConnectionInfo> = {}): DbConnectionInfo {
  return {
    connection_id: 'c1',
    name: 'Local',
    kind: 'sqlite',
    read_only: false,
    state: 'disconnected',
    host: null,
    database: 'app.db',
    ...overrides,
  }
}

function queryResult(overrides: Partial<DbQueryResult> = {}): DbQueryResult {
  return {
    columns: [
      { name: 'id', type_name: 'INTEGER', nullable: false },
      { name: 'name', type_name: 'TEXT', nullable: true },
    ],
    rows: [
      ['1', 'alice'],
      ['2', null],
    ],
    row_count: 12,
    truncated: true,
    handle: 'h1',
    elapsed_ms: 7,
    untrusted: true,
    ...overrides,
  }
}

function schemaNode(): DbSchemaNode {
  return {
    name: 'main',
    kind: 'database',
    detail: null,
    children: [
      {
        name: 'users',
        kind: 'table',
        detail: '2 columns',
        children: [
          {
            name: 'id',
            kind: 'column',
            detail: 'INTEGER NOT NULL',
            children: [],
          },
        ],
      },
    ],
  }
}

function stateWith(overrides: Partial<DbState> = {}): DbState {
  return { ...createDbState(), ...overrides }
}

const translate: Translate = key => `t:${key}`

describe('labels 与摘要纯函数', () => {
  it('connectionLabel 依次回落到名称、database、connection_id', () => {
    expect(connectionLabel(connection({ name: '  Prod  ' }))).toBe('Prod')
    expect(connectionLabel(connection({ name: ' ', database: 'prod.db' }))).toBe('prod.db')
    expect(
      connectionLabel(connection({ name: '', database: null, connection_id: 'conn-x' })),
    ).toBe('conn-x')
  })

  it('dbKindLabel / connectionStateLabel 走 i18n key', () => {
    expect(dbKindLabel('sqlite', translate)).toBe('t:db.kinds.sqlite')
    expect(dbKindLabel('postgres', translate)).toBe('t:db.kinds.postgres')
    expect(connectionStateLabel('connected', translate)).toBe('t:db.states.connected')
  })

  it('resultSummary 汇总行数、样本数、列数、耗时与截断标记', () => {
    expect(resultSummary(null)).toEqual({
      rows: 0,
      sampleRows: 0,
      columns: 0,
      elapsedMs: 0,
      truncated: false,
    })
    expect(resultSummary(queryResult())).toEqual({
      rows: 12,
      sampleRows: 2,
      columns: 2,
      elapsedMs: 7,
      truncated: true,
    })
  })

  it('isUntrusted 只在结果明确标记 untrusted 时为 true', () => {
    expect(isUntrusted(null)).toBe(false)
    expect(isUntrusted(queryResult({ untrusted: false }))).toBe(false)
    expect(isUntrusted(queryResult())).toBe(true)
  })

  it('canConnect 只允许未处于 connecting 的 SQLite 连接', () => {
    expect(canConnect(connection({ kind: 'sqlite', state: 'disconnected' }))).toBe(true)
    expect(canConnect(connection({ kind: 'postgres' }))).toBe(false)
    expect(canConnect(connection({ kind: 'sqlite', state: 'connecting' }))).toBe(false)
  })
})

describe('reducer actions', () => {
  it('createDbState 提供空状态与只读默认值', () => {
    const state = createDbState()
    expect(state).toEqual({
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
    })
  })

  it('dbSqlChanged / dbBusy 不可变更新，值相同时引用不变', () => {
    const state = createDbState()
    const typed = dbSqlChanged(state, 'SELECT 1')
    expect(typed.sql).toBe('SELECT 1')
    expect(state.sql).toBe('')
    expect(dbSqlChanged(typed, 'SELECT 1')).toBe(typed)

    const busy = dbBusy(state, true)
    expect(busy.loading).toBe(true)
    expect(dbBusy(busy, true)).toBe(busy)
  })

  it('dbConnectionsLoaded 保留仍存在的选中连接，否则选第一条并清空旧结果', () => {
    const withActive = stateWith({
      connections: [connection({ connection_id: 'c1' })],
      activeConnectionId: 'c1',
      result: queryResult(),
      schemaNodes: [schemaNode()],
    })
    const kept = dbConnectionsLoaded(withActive, [
      connection({ connection_id: 'c0' }),
      connection({ connection_id: 'c1', read_only: true }),
    ])
    expect(kept.activeConnectionId).toBe('c1')
    expect(kept.readOnly).toBe(true)
    expect(kept.result).not.toBeNull()
    expect(kept.schemaNodes).toHaveLength(1)

    const removed = dbConnectionsLoaded(withActive, [connection({ connection_id: 'c9' })])
    expect(removed.activeConnectionId).toBe('c9')
    expect(removed.result).toBeNull()
    expect(removed.schemaNodes).toEqual([])
    expect(removed.readOnly).toBe(false)
  })

  it('dbConnectionSelected 更新只读徽标并清空 schema/结果', () => {
    const state = stateWith({
      connections: [
        connection({ connection_id: 'c1' }),
        connection({ connection_id: 'c2', read_only: true }),
      ],
      activeConnectionId: 'c1',
      result: queryResult(),
      execResult: { affected_rows: 1, elapsed_ms: 2 },
      explainResult: { plan: 'SCAN', elapsed_ms: 1 },
      schemaNodes: [schemaNode()],
    })
    const next = dbConnectionSelected(state, 'c2')
    expect(next.activeConnectionId).toBe('c2')
    expect(next.readOnly).toBe(true)
    expect(next.result).toBeNull()
    expect(next.execResult).toBeNull()
    expect(next.explainResult).toBeNull()
    expect(next.schemaNodes).toEqual([])
    expect(activeConnection(next)?.connection_id).toBe('c2')
    expect(dbConnectionSelected(next, 'c2')).toBe(next)

    const cleared = dbConnectionSelected(next, 'missing')
    expect(cleared.activeConnectionId).toBeNull()
    expect(cleared.readOnly).toBe(false)
  })

  it('dbSchemaLoaded 只接受当前连接的结果', () => {
    const state = stateWith({
      connections: [connection({ connection_id: 'c1' })],
      activeConnectionId: 'c1',
    })
    expect(dbSchemaLoaded(state, 'other', [schemaNode()])).toBe(state)
    expect(dbSchemaLoaded(state, 'c1', [schemaNode()]).schemaNodes).toHaveLength(1)
  })

  it('query/execute/explain 成功时互斥清空其它结果并复位 loading/error', () => {
    const state = stateWith({
      loading: true,
      error: { code: 'io', message: 'old' },
      execResult: { affected_rows: 1, elapsed_ms: 1 },
      explainResult: { plan: 'old', elapsed_ms: 1 },
    })
    const queried = dbQuerySucceeded(state, queryResult())
    expect(queried.result).not.toBeNull()
    expect(queried.execResult).toBeNull()
    expect(queried.explainResult).toBeNull()
    expect(queried.loading).toBe(false)
    expect(queried.error).toBeNull()

    const executed = dbExecuteSucceeded(queried, { affected_rows: 3, elapsed_ms: 4 })
    expect(executed.result).toBeNull()
    expect(executed.execResult?.affected_rows).toBe(3)

    const explained = dbExplainSucceeded(executed, { plan: 'SCAN users', elapsed_ms: 2 })
    expect(explained.execResult).toBeNull()
    expect(explained.explainResult?.plan).toBe('SCAN users')
  })

  it('dbFailed 结束 loading 并保留错误码与消息', () => {
    const next = dbFailed(stateWith({ loading: true }), {
      code: 'db_rejected',
      message: 'statement blocked',
    })
    expect(next.loading).toBe(false)
    expect(next.error).toEqual({ code: 'db_rejected', message: 'statement blocked' })
  })

  it('applyDbEvent 处理 db:status 的新增与状态更新，忽略其它事件', () => {
    const initial = stateWith({
      connections: [connection({ connection_id: 'c1', state: 'disconnected' })],
      activeConnectionId: 'c1',
      schemaNodes: [schemaNode()],
    })
    const connected = applyDbEvent(initial, {
      type: 'db:status',
      connection: connection({ connection_id: 'c1', state: 'connected', read_only: true }),
    })
    expect(connected.connections).toHaveLength(1)
    expect(connected.connections[0].state).toBe('connected')
    expect(connected.readOnly).toBe(true)
    expect(connected.schemaNodes).toHaveLength(1)

    const disconnected = applyDbEvent(connected, {
      type: 'db:status',
      connection: connection({ connection_id: 'c1', state: 'disconnected' }),
    })
    expect(disconnected.schemaNodes).toEqual([])
    expect(disconnected.readOnly).toBe(false)

    const added = applyDbEvent(disconnected, {
      type: 'db:status',
      connection: connection({ connection_id: 'c2', state: 'error', error: 'boom' }),
    })
    expect(added.connections.map(item => item.connection_id)).toEqual(['c1', 'c2'])

    const errorEvent = {
      type: 'error' as const,
      error: { code: 'io' as const, message: 'nope' },
    }
    expect(applyDbEvent(initial, errorEvent)).toBe(initial)
  })

  it('dbConnectionUpserted 在活动连接断开时清空 schema/结果', () => {
    const state = stateWith({
      connections: [connection({ connection_id: 'c1', state: 'connected' })],
      activeConnectionId: 'c1',
      schemaNodes: [schemaNode()],
      result: queryResult(),
    })
    const next = dbConnectionUpserted(
      state,
      connection({ connection_id: 'c1', state: 'error', error: 'lost' }),
    )
    expect(next.connections[0].state).toBe('error')
    expect(next.schemaNodes).toEqual([])
    expect(next.result).toBeNull()
  })

  it('dbConnectionDeleted 删除非活动连接不影响结果，删除活动连接回落到第一条', () => {
    const state = stateWith({
      connections: [
        connection({ connection_id: 'c1' }),
        connection({ connection_id: 'c2' }),
      ],
      activeConnectionId: 'c1',
      result: queryResult(),
      schemaNodes: [schemaNode()],
    })
    const other = dbConnectionDeleted(state, 'c2')
    expect(other.connections.map(item => item.connection_id)).toEqual(['c1'])
    expect(other.result).not.toBeNull()

    const active = dbConnectionDeleted(state, 'c1')
    expect(active.activeConnectionId).toBe('c2')
    expect(active.result).toBeNull()
    expect(active.schemaNodes).toEqual([])
    expect(dbConnectionDeleted(active, 'missing')).toBe(active)
  })
})

describe('SQL 插入与连接 id', () => {
  it('selectSqlForTable 拼接限定名并忽略空段', () => {
    expect(selectSqlForTable(['main', 'users'])).toBe('SELECT * FROM main.users')
    expect(selectSqlForTable([' ', ''])).toBe('')
  })

  it('sqlWithTableQuery 空编辑器直接写入，否则追加到末尾', () => {
    expect(sqlWithTableQuery('', ['users'])).toBe('SELECT * FROM users')
    expect(sqlWithTableQuery('  \n', ['users'])).toBe('SELECT * FROM users')
    expect(sqlWithTableQuery('SELECT 1;\n\n', ['users'])).toBe(
      'SELECT 1;\nSELECT * FROM users',
    )
    expect(sqlWithTableQuery('SELECT 1', [])).toBe('SELECT 1')
  })

  it('nextConnectionId 避开已有 id 冲突', () => {
    const now = 1_700_000_000_000
    const base = `conn-${now.toString(36)}`
    expect(nextConnectionId([], now)).toBe(base)
    expect(
      nextConnectionId([connection({ connection_id: base })], now),
    ).toBe(`${base}-2`)
    expect(
      nextConnectionId(
        [
          connection({ connection_id: base }),
          connection({ connection_id: `${base}-2` }),
        ],
        now,
      ),
    ).toBe(`${base}-3`)
  })
})

function ok<T>(data: T): BridgeResult<T> {
  return { ok: true, data, fallback: true }
}

function listResult(connections: DbConnectionInfo[]): DbListResult {
  return { kind: 'connections', connections }
}

function connectionResult(item: DbConnectionInfo): DbConnectionResult {
  return { kind: 'connection', connection: item }
}

function schemaResult(connectionId: string, nodes: DbSchemaNode[]): DbSchemaResult {
  return { kind: 'db_schema', connection_id: connectionId, nodes }
}

function queryCommand(result: DbQueryResult): DbQueryCommandResult {
  return { kind: 'db_query', result }
}

function execCommand(affectedRows: number): DbExecCommandResult {
  return { kind: 'db_exec', result: { affected_rows: affectedRows, elapsed_ms: 3 } }
}

function explainCommand(plan: string): DbExplainCommandResult {
  return { kind: 'db_explain', result: { plan, elapsed_ms: 2 } }
}

function ack(): AckResult {
  return { kind: 'ack' }
}

function makeDeps(overrides: Partial<DbControllerDeps> = {}): DbControllerDeps {
  return {
    dbList: async () => ok(listResult([])),
    dbConnect: async id =>
      ok(connectionResult(connection({ connection_id: id, state: 'connected' }))),
    dbDisconnect: async id =>
      ok(connectionResult(connection({ connection_id: id, state: 'disconnected' }))),
    dbSchema: async id => ok(schemaResult(id, [])),
    dbQuery: async () => ok(queryCommand(queryResult())),
    dbExecute: async () => ok(execCommand(1)),
    dbExplain: async () => ok(explainCommand('SCAN')),
    dbSaveConnection: async config =>
      ok(
        connectionResult(
          connection({
            connection_id: config.connection_id,
            name: config.name,
            database: config.database,
            read_only: config.read_only,
          }),
        ),
      ),
    dbDeleteConnection: async () => ok(ack()),
    onCoreEvent: () => () => {},
    ...overrides,
  }
}

describe('createDbController', () => {
  it('refresh 后选中第一条连接，query 把 trim 后的 SQL 与 limit 交给 bridge', async () => {
    const calls: Array<{ id: string; sql: string; limit?: number }> = []
    const deps = makeDeps({
      dbList: async () =>
        ok(
          listResult([
            connection({ connection_id: 'c1', state: 'connected', read_only: true }),
          ]),
        ),
      dbQuery: async (id, sql, limit) => {
        calls.push({ id, sql, limit })
        return ok(queryCommand(queryResult()))
      },
    })
    const controller = createDbController(deps)
    expect(await controller.refresh()).toBe(true)
    expect(controller.getState().activeConnectionId).toBe('c1')
    expect(controller.getState().readOnly).toBe(true)

    controller.setSql('  SELECT 1  ')
    expect(await controller.query(50)).toBe(true)
    expect(calls).toEqual([{ id: 'c1', sql: 'SELECT 1', limit: 50 }])
    expect(controller.getState().result?.row_count).toBe(12)

    controller.dispose()
  })

  it('connect 成功后自动加载 schema', async () => {
    const schema = [schemaNode()]
    const controller = createDbController(
      makeDeps({
        dbList: async () => ok(listResult([connection()])),
        dbConnect: async id =>
          ok(connectionResult(connection({ connection_id: id, state: 'connected' }))),
        dbSchema: async id => ok(schemaResult(id, schema)),
      }),
    )
    await controller.refresh()
    expect(await controller.connect()).toBe(true)
    expect(controller.getState().connections[0].state).toBe('connected')
    expect(controller.getState().schemaNodes).toEqual(schema)

    controller.dispose()
  })

  it('只读连接拒绝 execute，且不调用 bridge', async () => {
    let executed = false
    const controller = createDbController(
      makeDeps({
        dbList: async () =>
          ok(listResult([connection({ state: 'connected', read_only: true })])),
        dbExecute: async () => {
          executed = true
          return ok(execCommand(1))
        },
      }),
    )
    await controller.refresh()
    controller.setSql('DELETE FROM users')
    expect(await controller.execute()).toBe(false)
    expect(executed).toBe(false)
    expect(controller.getState().error?.code).toBe('db_read_only')

    controller.dispose()
  })

  it('saveConnection 选中新连接；deleteConnection 删除后回落', async () => {
    const controller = createDbController(makeDeps())
    const saved = await controller.saveConnection({
      connection_id: 'new-1',
      name: 'New',
      kind: 'sqlite',
      host: '',
      port: 0,
      database: 'new.db',
      user: '',
      secret_ref: '',
      read_only: true,
    })
    expect(saved?.connection_id).toBe('new-1')
    expect(controller.getState().activeConnectionId).toBe('new-1')
    expect(controller.getState().readOnly).toBe(true)

    expect(await controller.deleteConnection('new-1')).toBe(true)
    expect(controller.getState().connections).toEqual([])
    expect(controller.getState().activeConnectionId).toBeNull()
    expect(controller.getState().readOnly).toBe(false)

    controller.dispose()
  })

  it('bridge 错误进入 state.error 并结束 loading', async () => {
    const controller = createDbController(
      makeDeps({
        dbList: async () =>
          ok(listResult([connection({ state: 'connected' })])),
        dbQuery: async () => ({
          ok: false,
          error: { code: 'permission_denied', message: 'blocked' },
          fallback: false,
        }),
      }),
    )
    await controller.refresh()
    controller.setSql('DROP TABLE users')
    expect(await controller.query()).toBe(false)
    expect(controller.getState().loading).toBe(false)
    expect(controller.getState().error).toEqual({
      code: 'permission_denied',
      message: 'blocked',
    })

    controller.dispose()
  })

  it('db:status 事件更新连接列表', async () => {
    let emit: (event: UiCoreEvent) => void = () => {}
    const controller = createDbController(
      makeDeps({
        dbList: async () => ok(listResult([connection()])),
        onCoreEvent: handler => {
          emit = handler
          return () => {}
        },
      }),
    )
    await controller.refresh()
    emit({
      type: 'db:status',
      connection: connection({ state: 'connected' }),
    })
    expect(controller.getState().connections[0].state).toBe('connected')

    controller.dispose()
  })
})
