import assert from 'node:assert/strict';
import test from 'node:test';

import { createBridge } from '../src/bridge.mjs';
import { schemaSql } from '../src/config.mjs';

const PG_FIELDS = [
  { name: 'id', dataTypeID: 23 },
  { name: 'name', dataTypeID: 25 },
];

function baseCallState() {
  return {
    log: [],
    pg: {
      rows: [
        [1, 'alpha'],
        [2, null],
        [3, 'gamma'],
      ],
      fields: PG_FIELDS,
      affectedRows: 7,
      tables: [
        { table_schema: 'public', table_name: 'users', table_type: 'BASE TABLE' },
        { table_schema: 'public', table_name: 'user_view', table_type: 'VIEW' },
      ],
      columns: [
        {
          table_schema: 'public',
          table_name: 'users',
          column_name: 'id',
          data_type: 'integer',
          is_nullable: 'NO',
        },
        {
          table_schema: 'public',
          table_name: 'users',
          column_name: 'name',
          data_type: 'text',
          is_nullable: 'YES',
        },
      ],
      readOnly: false,
      closed: 0,
    },
    mysql: {
      rows: [['alpha'], [null]],
      fields: [
        { name: 'name', type: 253, flags: 1 },
        { name: 'note', type: 252, flags: 0 },
      ],
      affectedRows: 3,
      tables: [
        { table_schema: 'app', table_name: 'users', table_type: 'BASE TABLE' },
        { table_schema: 'app', table_name: 'user_view', table_type: 'VIEW' },
      ],
      columns: [
        {
          table_schema: 'app',
          table_name: 'users',
          column_name: 'id',
          data_type: 'int',
          is_nullable: 'NO',
        },
      ],
      closed: 0,
    },
    mssql: {
      rows: [
        [1, 'alpha'],
        [2, null],
      ],
      meta: [
        { name: 'id', type: { declaration: 'int' }, nullable: false },
        { name: 'name', type: { declaration: 'nvarchar(100)' }, nullable: true },
      ],
      affectedRows: [2, 1],
      tables: [
        { table_schema: 'dbo', table_name: 'users', table_type: 'BASE TABLE' },
      ],
      columns: [
        {
          table_schema: 'dbo',
          table_name: 'users',
          column_name: 'id',
          data_type: 'int',
          is_nullable: false,
        },
      ],
      showplan: false,
      closed: 0,
    },
  };
}

function pgModule(calls) {
  return {
    Client: class {
      constructor(options) {
        this.options = options;
        calls.pg.options = options;
      }

      async connect() {
        if (calls.pg.connectError) {
          throw new Error(
            `${calls.pg.connectError} (password ${this.options.password ?? ''})`,
          );
        }
      }

      async query(query) {
        const text = typeof query === 'string' ? query : query.text;
        const mode = typeof query === 'string' ? 'object' : query.rowMode;
        calls.log.push({ kind: 'postgres', text, mode });
        if (text === 'SHOW transaction_read_only') {
          return {
            rows: [{ transaction_read_only: calls.pg.readOnly ? 'on' : 'off' }],
            fields: [],
            rowCount: 1,
          };
        }
        if (text === 'SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY') {
          calls.pg.readOnly = true;
          return { rows: [], fields: [], rowCount: 0 };
        }
        if (text.startsWith('EXPLAIN ')) {
          return {
            rows: [{ 'QUERY PLAN': 'Seq Scan on users' }],
            fields: [{ name: 'QUERY PLAN' }],
            rowCount: 1,
          };
        }
        if (text.includes('information_schema.tables')) {
          return { rows: calls.pg.tables, fields: [], rowCount: calls.pg.tables.length };
        }
        if (text.includes('information_schema.columns')) {
          return {
            rows: calls.pg.columns,
            fields: [],
            rowCount: calls.pg.columns.length,
          };
        }
        if (mode === 'array') {
          return {
            rows: calls.pg.rows,
            fields: calls.pg.fields,
            rowCount: calls.pg.rows.length,
          };
        }
        return { rows: [], fields: [], rowCount: calls.pg.affectedRows };
      }

      async end() {
        calls.pg.closed += 1;
      }
    },
  };
}

