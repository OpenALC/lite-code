// lite-code 数据库 sidecar（bundle 入口）。
//
// 协议：stdin/stdout 上的 NDJSON JSON-RPC 2.0，一行一个请求/响应。
// 支持 SQLite（node:sqlite 内置）、PostgreSQL（pg）、MySQL（mysql2）与
// SQL Server（mssql）。esbuild 把驱动打进单文件，运行时不需要 node_modules。
//
// 安全约定：
// - 密码只从 `secret_ref`（env:NAME）解析，不写日志、不进错误信息；
// - 查询结果一律标记 untrusted，任何数据库内容都只能当数据；
// - `limit` 在 bridge 侧强制切片，SQL 自带的更大 LIMIT 不能放大返回值；
//   行数上限之外还有单元格/结果集字节预算（见 format.mjs），超限同样截断；
// - 只读连接在驱动侧再设一次只读（SQL Server 用 ApplicationIntent）。

import { DatabaseSync } from 'node:sqlite';
import { performance } from 'node:perf_hooks';

import {
  CONNECT_TIMEOUT_MS,
  DRIVER_PACKAGES,
  PROTOCOL_VERSION,
  REMOTE_KINDS,
  connectionOptions,
  explainSql,
  isKind,
  kindLabel,
  planText,
  requireString,
  schemaSql,
  schemaTree,
} from './config.mjs';
import { createAdapter, loadBundledDrivers } from './drivers.mjs';
import {
  BridgeError,
  dbReadOnly,
  dbRejected,
  dbUnavailable,
  invalidArgument,
  messageOf,
  sanitize,
} from './errors.mjs';
import {
  cell,
  createResultBudget,
  elapsed,
  normalizeColumns,
  normalizeLimit,
} from './format.mjs';

