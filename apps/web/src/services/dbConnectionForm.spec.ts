import { describe, expect, it } from 'vitest'

import {
  buildConfig,
  createDraft,
  defaultPort,
  draftFromConfig,
  draftFromInfo,
  envNameToSecretRef,
  isRemoteKind,
  parsePort,
  remoteSummary,
  secretRefToEnvName,
  validateDraft,
  type ConnectionDraft,
} from './dbConnectionForm'
import type { DbConnectionConfig, DbConnectionInfo } from './dbTypes'

function draft(overrides: Partial<ConnectionDraft> = {}): ConnectionDraft {
  return { ...createDraft(), ...overrides }
}

function config(overrides: Partial<DbConnectionConfig> = {}): DbConnectionConfig {
  return {
    connection_id: 'c1',
    name: 'Local',
    kind: 'sqlite',
    host: '',
    port: 0,
    database: 'app.db',
    user: '',
    secret_ref: '',
    read_only: false,
    ...overrides,
  }
}

describe('kind 与默认端口', () => {
  it('isRemoteKind 只有 sqlite 是本地 kind', () => {
    expect(isRemoteKind('sqlite')).toBe(false)
    expect(isRemoteKind('postgres')).toBe(true)
    expect(isRemoteKind('mysql')).toBe(true)
    expect(isRemoteKind('mssql')).toBe(true)
  })

  it('defaultPort：pg 5432 / mysql 3306 / mssql 1433 / sqlite 0', () => {
    expect(defaultPort('postgres')).toBe(5432)
    expect(defaultPort('mysql')).toBe(3306)
    expect(defaultPort('mssql')).toBe(1433)
    expect(defaultPort('sqlite')).toBe(0)
  })

  it('createDraft：远程 kind 预填默认端口，sqlite 端口为空串', () => {
    expect(createDraft('postgres').port).toBe('5432')
    expect(createDraft('mysql').port).toBe('3306')
    expect(createDraft('sqlite').port).toBe('')
  })
})

describe('parsePort', () => {
  it('空串回落 kind 默认端口', () => {
    expect(parsePort('', 'postgres')).toBe(5432)
    expect(parsePort('  ', 'mysql')).toBe(3306)
  })

  it('合法端口原样返回', () => {
    expect(parsePort('5432', 'postgres')).toBe(5432)
    expect(parsePort('3307', 'mysql')).toBe(3307)
    expect(parsePort('1', 'postgres')).toBe(1)
    expect(parsePort('65535', 'mysql')).toBe(65535)
  })

  it('非法端口返回 null：越界/非整数/非数字', () => {
    expect(parsePort('0', 'postgres')).toBeNull()
    expect(parsePort('65536', 'postgres')).toBeNull()
    expect(parsePort('-1', 'postgres')).toBeNull()
    expect(parsePort('3.5', 'postgres')).toBeNull()
    expect(parsePort('abc', 'postgres')).toBeNull()
    expect(parsePort('12a', 'postgres')).toBeNull()
  })

  it('sqlite 恒为 0', () => {
    expect(parsePort('9999', 'sqlite')).toBe(0)
  })
})

describe('secret_ref 环境变量名往返', () => {
  it('envNameToSecretRef：trim 并拼 env: 前缀，空串表示无密码', () => {
    expect(envNameToSecretRef('PGPASS')).toBe('env:PGPASS')
    expect(envNameToSecretRef('  MYSQL_PWD ')).toBe('env:MYSQL_PWD')
    expect(envNameToSecretRef('')).toBe('')
    expect(envNameToSecretRef('   ')).toBe('')
  })

  it('secretRefToEnvName 反解 env: 前缀', () => {
    expect(secretRefToEnvName('env:PGPASS')).toBe('PGPASS')
    expect(secretRefToEnvName('env:MYSQL_PWD')).toBe('MYSQL_PWD')
  })

  it('非 env: 引用（keyring:// 等未实现方案）与空串反解为空', () => {
    expect(secretRefToEnvName('')).toBe('')
    expect(secretRefToEnvName('keyring://db/pg')).toBe('')
  })

  it('secret_ref 往返：env 名 → secret_ref → env 名恒等', () => {
    for (const name of ['PGPASS', 'MYSQL_PWD']) {
      expect(secretRefToEnvName(envNameToSecretRef(name))).toBe(name)
    }
  })
})