function mysqlModule(calls) {
  return {
    async createConnection(options) {
      calls.mysql.options = options;
      if (calls.mysql.connectError) {
        throw new Error(calls.mysql.connectError);
      }
      return {
        async query(query) {
          if (typeof query === 'string') {
            calls.log.push({ kind: 'mysql', text: query, mode: 'object' });
            if (query === 'SET SESSION TRANSACTION READ ONLY') {
              calls.mysql.readOnly = true;
              return [{ affectedRows: 0 }, []];
            }
            if (query.includes('information_schema.TABLES')) {
              return [calls.mysql.tables, []];
            }
            if (query.includes('information_schema.COLUMNS')) {
              return [calls.mysql.columns, []];
            }
            if (query.startsWith('EXPLAIN ')) {
              return [
                [{ id: 1, select_type: 'SIMPLE', table: 'users', type: 'ALL' }],
                [],
              ];
            }
            return [{ affectedRows: calls.mysql.affectedRows }, []];
          }
          calls.log.push({
            kind: 'mysql',
            text: query.sql,
            mode: query.rowsAsArray ? 'array' : 'object',
          });
          assert.equal(query.rowsAsArray, true);
          return [calls.mysql.rows, calls.mysql.fields];
        },
        async end() {
          calls.mysql.closed += 1;
        },
      };
    },
  };
}

function mssqlModule(calls) {
  return {
    ConnectionPool: class {
      constructor(options) {
        calls.mssql.options = options;
      }

      async connect() {
        if (calls.mssql.connectError) {
          throw new Error(calls.mssql.connectError);
        }
      }

      request() {
        return {
          arrayRowMode: false,
          async query(sql) {
            const mode = this.arrayRowMode ? 'array' : 'object';
            calls.log.push({ kind: 'mssql', text: sql, mode });
            if (sql === 'SET SHOWPLAN_TEXT ON') {
              calls.mssql.showplan = true;
              return { recordset: [], rowsAffected: [0] };
            }
            if (sql === 'SET SHOWPLAN_TEXT OFF') {
              calls.mssql.showplan = false;
              return { recordset: [], rowsAffected: [0] };
            }
            if (calls.mssql.showplan) {
              return {
                recordset: [{ StmtText: 'Clustered Index Scan (users)' }],
                rowsAffected: [],
              };
            }
            if (sql.includes('sys.objects AS o')) {
              return { recordset: calls.mssql.tables, rowsAffected: [] };
            }
            if (sql.includes('sys.columns AS c')) {
              return { recordset: calls.mssql.columns, rowsAffected: [] };
            }
            if (calls.mssql.forcedError) {
              throw new Error('Invalid object name users');
            }
            if (mode === 'array') {
              return {
                recordset: calls.mssql.rows,
                columns: [calls.mssql.meta],
                rowsAffected: [calls.mssql.rows.length],
              };
            }
            return { recordset: [], rowsAffected: calls.mssql.affectedRows };
          },
        };
      }

      async close() {
        calls.mssql.closed += 1;
      }
    },
  };
}

function setup({ env = {}, available, connectError } = {}) {
  const calls = baseCallState();
  if (connectError?.kind) {
    calls[connectError.kind].connectError = connectError.message;
  }
  const modules = {
    postgres: pgModule(calls),
    mysql: mysqlModule(calls),
    mssql: mssqlModule(calls),
  };
  const bridge = createBridge({
    env,
    loadDrivers: async () => ({
      available: available ?? modules,
      errors: available ? { postgres: 'pg: not bundled' } : {},
    }),
  });
  return { bridge, calls, modules };
}

const pgConnect = (overrides = {}) => ({
  connection_id: 'pg',
  name: 'pg',
  kind: 'postgres',
  host: 'db.local',
  port: 5432,
  database: 'app',
  user: 'app',
  secret_ref: '',
  read_only: false,
  ...overrides,
});

const mysqlConnect = (overrides = {}) => ({
  connection_id: 'my',
  kind: 'mysql',
  host: 'mysql.local',
  port: 3306,
  database: 'app',
  user: 'app',
  secret_ref: '',
  read_only: false,
  ...overrides,
});

const mssqlConnect = (overrides = {}) => ({
  connection_id: 'ms',
  kind: 'mssql',
  host: 'mssql.local',
  port: 1433,
  database: 'app',
  user: 'app',
  secret_ref: '',
  read_only: false,
  ...overrides,
});

test('initialize advertises sqlite and bundled drivers', async () => {
  const { bridge } = setup();
  assert.deepEqual(await bridge.initialize(), {
    protocolVersion: 1,
    kinds: ['sqlite', 'postgres', 'mysql', 'mssql'],
  });
});

