// 连接参数映射、密钥引用解析与各驱动固定的元数据 SQL。
//
// 这个模块不 import 任何驱动，测试可以直接断言参数与 SQL，不需要连数据库。
// 密码只从环境变量解析，绝不写入日志或错误信息；`secret_ref` 本身不是密钥。

import { dbUnavailable, invalidArgument } from './errors.mjs';

export const PROTOCOL_VERSION = 1;
export const CONNECT_TIMEOUT_MS = 10_000;

export const KINDS = ['sqlite', 'postgres', 'mysql', 'mssql'];
export const REMOTE_KINDS = ['postgres', 'mysql', 'mssql'];

export const DRIVER_PACKAGES = {
  postgres: 'pg',
  mysql: 'mysql2',
  mssql: 'mssql',
};

export function kindLabel(kind) {
  switch (kind) {
    case 'postgres':
      return 'PostgreSQL';
    case 'mysql':
      return 'MySQL';
    case 'mssql':
      return 'SQL Server';
    case 'sqlite':
      return 'SQLite';
    default:
      return String(kind);
  }
}

export function isKind(value) {
  return typeof value === 'string' && KINDS.includes(value);
}

export function requireString(params, field) {
  const value = params?.[field];
  if (typeof value !== 'string' || value.length === 0) {
    throw invalidArgument(`missing or invalid "${field}"`);
  }
  return value;
}

export function optionalString(params, field) {
  const value = params?.[field];
  if (value === undefined || value === null) return '';
  if (typeof value !== 'string') {
    throw invalidArgument(`invalid "${field}"`);
  }
  return value;
}

export function requirePort(value) {
  const port = typeof value === 'string' && value.trim() !== '' ? Number(value) : value;
  if (!Number.isInteger(port) || port < 1 || port > 65_535) {
    throw invalidArgument('invalid "port"');
  }
  return port;
}

export function parseExtra(extra) {
  if (extra === undefined || extra === null || extra === '') return {};
  let value = extra;
  if (typeof extra === 'string') {
    try {
      value = JSON.parse(extra);
    } catch {
      throw invalidArgument('"extra" 必须是合法的 JSON 对象');
    }
  }
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw invalidArgument('"extra" 必须是 JSON 对象');
  }
  return value;
}

/**
 * `secret_ref` 为空表示无密码；`env:NAME` 从 sidecar 继承的环境读取。
 * Keyring 尚未在 Rust 侧实现，这里明确拒绝而不是把引用当密码。
 */
export function resolveSecret(secretRef, env = process.env) {
  if (secretRef === undefined || secretRef === null || secretRef === '') {
    return undefined;
  }
  if (typeof secretRef !== 'string') {
    throw invalidArgument('invalid "secret_ref"');
  }
  if (secretRef.startsWith('env:')) {
    const name = secretRef.slice('env:'.length);
    if (name.length === 0) {
      throw invalidArgument('"secret_ref" 的 env: 变量名不能为空');
    }
    const value = env[name];
    if (typeof value !== 'string' || value.length === 0) {
      throw dbUnavailable(`环境变量 ${name} 未设置（secret_ref）`);
    }
    return value;
  }
  if (secretRef.startsWith('keyring://') || secretRef.startsWith('vault://')) {
    throw dbUnavailable('SecretStore 尚未实现；请使用 env:NAME 形式的 secret_ref');
  }
  throw invalidArgument('"secret_ref" 只支持 env:NAME 或留空');
}

/**
 * 返回 `{ kind, options, password, readOnly, readOnlyStatement? }`。
 * `extra` 先展开，身份字段随后覆盖，避免 extra 改写 host/密码等。
 */