describe('validateDraft', () => {
  it('合法草稿返回 null（sqlite 与远程）', () => {
    expect(validateDraft(draft({ name: 'a', database: 'app.db' }))).toBeNull()
    expect(
      validateDraft(
        draft({ name: 'a', kind: 'postgres', host: 'h', database: 'db' }),
      ),
    ).toBeNull()
  })

  it('名称必填', () => {
    expect(validateDraft(draft({ name: '  ' }))).toBe('db.nameRequired')
  })

  it('远程 kind 主机必填', () => {
    expect(
      validateDraft(draft({ name: 'a', kind: 'mysql', host: ' ', database: 'db' })),
    ).toBe('db.hostRequired')
  })

  it('远程 kind 端口非法报 portInvalid', () => {
    expect(
      validateDraft(
        draft({ name: 'a', kind: 'postgres', host: 'h', database: 'db', port: '70000' }),
      ),
    ).toBe('db.portInvalid')
    expect(
      validateDraft(
        draft({ name: 'a', kind: 'postgres', host: 'h', database: 'db', port: 'x' }),
      ),
    ).toBe('db.portInvalid')
  })

  it('sqlite 数据库路径为空报 pathRequired，远程报 databaseRequired', () => {
    expect(validateDraft(draft({ name: 'a', database: ' ' }))).toBe('db.pathRequired')
    expect(
      validateDraft(draft({ name: 'a', kind: 'mysql', host: 'h', database: ' ' })),
    ).toBe('db.databaseRequired')
  })

  it('远程 kind 空端口输入合法（回落默认端口）', () => {
    expect(
      validateDraft(
        draft({ name: 'a', kind: 'postgres', host: 'h', database: 'db', port: '' }),
      ),
    ).toBeNull()
  })
})

describe('buildConfig', () => {
  it('sqlite 回归：远程字段全部为空，端口为 0', () => {
    const built = buildConfig(
      draft({ name: ' 本地 ', kind: 'sqlite', database: ' app.db ', readOnly: true }),
      'c1',
    )
    expect(built).toEqual({
      connection_id: 'c1',
      name: '本地',
      kind: 'sqlite',
      host: '',
      port: 0,
      database: 'app.db',
      user: '',
      secret_ref: '',
      read_only: true,
    })
  })

  it('postgres：组装远程字段，空端口回落默认 5432', () => {
    const built = buildConfig(
      draft({
        name: 'pg',
        kind: 'postgres',
        host: ' db.internal ',
        port: '',
        database: ' analytics ',
        user: ' reporter ',
        passwordEnv: 'PGPASS',
        readOnly: true,
      }),
      'c2',
    )
    expect(built).toEqual({
      connection_id: 'c2',
      name: 'pg',
      kind: 'postgres',
      host: 'db.internal',
      port: 5432,
      database: 'analytics',
      user: 'reporter',
      secret_ref: 'env:PGPASS',
      read_only: true,
    })
  })

  it('mysql：显式端口与非空密码环境变量名', () => {
    const built = buildConfig(
      draft({
        name: 'my',
        kind: 'mysql',
        host: 'db.internal',
        port: '3307',
        database: 'shop',
        user: '',
        passwordEnv: 'MYSQL_PWD ',
      }),
      'c3',
    )
    expect(built.host).toBe('db.internal')
    expect(built.port).toBe(3307)
    expect(built.user).toBe('')
    expect(built.secret_ref).toBe('env:MYSQL_PWD')
  })

  it('远程 kind 无密码时 secret_ref 为空串', () => {
    const built = buildConfig(
      draft({ name: 'pg', kind: 'postgres', host: 'h', database: 'db' }),
      'c4',
    )
    expect(built.secret_ref).toBe('')
  })
})

describe('draftFromConfig 回填', () => {
  it('远程配置反解：secret_ref → 环境变量名，端口回填数字', () => {
    const source = config({
      kind: 'postgres',
      host: 'db.internal',
      port: 5433,
      database: 'analytics',
      user: 'reporter',
      secret_ref: 'env:PGPASS',
      read_only: true,
    })
    const filled = draftFromConfig(source)
    expect(filled).toEqual({
      name: 'Local',
      kind: 'postgres',
      host: 'db.internal',
      port: '5433',
      database: 'analytics',
      user: 'reporter',
      passwordEnv: 'PGPASS',
      readOnly: true,
    })
  })

  it('远程配置 port 为 0 时回填 kind 默认端口', () => {
    const filled = draftFromConfig(
      config({ kind: 'mysql', host: 'h', database: 'd', port: 0 }),
    )
    expect(filled.port).toBe('3306')
  })

  it('sqlite 配置回填保持远程字段为空', () => {
    const filled = draftFromConfig(config({ database: 'app.db' }))
    expect(filled.host).toBe('')
    expect(filled.port).toBe('')
    expect(filled.user).toBe('')
    expect(filled.passwordEnv).toBe('')
    expect(filled.database).toBe('app.db')
  })

  it('draft → config → draft 往返恒等（含 secret_ref）', () => {
    const source = config({
      kind: 'postgres',
      host: 'db.internal',
      port: 5433,
      database: 'analytics',
      user: 'reporter',
      secret_ref: 'env:PGPASS',
      read_only: false,
    })
    expect(buildConfig(draftFromConfig(source), source.connection_id)).toEqual(source)
  })
})