test('missing driver is db_unavailable with an explicit bundle message', async () => {
  const { bridge } = setup({ available: {} });
  assert.deepEqual((await bridge.initialize()).kinds, ['sqlite']);
  await assert.rejects(bridge.connect(pgConnect()), (error) => {
    assert.equal(error.code, 'db_unavailable');
    assert.match(error.message, /驱动未打包/);
    assert.match(error.message, /pg/);
    return true;
  });
});

test('postgres maps connection params, resolves env secret and sets read-only', async () => {
  const { bridge, calls } = setup({ env: { PGPASS: 's3cret' } });
  await bridge.connect(
    pgConnect({
      secret_ref: 'env:PGPASS',
      read_only: true,
      extra: '{"ssl":false,"application_name":"lite-test"}',
    }),
  );

  const options = calls.pg.options;
  assert.equal(options.host, 'db.local');
  assert.equal(options.port, 5432);
  assert.equal(options.database, 'app');
  assert.equal(options.user, 'app');
  assert.equal(options.password, 's3cret');
  assert.equal(options.connectionTimeoutMillis, 10_000);
  assert.equal(options.ssl, false);
  assert.equal(options.application_name, 'lite-test');
  assert.equal(calls.pg.readOnly, true);
  assert.ok(
    calls.log.some(
      (entry) =>
        entry.text === 'SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY',
    ),
  );
  assert.ok(
    calls.log.some((entry) => entry.text === 'SHOW transaction_read_only'),
  );
});

test('mysql maps connection params and enables session read-only', async () => {
  const { bridge, calls } = setup({ env: { MYPASS: 'mysql-secret' } });
  await bridge.connect(
    mysqlConnect({
      secret_ref: 'env:MYPASS',
      read_only: true,
      extra: '{"charset":"utf8mb4"}',
    }),
  );

  const options = calls.mysql.options;
  assert.equal(options.host, 'mysql.local');
  assert.equal(options.port, 3306);
  assert.equal(options.database, 'app');
  assert.equal(options.user, 'app');
  assert.equal(options.password, 'mysql-secret');
  assert.equal(options.connectTimeout, 10_000);
  assert.equal(options.supportBigNumbers, true);
  assert.equal(options.bigNumberStrings, true);
  assert.equal(options.multipleStatements, false);
  assert.equal(options.charset, 'utf8mb4');
  assert.equal(calls.mysql.readOnly, true);
  assert.ok(
    calls.log.some((entry) => entry.text === 'SET SESSION TRANSACTION READ ONLY'),
  );
});

test('mssql maps connection params to ConnectionPool and readOnlyIntent', async () => {
  const { bridge, calls } = setup({ env: { MSPASS: 'mssql-secret' } });
  await bridge.connect(
    mssqlConnect({
      secret_ref: 'env:MSPASS',
      read_only: true,
      extra: '{"encrypt":false,"trustServerCertificate":true,"pool":{"max":1}}',
    }),
  );

  const options = calls.mssql.options;
  assert.equal(options.server, 'mssql.local');
  assert.equal(options.port, 1433);
  assert.equal(options.database, 'app');
  assert.equal(options.user, 'app');
  assert.equal(options.password, 'mssql-secret');
  assert.equal(options.connectionTimeout, 10_000);
  assert.equal(options.pool.max, 1);
  assert.equal(options.options.encrypt, false);
  assert.equal(options.options.trustServerCertificate, true);
  assert.equal(options.options.readOnlyIntent, true);
  assert.equal(options.options.appName, 'lite-code');
});

test('missing env secret is db_unavailable and never invents a password', async () => {
  const { bridge } = setup({ env: {} });
  await assert.rejects(
    bridge.connect(pgConnect({ secret_ref: 'env:NOT_SET' })),
    (error) => {
      assert.equal(error.code, 'db_unavailable');
      assert.match(error.message, /NOT_SET/);
      return true;
    },
  );
});

test('keyring secret_ref is rejected until SecretStore exists', async () => {
  const { bridge } = setup();
  await assert.rejects(
    bridge.connect(pgConnect({ secret_ref: 'keyring://lite-code/pg' })),
    (error) => {
      assert.equal(error.code, 'db_unavailable');
      assert.match(error.message, /SecretStore/);
      return true;
    },
  );
});

test('connect failures are readable and sanitize the password', async () => {
  const { bridge } = setup({
    env: { PW: 'topsecret' },
    connectError: {
      kind: 'pg',
      message: 'password authentication failed for user "app"',
    },
  });
  await assert.rejects(
    bridge.connect(pgConnect({ secret_ref: 'env:PW' })),
    (error) => {
      assert.equal(error.code, 'db_unavailable');
      assert.match(error.message, /password authentication failed/);
      assert.ok(!error.message.includes('topsecret'));
      assert.match(error.message, /\*\*\*/);
      return true;
    },
  );
});

