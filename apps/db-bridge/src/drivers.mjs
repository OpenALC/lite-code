// 驱动懒加载与适配层。esbuild 会把三个 npm 包打进单文件；若某个包在构建时
// 被排除，动态 import 会失败，connect 会明确返回“驱动未打包”。

import { CONNECT_TIMEOUT_MS, kindLabel } from './config.mjs';
import { dbUnavailable, messageOf } from './errors.mjs';

const LOADERS = {
  postgres: { packageName: 'pg', load: () => import('pg') },
  mysql: { packageName: 'mysql2', load: () => import('mysql2/promise') },
  mssql: { packageName: 'mssql', load: () => import('mssql') },
};

export async function loadBundledDrivers() {
  const available = {};
  const errors = {};
  await Promise.all(
    Object.entries(LOADERS).map(async ([kind, { packageName, load }]) => {
      try {
        available[kind] = await load();
      } catch (error) {
        errors[kind] = `${packageName}: ${messageOf(error)}`;
      }
    }),
  );
  return { available, errors };
}

const PG_TYPE_NAMES = new Map([
  [16, 'bool'],
  [17, 'bytea'],
  [18, 'char'],
  [19, 'name'],
  [20, 'int8'],
  [21, 'int2'],
  [23, 'int4'],
  [25, 'text'],
  [26, 'oid'],
  [114, 'json'],
  [142, 'xml'],
  [700, 'float4'],
  [701, 'float8'],
  [790, 'money'],
  [1042, 'bpchar'],
  [1043, 'varchar'],
  [1082, 'date'],
  [1083, 'time'],
  [1114, 'timestamp'],
  [1184, 'timestamptz'],
  [1186, 'interval'],
  [1266, 'timetz'],
  [1700, 'numeric'],
  [2950, 'uuid'],
  [3802, 'jsonb'],
]);

const MYSQL_TYPE_NAMES = new Map([
  [0, 'decimal'],
  [1, 'tinyint'],
  [2, 'smallint'],
  [3, 'int'],
  [4, 'float'],
  [5, 'double'],
  [7, 'timestamp'],
  [8, 'bigint'],
  [9, 'mediumint'],
  [10, 'date'],
  [11, 'time'],
  [12, 'datetime'],
  [13, 'year'],
  [14, 'date'],
  [15, 'varchar'],
  [16, 'bit'],
  [245, 'json'],
  [246, 'decimal'],
  [247, 'enum'],
  [248, 'set'],
  [249, 'tinyblob'],
  [250, 'mediumblob'],
  [251, 'longblob'],
  [252, 'blob'],
  [253, 'varchar'],
  [254, 'char'],
  [255, 'geometry'],
]);

const pgTypeName = (oid) => PG_TYPE_NAMES.get(oid) ?? `oid:${oid}`;
const mysqlTypeName = (type) => MYSQL_TYPE_NAMES.get(type) ?? `type:${type}`;

function ignoreRuntimeError(target) {
  if (typeof target?.on === 'function') {
    // 空闲连接错误不应让 sidecar 崩溃；下一次请求会返回可读错误。
    target.on('error', () => {});
  }
}

function timeoutError(kind) {
  return dbUnavailable(`${kindLabel(kind)} 连接超时（${CONNECT_TIMEOUT_MS} ms）`);
}

async function withTimeout(promise, kind) {
  // 驱动自身也有 10s connectTimeout；这里多留 1s，让驱动先拒绝并清理
  // socket，外层的 Promise.race 只作为兜底。
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(
      () => reject(timeoutError(kind)),
      CONNECT_TIMEOUT_MS + 1_000,
    );
    timer.unref?.();
  });
  try {
    return await Promise.race([promise, timeout]);
  } finally {
    clearTimeout(timer);
  }
}

class PgAdapter {
  constructor(client) {
    this.client = client;
  }

  async select(sql) {
    const result = await this.client.query({ text: sql, rowMode: 'object' });
    return result?.rows ?? [];
  }

  async query(sql) {
    const result = await this.client.query({ text: sql, rowMode: 'array' });
    return {
      columns: (result?.fields ?? []).map((field) => ({
        name: field.name,
        type_name: pgTypeName(field.dataTypeID),
        nullable: true,
      })),
      rows: result?.rows ?? [],
    };
  }

  async execute(sql) {
    const result = await this.client.query(sql);
    return Number(result?.rowCount ?? 0);
  }

  async close() {
    await this.client.end();
  }
}

class MysqlAdapter {
  constructor(connection) {
    this.connection = connection;
  }

  async select(sql) {
    const [rows] = await this.connection.query(sql);
    return Array.isArray(rows) ? rows : [];
  }

  async query(sql) {
    const [rows, fields] = await this.connection.query({ sql, rowsAsArray: true });
    if (!Array.isArray(rows)) {
      throw new Error('query did not return a result set');
    }
    return {
      columns: (fields ?? []).map((field) => ({
        name: field.name,
        type_name: mysqlTypeName(field.type),
        nullable: (Number(field.flags) & 1) === 0,
      })),
      rows,
    };
  }

  async execute(sql) {
    const [result] = await this.connection.query(sql);
    return Number(result?.affectedRows ?? 0);
  }

  async close() {
    await this.connection.end();
  }
}

class MssqlAdapter {
  constructor(pool) {
    this.pool = pool;
  }

  async select(sql) {
    const result = await this.pool.request().query(sql);
    return result?.recordset ?? [];
  }

  async query(sql) {
    const request = this.pool.request();
    request.arrayRowMode = true;
    const result = await request.query(sql);
    const metadata = result?.columns?.[0] ?? [];
    return {
      columns: metadata.map((column) => ({
        name: column.name,
        type_name:
          column.type?.declaration ?? column.type?.name ?? '',
        nullable: column.nullable !== false,
      })),
      rows: result?.recordset ?? [],
    };
  }

  async execute(sql) {
    const result = await this.pool.request().query(sql);
    return (result?.rowsAffected ?? []).reduce(
      (total, count) => total + Number(count ?? 0),
      0,
    );
  }

  async close() {
    await this.pool.close();
  }
}

async function connectPg(module, settings) {
  const pg = module.default ?? module;
  const client = new pg.Client(settings.options);
  ignoreRuntimeError(client);
  try {
    await withTimeout(client.connect(), 'postgres');
  } catch (error) {
    await Promise.resolve(client.end()).catch(() => {});
    throw error;
  }
  return new PgAdapter(client);
}

async function connectMysql(module, settings) {
  const mysql = module.default ?? module;
  const connection = await withTimeout(
    mysql.createConnection(settings.options),
    'mysql',
  );
  ignoreRuntimeError(connection);
  return new MysqlAdapter(connection);
}

async function connectMssql(module, settings) {
  const sql = module.default ?? module;
  const pool = new sql.ConnectionPool(settings.options);
  ignoreRuntimeError(pool);
  try {
    await withTimeout(pool.connect(), 'mssql');
  } catch (error) {
    await Promise.resolve(pool.close()).catch(() => {});
    throw error;
  }
  return new MssqlAdapter(pool);
}

export async function createAdapter(kind, module, settings) {
  switch (kind) {
    case 'postgres':
      return connectPg(module, settings);
    case 'mysql':
      return connectMysql(module, settings);
    case 'mssql':
      return connectMssql(module, settings);
    default:
      throw dbUnavailable(`${kindLabel(kind)} 驱动未打包`);
  }
}
