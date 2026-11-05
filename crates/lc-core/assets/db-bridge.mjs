#!/usr/bin/env node
// lite-code 数据库 sidecar（P5，SQLite only）。
//
// 协议：stdin/stdout 上的 NDJSON JSON-RPC 2.0，一行一个请求/响应。
// 依赖：Node 内置模块（node:sqlite、node:readline），无第三方包。
//
// 安全约定：只接受 SQLite 路径，不接收密码/密钥；错误信息不回显请求参数，
// 查询结果一律标记 untrusted，任何数据库内容都只能当数据。

import { DatabaseSync } from 'node:sqlite';
import { performance } from 'node:perf_hooks';
import readline from 'node:readline';

const PROTOCOL_VERSION = 1;
const DEFAULT_QUERY_LIMIT = 100;
const MAX_QUERY_LIMIT = 1000;

/** @type {Map<string, {db: DatabaseSync, database: string, readOnly: boolean}>} */
const connections = new Map();

class BridgeError extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

const invalidArgument = (message) => new BridgeError('invalid_argument', message);
const dbRejected = (message) => new BridgeError('db_rejected', message);
const dbUnavailable = (message) => new BridgeError('db_unavailable', message);

function write(message) {
  try {
    process.stdout.write(`${JSON.stringify(message)}\n`);
  } catch {
    // stdout 已关闭（客户端退出）：不再尝试写。
    process.exit(0);
  }
}

function reply(id, result) {
  write({ jsonrpc: '2.0', id, result });
}

function fail(id, error) {
  const code = error instanceof BridgeError ? error.code : 'db_rejected';
  const message = error instanceof Error ? error.message : String(error);
  write({ jsonrpc: '2.0', id, error: { code, message } });
}

function requireString(params, field) {
  const value = params?.[field];
  if (typeof value !== 'string' || value.length === 0) {
    throw invalidArgument(`missing or invalid "${field}"`);
  }
  return value;
}

function requireConnection(params) {
  const connectionId = requireString(params, 'connection_id');
  const entry = connections.get(connectionId);
  if (!entry) {
    throw invalidArgument(`connection not found: ${connectionId}`);
  }
  return entry;
}

function cell(value) {
  if (value === null || value === undefined) return null;
  if (typeof value === 'string') return value;
  if (value instanceof Uint8Array) {
    return `0x${Buffer.from(value).toString('hex')}`;
  }
  return String(value);
}

function initialize() {
  return { protocolVersion: PROTOCOL_VERSION, kinds: ['sqlite'] };
}