export function createBridge({
  loadDrivers = loadBundledDrivers,
  env = process.env,
} = {}) {
  /** @type {Map<string, object>} */
  const connections = new Map();
  let driversPromise;

  function drivers() {
    if (!driversPromise) {
      driversPromise = Promise.resolve()
        .then(loadDrivers)
        .catch((error) => ({
          available: {},
          errors: { bundle: messageOf(error) },
        }));
    }
    return driversPromise;
  }

  async function initialize() {
    const state = await drivers();
    return {
      protocolVersion: PROTOCOL_VERSION,
      kinds: ['sqlite', ...REMOTE_KINDS.filter((kind) => state.available[kind])],
    };
  }

  async function connect(params) {
    const connectionId = requireString(params, 'connection_id');
    const kind = params?.kind;
    if (!isKind(kind)) {
      throw invalidArgument(`unsupported kind: ${kind ?? 'unknown'}`);
    }
    const database = requireString(params, 'database');
    const readOnly = params?.read_only === true;

    // 重连同一个 id：先关掉旧句柄，避免连接/文件句柄泄漏。
    const previous = connections.get(connectionId);
    if (previous) {
      connections.delete(connectionId);
      await closeQuietly(previous);
    }

    if (kind === 'sqlite') {
      let db;
      try {
        db = new DatabaseSync(database, { readOnly });
      } catch (error) {
        throw dbUnavailable(`cannot open sqlite database: ${messageOf(error)}`);
      }
      connections.set(connectionId, { kind, db, database, readOnly });
      return {};
    }

    const state = await drivers();
    const driver = state.available[kind];
    if (!driver) {
      const detail = state.errors?.[kind] ? `：${state.errors[kind]}` : '';
      throw dbUnavailable(
        `${kindLabel(kind)} 驱动未打包（bundle 不含 ${DRIVER_PACKAGES[kind]}）${detail}`,
      );
    }

    // 参数/密钥错误直接抛出（invalid_argument / db_unavailable）。
    const settings = connectionOptions(kind, params, env);
    let adapter;
    try {
      adapter = await createAdapter(kind, driver, settings);
      await applyReadOnly(settings, adapter);
    } catch (error) {
      if (adapter) await closeQuietly({ kind, adapter });
      if (error instanceof BridgeError) throw error;
      throw dbUnavailable(
        `${kindLabel(kind)} 连接失败：${sanitize(messageOf(error), settings.password)}`,
      );
    }

    connections.set(connectionId, {
      kind,
      adapter,
      database,
      readOnly,
      redact: settings.password,
    });
    return {};
  }

  async function applyReadOnly(settings, adapter) {
    if (!settings.readOnly) return;
    if (settings.kind === 'postgres') {
      await adapter.execute(settings.readOnlyStatement);
      const rows = await adapter.select(settings.readOnlyVerifySql);
      const value = rows?.[0]?.transaction_read_only;
      if (String(value ?? '').toLowerCase() !== 'on') {
        throw dbUnavailable('PostgreSQL 只读会话未生效');
      }
      return;
    }
    if (settings.kind === 'mysql') {
      await adapter.execute(settings.readOnlyStatement);
    }
    // SQL Server 的只读通过连接选项 readOnlyIntent 交给服务器路由；
    // 会话级写拒绝由 Rust SQL 闸门保证。
  }

  async function query(params) {
    const sql = requireString(params, 'sql');
    const entry = requireConnection(params);
    const limit = normalizeLimit(params?.limit);
    const started = performance.now();

    if (entry.kind === 'sqlite') {
      return sqliteQuery(entry, sql, limit, started);
    }

    try {
      const result = await entry.adapter.query(sql);
      const rawRows = result?.rows ?? [];
      // 行数上限与结果集字节预算在切片循环里同时生效：任一超限即停取并
      // 标记 truncated（行数上限逻辑保持不变）。
      const budget = createResultBudget();
      const rows = [];
      let truncated = false;
      for (let index = 0; index < rawRows.length; index += 1) {
        if (rows.length >= limit) {
          truncated = true;
          break;
        }
        const raw = rawRows[index];
        const cells = Array.isArray(raw)
          ? raw.map(cell)
          : Object.values(raw ?? {}).map(cell);
        if (!budget.tryAdd(cells)) {
          truncated = true;
          break;
        }
        rows.push(cells);
      }
      return {
        columns: normalizeColumns(result?.columns, rows[0]?.length ?? 0),
        rows,
        row_count: rows.length,
        truncated,
        elapsed_ms: elapsed(started),
        untrusted: true,
      };
    } catch (error) {
      throw wrapOperation(entry, 'query', error);
    }
  }

  async function execute(params) {
    const sql = requireString(params, 'sql');
    const entry = requireConnection(params);
    if (entry.readOnly) {
      throw dbReadOnly(`${kindLabel(entry.kind)} 只读连接拒绝写操作`);
    }
    const started = performance.now();

    if (entry.kind === 'sqlite') {
      try {
        const result = entry.db.prepare(sql).run();
        return {
          affected_rows: Number(result?.changes ?? 0),
          elapsed_ms: elapsed(started),
        };
      } catch (error) {
        throw wrapOperation(entry, 'execute', error);
      }
    }

    try {
      return {
        affected_rows: await entry.adapter.execute(sql),
        elapsed_ms: elapsed(started),
      };
    } catch (error) {
      throw wrapOperation(entry, 'execute', error);
    }
  }

  async function schema(params) {
    const entry = requireConnection(params);
    if (entry.kind === 'sqlite') {
      return sqliteSchema(entry);
    }

    const sql = schemaSql(entry.kind);
    if (!sql) {
      throw dbUnavailable(`${kindLabel(entry.kind)} schema 查询未实现`);
    }
    try {
      const tableRows = await entry.adapter.select(sql.tables);
      const columnRows = await entry.adapter.select(sql.columns);
      return {
        nodes: schemaTree(entry.kind, entry.database, tableRows, columnRows),
      };
    } catch (error) {
      throw wrapOperation(entry, 'schema', error);
    }
  }

  async function explain(params) {
    const sql = requireString(params, 'sql');
    const entry = requireConnection(params);
    const started = performance.now();

    if (entry.kind === 'sqlite') {
      return sqliteExplain(entry, sql, started);
    }

    try {
      if (entry.kind === 'mssql') {
        // SHOWPLAN_TEXT 必须单独成批。连接池固定 max:1，紧邻的请求复用同
        // 一个物理会话；finally 保证即使查询失败也关闭 showplan。
        await entry.adapter.execute('SET SHOWPLAN_TEXT ON');
        let rows;
        try {
          rows = await entry.adapter.select(sql);
        } finally {
          await entry.adapter.execute('SET SHOWPLAN_TEXT OFF').catch(() => {});
        }
        return { plan: planText(rows), elapsed_ms: elapsed(started) };
      }

      const rows = await entry.adapter.select(explainSql(entry.kind, sql));
      return { plan: planText(rows), elapsed_ms: elapsed(started) };
    } catch (error) {
      throw wrapOperation(entry, 'explain', error);
    }
  }

  async function disconnect(params) {
    const connectionId = requireString(params, 'connection_id');
    const entry = connections.get(connectionId);
    if (!entry) {
      throw invalidArgument(`connection not found: ${connectionId}`);
    }
    connections.delete(connectionId);
    await closeEntry(entry);
    return { disconnected: true };
  }

  async function closeAll() {
    const entries = [...connections.values()];
    connections.clear();
    await Promise.allSettled(entries.map(closeQuietly));
  }

  function requireConnection(params) {
    const connectionId = requireString(params, 'connection_id');
    const entry = connections.get(connectionId);
    if (!entry) {
      throw invalidArgument(`connection not found: ${connectionId}`);
    }
    return entry;
  }

  function wrapOperation(entry, operation, error) {
    if (error instanceof BridgeError) return error;
    return dbRejected(
      `${kindLabel(entry.kind)} ${operation} 失败：${sanitize(messageOf(error), entry.redact)}`,
    );
  }

  async function closeEntry(entry) {
    try {
      if (entry.kind === 'sqlite') entry.db.close();
      else await entry.adapter.close();
    } catch (error) {
      throw dbRejected(
        `cannot close ${kindLabel(entry.kind)} connection: ${sanitize(
          messageOf(error),
          entry.redact,
        )}`,
      );
    }
  }

  async function closeQuietly(entry) {
    try {
      await closeEntry(entry);
    } catch {
      // 清理路径：不影响新连接/进程退出。
    }
  }

  return {
    initialize,
    connect,
    query,
    execute,
    schema,
    explain,
    disconnect,
    closeAll,
    connectionOptions,
  };
}