export function connectionOptions(kind, params, env = process.env) {
  if (!isKind(kind) || kind === 'sqlite') {
    throw invalidArgument(`unsupported kind: ${kind ?? 'unknown'}`);
  }
  const host = requireString(params, 'host');
  const port = requirePort(params?.port);
  const database = requireString(params, 'database');
  const user = optionalString(params, 'user');
  const readOnly = params?.read_only === true;
  const extra = parseExtra(params?.extra);
  const password = resolveSecret(params?.secret_ref, env);
  // 空 user 交给驱动默认值（PG 的 OS 用户 / MSSQL 的 Windows 认证），
  // 不要显式传空字符串。
  const identity = user === '' ? {} : { user };

  if (kind === 'postgres') {
    return {
      kind,
      password,
      readOnly,
      options: {
        ...extra,
        host,
        port,
        database,
        ...identity,
        password,
        connectionTimeoutMillis: CONNECT_TIMEOUT_MS,
        application_name:
          typeof extra.application_name === 'string' && extra.application_name !== ''
            ? extra.application_name
            : 'lite-code',
      },
      readOnlyStatement: 'SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY',
      readOnlyVerifySql: 'SHOW transaction_read_only',
    };
  }

  if (kind === 'mysql') {
    return {
      kind,
      password,
      readOnly,
      options: {
        ...extra,
        host,
        port,
        database,
        ...identity,
        password,
        connectTimeout: CONNECT_TIMEOUT_MS,
        supportBigNumbers: true,
        bigNumberStrings: true,
        // SQL 闸门已拒绝多语句；驱动层再关一次，避免 extra 打开。
        multipleStatements: false,
      },
      readOnlyStatement: 'SET SESSION TRANSACTION READ ONLY',
    };
  }

  const {
    options: extraOptions = {},
    pool: extraPool = {},
    requestTimeout,
    encrypt,
    trustServerCertificate,
    ...rest
  } = extra;
  if (
    extraOptions === null ||
    typeof extraOptions !== 'object' ||
    Array.isArray(extraOptions)
  ) {
    throw invalidArgument('"extra.options" 必须是 JSON 对象');
  }
  if (extraPool === null || typeof extraPool !== 'object' || Array.isArray(extraPool)) {
    throw invalidArgument('"extra.pool" 必须是 JSON 对象');
  }
  const timeout = Number(requestTimeout);
  // 共享一个物理会话，schema/explain 与普通查询不会争抢连接；
  // SHOWPLAN_TEXT 的会话状态也因此可预测。pool.max 固定为 1。
  const pool = { max: 1, min: 0, idleTimeoutMillis: 30_000, ...extraPool };
  pool.max = 1;
  return {
    kind,
    password,
    readOnly,
    options: {
      ...rest,
      server: host,
      port,
      database,
      ...identity,
      password,
      connectionTimeout: CONNECT_TIMEOUT_MS,
      requestTimeout: Number.isFinite(timeout) && timeout >= 0 ? timeout : 30_000,
      pool,
      options: {
        encrypt: encrypt ?? true,
        trustServerCertificate: trustServerCertificate ?? false,
        appName: 'lite-code',
        ...extraOptions,
        // read_only 时把 ApplicationIntent=ReadOnly 交给服务器/可用性组路由；
        // SQL Server 没有会话级只读事务，写拒绝仍由 Rust SQL 闸门兜底。
        readOnlyIntent: readOnly,
      },
    },
  };
}