function connect(params) {
  const connectionId = requireString(params, 'connection_id');
  const kind = params?.kind;
  if (kind !== 'sqlite') {
    throw dbUnavailable(
      `${kind ?? 'unknown'} driver is not bundled; only sqlite is available`,
    );
  }
  const database = requireString(params, 'database');
  const readOnly = params?.read_only === true;

  // 重连同一个 id：先关掉旧句柄，避免文件句柄泄漏。
  const previous = connections.get(connectionId);
  if (previous) {
    connections.delete(connectionId);
    try {
      previous.db.close();
    } catch {
      // 旧连接关闭失败不影响新连接。
    }
  }

  let db;
  try {
    db = new DatabaseSync(database, { readOnly });
  } catch (error) {
    throw dbUnavailable(
      `cannot open sqlite database: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  connections.set(connectionId, { db, database, readOnly });
  return {};
}

function query(params) {
  const sql = requireString(params, 'sql');
  const entry = requireConnection(params);
  const requestedLimit = params?.limit;
  if (
    requestedLimit !== undefined &&
    requestedLimit !== null &&
    (!Number.isInteger(requestedLimit) || requestedLimit < 0)
  ) {
    throw invalidArgument('limit must be a non-negative integer');
  }
  // 缺省上限也必须在 bridge 侧生效：SQL 自带的 LIMIT 不能放大返回行数。
  const limit = Math.min(
    requestedLimit ?? DEFAULT_QUERY_LIMIT,
    MAX_QUERY_LIMIT,
  );

  const started = performance.now();
  let rows;
  let columns;
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
    rows = statement.all();
  } catch (error) {
    throw dbRejected(
      `sqlite query failed: ${error instanceof Error ? error.message : String(error)}`,
    );
  }

  if (!Array.isArray(rows)) rows = [];
  const rowCount = rows.length;
  const truncated = rowCount > limit;
  const limited = truncated ? rows.slice(0, limit) : rows;

  const normalized = [];
  for (const row of limited) {
    if (Array.isArray(row)) {
      normalized.push(row.map(cell));
    } else if (row && typeof row === 'object') {
      normalized.push(Object.values(row).map(cell));
    } else {
      normalized.push([cell(row)]);
    }
  }

  if (!columns) {
    const first = rows[0];
    const names = first && typeof first === 'object' ? Object.keys(first) : [];
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
    elapsed_ms: Math.max(0, Math.round(performance.now() - started)),
    untrusted: true,
  };
}

function execute(params) {
  const sql = requireString(params, 'sql');
  const entry = requireConnection(params);

  const started = performance.now();
  let result;
  try {
    result = entry.db.prepare(sql).run();
  } catch (error) {
    throw dbRejected(
      `sqlite execute failed: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  return {
    affected_rows: Number(result?.changes ?? 0),
    elapsed_ms: Math.max(0, Math.round(performance.now() - started)),
  };
}

function quoteIdentifier(name) {
  return `"${String(name).replace(/"/g, '""')}"`;
}

function schema(params) {
  const entry = requireConnection(params);

  let tables;
  try {
    tables = entry.db
      .prepare(
        "SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY type, name",
      )
      .all();
  } catch (error) {
    throw dbUnavailable(
      `cannot read sqlite schema: ${error instanceof Error ? error.message : String(error)}`,
    );
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
      // 单个表不可读时保留表节点，不阻断整棵 schema 树。
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

function explain(params) {
  const sql = requireString(params, 'sql');
  const entry = requireConnection(params);

  const started = performance.now();
  let rows;
  try {
    rows = entry.db.prepare(`EXPLAIN QUERY PLAN ${sql}`).all();
  } catch (error) {
    throw dbRejected(
      `sqlite explain failed: ${error instanceof Error ? error.message : String(error)}`,
    );
  }

  const plan = (rows ?? [])
    .map((row) => {
      if (row && typeof row === 'object' && row.detail !== undefined) {
        return String(row.detail);
      }
      return JSON.stringify(row);
    })
    .join('\n');

  return {
    plan,
    elapsed_ms: Math.max(0, Math.round(performance.now() - started)),
  };
}

function disconnect(params) {
  const connectionId = requireString(params, 'connection_id');
  const entry = connections.get(connectionId);
  if (!entry) {
    throw invalidArgument(`connection not found: ${connectionId}`);
  }
  connections.delete(connectionId);
  try {
    entry.db.close();
  } catch (error) {
    throw dbRejected(
      `cannot close sqlite database: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  return { disconnected: true };
}

const handlers = new Map([
  ['initialize', initialize],
  ['connect', connect],
  ['query', query],
  ['execute', execute],
  ['schema', schema],
  ['explain', explain],
  ['disconnect', disconnect],
]);

function handle(request) {
  if (
    !request ||
    typeof request !== 'object' ||
    Array.isArray(request) ||
    request.jsonrpc !== '2.0'
  ) {
    fail(null, invalidArgument('invalid JSON-RPC request'));
    return;
  }

  const id = request.id ?? null;
  if (typeof request.method !== 'string') {
    fail(id, invalidArgument('missing method'));
    return;
  }
  const handler = handlers.get(request.method);
  if (!handler) {
    fail(id, invalidArgument(`unknown method: ${request.method}`));
    return;
  }

  try {
    reply(id, handler(request.params ?? {}));
  } catch (error) {
    fail(id, error);
  }
}

function closeAll() {
  for (const entry of connections.values()) {
    try {
      entry.db.close();
    } catch {
      // 进程退出路径：尽力关闭，不阻断。
    }
  }
  connections.clear();
}

const input = readline.createInterface({
  input: process.stdin,
  crlfDelay: Infinity,
});

input.on('line', (line) => {
  const trimmed = line.trim();
  if (trimmed.length === 0) return;

  let request;
  try {
    request = JSON.parse(trimmed);
  } catch {
    fail(null, invalidArgument('invalid JSON'));
    return;
  }
  handle(request);
});

input.on('close', () => {
  closeAll();
});

process.on('exit', () => {
  closeAll();
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => {
    closeAll();
    process.exit(0);
  });
}