test('postgres query maps fields, rows, null, limit and truncated', async () => {
  const { bridge, calls } = setup();
  await bridge.connect(pgConnect());
  calls.log.length = 0;

  const result = await bridge.query({
    connection_id: 'pg',
    sql: 'SELECT id, name FROM users',
    limit: 2,
  });

  assert.deepEqual(
    result.columns.map((column) => [column.name, column.type_name]),
    [
      ['id', 'int4'],
      ['name', 'text'],
    ],
  );
  assert.deepEqual(result.rows, [
    ['1', 'alpha'],
    ['2', null],
  ]);
  assert.equal(result.row_count, 2);
  assert.equal(result.truncated, true);
  assert.equal(result.untrusted, true);
  assert.deepEqual(calls.log[0], {
    kind: 'postgres',
    text: 'SELECT id, name FROM users',
    mode: 'array',
  });
});

test('mysql query uses rowsAsArray and maps field metadata', async () => {
  const { bridge, calls } = setup();
  await bridge.connect(mysqlConnect());
  calls.log.length = 0;

  const result = await bridge.query({
    connection_id: 'my',
    sql: 'SELECT name, note FROM users',
    limit: 10,
  });

  assert.deepEqual(
    result.columns.map((column) => [column.name, column.type_name, column.nullable]),
    [
      ['name', 'varchar', false],
      ['note', 'blob', true],
    ],
  );
  assert.deepEqual(result.rows, [['alpha'], [null]]);
  assert.equal(result.truncated, false);
  assert.equal(result.untrusted, true);
  assert.equal(calls.log[0].mode, 'array');
});

test('mssql query uses arrayRowMode and maps column metadata', async () => {
  const { bridge, calls } = setup();
  await bridge.connect(mssqlConnect());
  calls.log.length = 0;

  const result = await bridge.query({
    connection_id: 'ms',
    sql: 'SELECT id, name FROM dbo.users',
    limit: 10,
  });

  assert.deepEqual(
    result.columns.map((column) => [column.name, column.type_name, column.nullable]),
    [
      ['id', 'int', false],
      ['name', 'nvarchar(100)', true],
    ],
  );
  assert.deepEqual(result.rows, [
    ['1', 'alpha'],
    ['2', null],
  ]);
  assert.equal(result.untrusted, true);
  assert.equal(calls.log[0].mode, 'array');
});

test('read-only connections reject execute in the bridge', async () => {
  const { bridge, calls } = setup();
  await bridge.connect(pgConnect({ read_only: true }));
  calls.log.length = 0;

  await assert.rejects(
    bridge.execute({ connection_id: 'pg', sql: 'DELETE FROM users WHERE id = 1' }),
    (error) => {
      assert.equal(error.code, 'db_read_only');
      assert.match(error.message, /只读/);
      return true;
    },
  );
  assert.equal(calls.log.length, 0, '写语句不得到达驱动');
});

test('execute returns affected rows per driver', async () => {
  const { bridge } = setup();
  await bridge.connect(pgConnect());
  await bridge.connect(mysqlConnect());
  await bridge.connect(mssqlConnect());

  assert.equal(
    (
      await bridge.execute({
        connection_id: 'pg',
        sql: 'UPDATE users SET name = $1 WHERE id = 1',
      })
    ).affected_rows,
    7,
  );
  assert.equal(
    (
      await bridge.execute({
        connection_id: 'my',
        sql: 'UPDATE users SET name = ? WHERE id = 1',
      })
    ).affected_rows,
    3,
  );
  assert.equal(
    (
      await bridge.execute({
        connection_id: 'ms',
        sql: 'UPDATE users SET name = @p1 WHERE id = 1',
      })
    ).affected_rows,
    3,
  );
});

test('schema uses information_schema for postgres and builds a schema tree', async () => {
  const { bridge, calls } = setup();
  await bridge.connect(pgConnect());
  calls.log.length = 0;

  const result = await bridge.schema({ connection_id: 'pg' });
  assert.deepEqual(
    calls.log.map((entry) => entry.text),
    [schemaSql('postgres').tables, schemaSql('postgres').columns],
  );

  const root = result.nodes[0];
  assert.equal(root.name, 'app');
  assert.equal(root.kind, 'database');
  assert.equal(root.children[0].name, 'public');
  assert.equal(root.children[0].kind, 'schema');
  assert.deepEqual(
    root.children[0].children.map((node) => [node.name, node.kind]),
    [
      ['users', 'table'],
      ['user_view', 'view'],
    ],
  );
  assert.deepEqual(
    root.children[0].children[0].children.map((node) => [
      node.name,
      node.detail,
    ]),
    [
      ['id', 'integer'],
      ['name', 'text'],
    ],
  );
});