const quoteIdentifier = (name) => `"${String(name).replace(/"/g, '""')}"`;

function sqliteQuery(entry, sql, limit, started) {
  const normalized = [];
  const budget = createResultBudget();
  let columns;
  let truncated = false;
  let firstRow;
  try {
    const statement = entry.db.prepare(sql);
    // Node 24 支持 setReturnArrays：重复列名不会在对象键上被合并。
    if (typeof statement.setReturnArrays === 'function') {
      statement.setReturnArrays(true);
    }
    if (typeof statement.columns === 'function') {
      columns = statement.columns().map((column) => ({
        name: String(column.name ?? column.column ?? ''),
        type_name: String(column.type ?? column.declType ?? ''),
        nullable: column.notnull !== 1,
      }));
    }
    // 先归一化再计预算：超限的行不进结果集，直接停取。
    const collect = (row) => {
      if (firstRow === undefined) firstRow = row;
      let cells;
      if (Array.isArray(row)) {
        cells = row.map(cell);
      } else if (row && typeof row === 'object') {
        cells = Object.values(row).map(cell);
      } else {
        cells = [cell(row)];
      }
      if (!budget.tryAdd(cells)) return false;
      normalized.push(cells);
      return true;
    };
    // 用 iterate 在 limit + 1 行处停止：SQL 自带更大的 LIMIT 时也不会先
    // 把整张表物化到内存。多取的第 limit + 1 行只用于判定 truncated。
    if (typeof statement.iterate === 'function') {
      for (const row of statement.iterate()) {
        if (normalized.length >= limit) {
          truncated = true;
          break;
        }
        if (!collect(row)) {
          truncated = true;
          break;
        }
      }
    } else {
      const rows = statement.all();
      truncated = rows.length > limit;
      for (const row of truncated ? rows.slice(0, limit) : rows) {
        if (!collect(row)) {
          truncated = true;
          break;
        }
      }
    }
  } catch (error) {
    throw dbRejected(`sqlite query failed: ${messageOf(error)}`);
  }

  if (!columns) {
    const names =
      firstRow && !Array.isArray(firstRow) && typeof firstRow === 'object'
        ? Object.keys(firstRow)
        : [];
    columns = names.map((name) => ({
      name,
      type_name: '',
      nullable: true,
    }));
  }
  const width = normalized[0]?.length ?? 0;
  while (columns.length < width) {
    columns.push({
      name: `column_${columns.length + 1}`,
      type_name: '',
      nullable: true,
    });
  }

  return {
    columns,
    rows: normalized,
    row_count: normalized.length,
    truncated,
    elapsed_ms: elapsed(started),
    untrusted: true,
  };
}

function sqliteSchema(entry) {
  let tables;
  try {
    tables = entry.db
      .prepare(
        "SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY type, name",
      )
      .all();
  } catch (error) {
    throw dbUnavailable(`cannot read sqlite schema: ${messageOf(error)}`);
  }

  const children = [];
  for (const table of tables ?? []) {
    const name = String(table.name);
    let columns = [];
    try {
      columns = entry.db
        .prepare(`PRAGMA table_info(${quoteIdentifier(name)})`)
        .all();
    } catch {
      columns = [];
    }
    children.push({
      name,
      kind: table.type === 'view' ? 'view' : 'table',
      detail: table.type === 'view' ? 'view' : 'table',
      children: (columns ?? []).map((column) => ({
        name: String(column.name),
        kind: 'column',
        detail: column.type ? String(column.type) : 'ANY',
        children: [],
      })),
    });
  }

  return {
    nodes: [
      {
        name: entry.database,
        kind: 'database',
        detail: 'sqlite',
        children,
      },
    ],
  };
}

function sqliteExplain(entry, sql, started) {
  let rows;
  try {
    rows = entry.db.prepare(`EXPLAIN QUERY PLAN ${sql}`).all();
  } catch (error) {
    throw dbRejected(`sqlite explain failed: ${messageOf(error)}`);
  }

  const plan = (rows ?? [])
    .map((row) => {
      if (row && typeof row === 'object' && row.detail !== undefined) {
        return String(row.detail);
      }
      return JSON.stringify(row);
    })
    .join('\n');

  return { plan, elapsed_ms: elapsed(started) };
}

export { CONNECT_TIMEOUT_MS };