describe('draftFromInfo（列表数据源回填）', () => {
  function info(overrides: Partial<DbConnectionInfo> = {}): DbConnectionInfo {
    return {
      connection_id: 'c1',
      name: 'Prod',
      kind: 'postgres',
      read_only: false,
      state: 'disconnected',
      host: 'db.internal',
      database: 'analytics',
      ...overrides,
    }
  }

  /** 模拟 lc-core info_from_config（契约 0.9）：空 user/secret_ref 不携带。 */
  function infoFromConfig(config: DbConnectionConfig): DbConnectionInfo {
    const remote = isRemoteKind(config.kind)
    return {
      connection_id: config.connection_id,
      name: config.name,
      kind: config.kind,
      read_only: config.read_only,
      state: 'disconnected',
      host: remote ? config.host || null : null,
      port: remote ? config.port : 0,
      user: remote && config.user ? config.user : null,
      secretRef: remote && config.secret_ref ? config.secret_ref : null,
      database: config.database || null,
      error: null,
    }
  }

  it('远程连接回填 host/database/name，port 回落默认、user/密码为空', () => {
    const filled = draftFromInfo(info())
    expect(filled.name).toBe('Prod')
    expect(filled.host).toBe('db.internal')
    expect(filled.database).toBe('analytics')
    expect(filled.port).toBe('5432')
    expect(filled.user).toBe('')
    expect(filled.passwordEnv).toBe('')
  })

  it('sqlite 连接回填远程字段为空', () => {
    const filled = draftFromInfo(
      info({ kind: 'sqlite', host: null, database: 'app.db' }),
    )
    expect(filled.host).toBe('')
    expect(filled.port).toBe('')
    expect(filled.database).toBe('app.db')
  })

  it('契约 0.9：info 携带 port/user/secretRef 时如实回填（优先于回落）', () => {
    const filled = draftFromInfo(
      info({ port: 5433, user: 'reporter', secretRef: 'env:PGPASS' }),
    )
    expect(filled.port).toBe('5433')
    expect(filled.user).toBe('reporter')
    expect(filled.passwordEnv).toBe('PGPASS')
  })

  it('契约 0.9：sqlite 的 info 即使误带远程字段也回填为空', () => {
    const filled = draftFromInfo(
      info({
        kind: 'sqlite',
        host: null,
        database: 'app.db',
        port: 0,
        user: null,
        secretRef: null,
      }),
    )
    expect(filled.host).toBe('')
    expect(filled.port).toBe('')
    expect(filled.user).toBe('')
    expect(filled.passwordEnv).toBe('')
  })

  it('编辑回填往返：完整远程 draft → config → info → draft 全等', () => {
    const original = draft({
      name: 'pg',
      kind: 'postgres',
      host: 'db.internal',
      port: '5433',
      database: 'analytics',
      user: 'reporter',
      passwordEnv: 'PGPASS',
      readOnly: true,
    })
    const roundtrip = draftFromInfo(infoFromConfig(buildConfig(original, 'c1')))
    expect(roundtrip).toEqual(original)
  })

  it('无密码远程连接往返后 user/密码仍为空串', () => {
    const original = draft({
      name: 'pg',
      kind: 'postgres',
      host: 'db.internal',
      port: '5432',
      database: 'analytics',
    })
    const roundtrip = draftFromInfo(infoFromConfig(buildConfig(original, 'c1')))
    expect(roundtrip).toEqual(original)
  })

  it('端口缺省归一：draft 空端口经 config（回落默认）→ info 回填后为默认端口串', () => {
    const original = draft({
      name: 'pg',
      kind: 'postgres',
      host: 'db.internal',
      port: '',
      database: 'analytics',
    })
    const roundtrip = draftFromInfo(infoFromConfig(buildConfig(original, 'c1')))
    expect(roundtrip.port).toBe('5432')
    expect(roundtrip).toEqual({ ...original, port: '5432' })
  })
})

describe('remoteSummary', () => {
  it('host + database（无 port）渲染 host/database', () => {
    expect(remoteSummary({ host: 'db.internal', database: 'analytics' })).toBe(
      'db.internal/analytics',
    )
  })

  it('带 port 渲染 host:port/database', () => {
    expect(
      remoteSummary({ host: 'db.internal', port: 5433, database: 'analytics' }),
    ).toBe('db.internal:5433/analytics')
  })

  it('无 database 退化为 host，host 空白返回 null', () => {
    expect(remoteSummary({ host: 'db.internal' })).toBe('db.internal')
    expect(remoteSummary({ host: ' ', database: 'db' })).toBeNull()
    expect(remoteSummary({ host: null })).toBeNull()
    expect(remoteSummary({ database: 'app.db' })).toBeNull()
  })
})