test('schema uses information_schema for mysql and sys.* for mssql', async () => {
  for (const kind of ['mysql', 'mssql']) {
    const { bridge, calls } = setup();
    const connect = kind === 'mysql' ? mysqlConnect() : mssqlConnect();
    await bridge.connect(connect);
    calls.log.length = 0;

    const result = await bridge.schema({ connection_id: connect.connection_id });
    assert.deepEqual(
      calls.log.map((entry) => entry.text),
      [schemaSql(kind).tables, schemaSql(kind).columns],
      kind,
    );
    assert.equal(result.nodes[0].name, 'app');
    if (kind === 'mysql') {
      assert.equal(result.nodes[0].children[0].name, 'users');
      assert.equal(result.nodes[0].children[1].kind, 'view');
    } else {
      assert.equal(result.nodes[0].children[0].name, 'dbo');
      assert.equal(result.nodes[0].children[0].kind, 'schema');
      assert.equal(result.nodes[0].children[0].children[0].name, 'users');
    }
  }
});

test('explain returns driver plans and mssql toggles SHOWPLAN_TEXT', async () => {
  const pg = setup();
  await pg.bridge.connect(pgConnect());
  const pgPlan = await pg.bridge.explain({
    connection_id: 'pg',
    sql: 'SELECT * FROM users',
  });
  assert.match(pgPlan.plan, /Seq Scan on users/);
  assert.ok(pg.calls.log.some((entry) => entry.text === 'EXPLAIN SELECT * FROM users'));

  const my = setup();
  await my.bridge.connect(mysqlConnect());
  const myPlan = await my.bridge.explain({
    connection_id: 'my',
    sql: 'SELECT * FROM users',
  });
  assert.match(myPlan.plan, /select_type: SIMPLE/);
  assert.ok(my.calls.log.some((entry) => entry.text === 'EXPLAIN SELECT * FROM users'));

  const ms = setup();
  await ms.bridge.connect(mssqlConnect());
  ms.calls.log.length = 0;
  const msPlan = await ms.bridge.explain({
    connection_id: 'ms',
    sql: 'SELECT * FROM dbo.users',
  });
  assert.match(msPlan.plan, /Clustered Index Scan/);
  assert.deepEqual(
    ms.calls.log.map((entry) => entry.text),
    [
      'SET SHOWPLAN_TEXT ON',
      'SELECT * FROM dbo.users',
      'SET SHOWPLAN_TEXT OFF',
    ],
  );
});

test('driver query failures map to db_rejected and stay readable', async () => {
  const { bridge, calls } = setup();
  await bridge.connect(mssqlConnect());
  calls.mssql.forcedError = true;
  await assert.rejects(
    bridge.query({ connection_id: 'ms', sql: 'SELECT * FROM missing' }),
    (error) => {
      assert.equal(error.code, 'db_rejected');
      assert.match(error.message, /Invalid object name/);
      return true;
    },
  );
});

test('invalid extra, limit and unknown connection are invalid_argument', async () => {
  const { bridge } = setup();
  await assert.rejects(
    bridge.connect(pgConnect({ extra: '{not json' })),
    (error) => error.code === 'invalid_argument',
  );
  await bridge.connect(pgConnect());
  await assert.rejects(
    bridge.query({ connection_id: 'pg', sql: 'SELECT 1', limit: -1 }),
    (error) => error.code === 'invalid_argument',
  );
  await assert.rejects(
    bridge.query({ connection_id: 'missing', sql: 'SELECT 1' }),
    (error) => error.code === 'invalid_argument',
  );
});

test('disconnect closes the driver and clears the connection', async () => {
  const { bridge, calls } = setup();
  await bridge.connect(pgConnect());
  assert.deepEqual(await bridge.disconnect({ connection_id: 'pg' }), {
    disconnected: true,
  });
  assert.equal(calls.pg.closed, 1);
  await assert.rejects(
    bridge.query({ connection_id: 'pg', sql: 'SELECT 1' }),
    (error) => error.code === 'invalid_argument',
  );
});
