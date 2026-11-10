import { describe, expect, it } from 'vitest'

import { connectErrorHintKey } from './dbErrorHint'

describe('connectErrorHintKey（db-mysql-pg.md §3.3 四类错误分流）', () => {
  it('db_unavailable + Node 缺失 → 引导运行时设置', () => {
    expect(
      connectErrorHintKey({
        code: 'db_unavailable',
        message: 'Node 运行时不可用（未探测到可用的 node）',
      }),
    ).toBe('db.errorHint.nodeRuntime')
  })

  it('db_unavailable + 网络错误（驱动原文）→ 检查连接参数提示', () => {
    expect(
      connectErrorHintKey({
        code: 'db_unavailable',
        message: 'connect ECONNREFUSED 10.0.0.8:5432',
      }),
    ).toBe('db.errorHint.connection')
  })

  it('db_unavailable + 认证失败（脱敏驱动原文）→ 检查连接参数提示', () => {
    expect(
      connectErrorHintKey({
        code: 'db_unavailable',
        message: 'password authentication failed for user "reporter"',
      }),
    ).toBe('db.errorHint.connection')
  })

  it('db_unavailable + secret_ref 环境变量未设置 → 检查连接参数提示', () => {
    expect(
      connectErrorHintKey({
        code: 'db_unavailable',
        message: '环境变量 PGPASS 未设置（secret_ref）',
      }),
    ).toBe('db.errorHint.connection')
  })

  it('db_rejected / db_read_only → null（现有展示即可）', () => {
    expect(
      connectErrorHintKey({ code: 'db_rejected', message: 'syntax error at "SELEC"' }),
    ).toBeNull()
    expect(
      connectErrorHintKey({
        code: 'db_read_only',
        message: 'read-only connection: execute is disabled',
      }),
    ).toBeNull()
  })

  it('其它错误码 → null（不介入非 db 分流）', () => {
    expect(connectErrorHintKey({ code: 'internal', message: 'x' })).toBeNull()
    expect(connectErrorHintKey({ code: 'not_found', message: 'x' })).toBeNull()
  })
})
