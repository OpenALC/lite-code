import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import readline from 'node:readline';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const bundle = fileURLToPath(
  new URL('../../../crates/lc-core/assets/db-bridge.mjs', import.meta.url),
);

test('built bundle starts, declares drivers and runs a SQLite query', async () => {
  const child = spawn(process.execPath, [bundle], {
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stderr = '';
  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });

  const lines = [];
  const reader = readline.createInterface({ input: child.stdout });
  reader.on('line', (line) => {
    if (line.trim() !== '') lines.push(JSON.parse(line));
  });

  const send = (id, method, params) => {
    child.stdin.write(
      `${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`,
    );
  };

  send(1, 'initialize', {});
  send(2, 'connect', {
    connection_id: 'mem',
    kind: 'sqlite',
    database: ':memory:',
    read_only: false,
  });
  send(3, 'query', {
    connection_id: 'mem',
    sql: 'SELECT 1 AS n',
    limit: 10,
  });
  send(4, 'disconnect', { connection_id: 'mem' });
  child.stdin.end();

  const code = await new Promise((resolve) => {
    child.on('close', resolve);
  });
  assert.equal(code, 0, stderr);

  const byId = new Map(lines.map((line) => [line.id, line]));
  assert.deepEqual(byId.get(1)?.result?.kinds, [
    'sqlite',
    'postgres',
    'mysql',
    'mssql',
  ]);
  assert.deepEqual(byId.get(2)?.result, {});
  assert.equal(byId.get(3)?.result?.rows?.[0]?.[0], '1');
  assert.deepEqual(byId.get(4)?.result, { disconnected: true });
});