export function schemaSql(kind) {
  switch (kind) {
    case 'postgres':
      return {
        tables: `SELECT table_schema, table_name, table_type
FROM information_schema.tables
WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
ORDER BY table_schema, table_type, table_name`,
        columns: `SELECT table_schema, table_name, column_name, data_type, is_nullable
FROM information_schema.columns
WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
ORDER BY table_schema, table_name, ordinal_position`,
      };
    case 'mysql':
      return {
        tables: `SELECT TABLE_SCHEMA AS table_schema, TABLE_NAME AS table_name, TABLE_TYPE AS table_type
FROM information_schema.TABLES
WHERE TABLE_SCHEMA = DATABASE()
ORDER BY TABLE_TYPE, TABLE_NAME`,
        columns: `SELECT TABLE_SCHEMA AS table_schema, TABLE_NAME AS table_name, COLUMN_NAME AS column_name, COLUMN_TYPE AS data_type, IS_NULLABLE AS is_nullable
FROM information_schema.COLUMNS
WHERE TABLE_SCHEMA = DATABASE()
ORDER BY TABLE_NAME, ORDINAL_POSITION`,
      };
    case 'mssql':
      return {
        tables: `SELECT s.name AS table_schema, o.name AS table_name,
       CASE WHEN o.type = 'V' THEN 'VIEW' ELSE 'BASE TABLE' END AS table_type
FROM sys.objects AS o
JOIN sys.schemas AS s ON o.schema_id = s.schema_id
WHERE o.type IN ('U', 'V') AND o.is_ms_shipped = 0
ORDER BY s.name, o.type, o.name`,
        columns: `SELECT s.name AS table_schema, o.name AS table_name, c.name AS column_name, t.name AS data_type, c.is_nullable AS is_nullable
FROM sys.columns AS c
JOIN sys.objects AS o ON c.object_id = o.object_id
JOIN sys.schemas AS s ON o.schema_id = s.schema_id
JOIN sys.types AS t ON c.user_type_id = t.user_type_id
WHERE o.type IN ('U', 'V') AND o.is_ms_shipped = 0
ORDER BY s.name, o.name, c.column_id`,
      };
    default:
      return null;
  }
}

export function explainSql(kind, sql) {
  switch (kind) {
    case 'postgres':
      return `EXPLAIN ${sql}`;
    case 'mysql':
      return `EXPLAIN ${sql}`;
    default:
      return null;
  }
}

export function planText(rows) {
  return (rows ?? [])
    .map((row) => {
      if (row && typeof row === 'object' && !Array.isArray(row)) {
        const values = Object.values(row);
        if (values.length === 1) return String(values[0] ?? '');
        return Object.entries(row)
          .map(([key, value]) => `${key}: ${String(value ?? '')}`)
          .join('  ');
      }
      return String(row ?? '');
    })
    .filter((line) => line.length > 0)
    .join('\n');
}

export function schemaTree(kind, database, tableRows, columnRows) {
  const columnsByTable = new Map();
  for (const row of columnRows ?? []) {
    const tableSchema = String(row.table_schema ?? '');
    const tableName = String(row.table_name ?? '');
    const key = `${tableSchema}\u0000${tableName}`;
    const columns = columnsByTable.get(key) ?? [];
    columns.push({
      name: String(row.column_name ?? ''),
      kind: 'column',
      detail: row.data_type === undefined || row.data_type === null
        ? ''
        : String(row.data_type),
      children: [],
    });
    columnsByTable.set(key, columns);
  }

  const tablesBySchema = new Map();
  for (const row of tableRows ?? []) {
    const tableSchema = String(row.table_schema ?? '');
    const tableName = String(row.table_name ?? '');
    const tableType = String(row.table_type ?? '');
    const key = `${tableSchema}\u0000${tableName}`;
    const table = {
      name: tableName,
      kind: tableType.toUpperCase().includes('VIEW') ? 'view' : 'table',
      detail: tableType.toUpperCase().includes('VIEW') ? 'view' : 'table',
      children: columnsByTable.get(key) ?? [],
    };
    const tables = tablesBySchema.get(tableSchema) ?? [];
    tables.push(table);
    tablesBySchema.set(tableSchema, tables);
  }

  if (kind === 'mysql') {
    // MySQL 的 database 就是 schema，保持与 SQLite 一样的扁平表列表。
    return [
      {
        name: database,
        kind: 'database',
        detail: 'mysql',
        children: [...tablesBySchema.values()].flat(),
      },
    ];
  }

  return [
    {
      name: database,
      kind: 'database',
      detail: kind === 'mssql' ? 'mssql' : 'postgres',
      children: [...tablesBySchema.entries()].map(([schema, tables]) => ({
        name: schema,
        kind: 'schema',
        detail: 'schema',
        children: tables,
      })),
    },
  ];
}
