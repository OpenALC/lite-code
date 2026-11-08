import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { after, before, test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { HELLO_JSON, buildFixtures } from './fixtures/wasm-fixtures.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '../../..');
const runner = join(repoRoot, 'crates/lc-core/assets/wasm-host.mjs');
const source = join(repoRoot, 'sidecars/wasm-host/src/wasm-host.mjs');

let tempDir;
let fixtures;

before(() => {
  tempDir = mkdtempSync(join(tmpdir(), 'lc-wasm-host-test-'));
  fixtures = buildFixtures(tempDir);
});

after(() => {
  rmSync(tempDir, { recursive: true, force: true });
});

test('the committed asset matches the sidecar source', () => {
  assert.equal(readFileSync(runner, 'utf8'), readFileSync(source, 'utf8'));
});

test('the example plugin points at the generated fixture', () => {
  const pluginDir = join(repoRoot, 'examples/plugins/hello-wasm');
  const manifest = JSON.parse(readFileSync(join(pluginDir, 'plugin.json'), 'utf8'));
  assert.equal(manifest.runtime, 'wasm');
  assert.equal(manifest.contributes.commands[0].command, 'example.wasmHello');
  assert.deepEqual(readFileSync(resolve(pluginDir, manifest.main)), readFileSync(fixtures.hello));
});

test('runs a WASI preview1 command and prints its JSON result', () => {
  const result = runFixture('hello');
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, `${HELLO_JSON}\n`);
  assert.equal(result.stderr, '');
});

test('passes command, args and workspace to the module via argv', () => {
  const argsJson = '{ "n": 9007199254740993, "s": "hi \\"there\\"" }';
  const result = run([
    '--wasm',
    fixtures.echoArgs,
    '--command',
    'example.echo',
    '--args',
    argsJson,
    '--workspace',
    '/workspace with space/插件',
  ]);

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stderr, '');
  const payload = JSON.parse(result.stdout);
  assert.deepEqual(payload.argv, [
    basename(fixtures.echoArgs),
    '--command',
    'example.echo',
    '--args',
    argsJson,
    '--workspace',
    '/workspace with space/插件',
  ]);
});

test('reports a nonzero exit with stderr details', () => {
  const result = runFixture('stderrExit');
  assert.equal(result.status, 1);

  const error = parseError(result);
  assert.equal(error.code, 'wasm_exit_nonzero');
  assert.equal(error.exitCode, 3);
  assert.equal(error.stderr, 'fixture exploded');
  assert.equal(error.stdout, undefined);
});

test('rejects stdout that is not JSON', () => {
  const result = runFixture('badJson');
  assert.equal(result.status, 1);

  const error = parseError(result);
  assert.equal(error.code, 'wasm_invalid_stdout');
  assert.equal(error.stdout, 'not json');
  assert.equal(error.stderr, '');
});

test('rejects empty stdout', () => {
  const result = runFixture('empty');
  assert.equal(result.status, 1);

  const error = parseError(result);
  assert.equal(error.code, 'wasm_invalid_stdout');
  assert.match(error.error, /no JSON/);
});

test('rejects invalid --args JSON before starting the module', () => {
  const result = run([
    '--wasm',
    fixtures.hello,
    '--command',
    'example.test',
    '--args',
    'not-json',
    '--workspace',
    '/workspace',
  ]);

  assert.equal(result.status, 1);
  const error = parseError(result);
  assert.equal(error.code, 'invalid_args');
});

test('reports a missing wasm module', () => {
  const result = run([
    '--wasm',
    join(tempDir, 'missing.wasm'),
    '--command',
    'example.test',
    '--args',
    '{}',
    '--workspace',
    '/workspace',
  ]);

  assert.equal(result.status, 1);
  const error = parseError(result);
  assert.equal(error.code, 'wasm_not_found');
});

test('requires all CLI options', () => {
  const result = run(['--wasm', fixtures.hello]);
  assert.equal(result.status, 1);

  const error = parseError(result);
  assert.equal(error.code, 'usage_error');
  assert.match(error.usage, /--workspace/);
});

test('terminates a module that exceeds the timeout', () => {
  const result = runFixture('hang', ['--timeout', '150']);
  assert.equal(result.status, 1);

  const error = parseError(result);
  assert.equal(error.code, 'wasm_timeout');
  assert.match(error.error, /150 ms/);
});

test('reports a wasm trap', () => {
  const result = runFixture('trap');
  assert.equal(result.status, 1);

  const error = parseError(result);
  assert.equal(error.code, 'wasm_start_failed');
  assert.match(error.error, /unreachable/);
});

test('reports a module without _start', () => {
  const result = runFixture('noStart');
  assert.equal(result.status, 1);

  const error = parseError(result);
  assert.equal(error.code, 'wasm_start_failed');
  assert.match(error.error, /_start/);
});

test('does not provide socket imports', () => {
  const result = runFixture('socketImport');
  assert.equal(result.status, 1);

  const error = parseError(result);
  assert.equal(error.code, 'wasm_instantiate_failed');
  assert.match(error.error, /sock_accept/);
});

test('--help exits successfully', () => {
  const result = run(['--help']);
  assert.equal(result.status, 0);
  assert.match(result.stdout, /Usage:/);
  assert.equal(result.stderr, '');
});

function runFixture(name, extra = []) {
  return run([
    '--wasm',
    fixtures[name],
    '--command',
    'example.test',
    '--args',
    '{}',
    '--workspace',
    '/workspace',
    ...extra,
  ]);
}

function run(args) {
  return spawnSync(process.execPath, [runner, ...args], {
    encoding: 'utf8',
    timeout: 15_000,
    windowsHide: true,
  });
}

function parseError(result) {
  const lines = result.stderr.trim().split(/\r?\n/).filter(Boolean);
  assert.equal(lines.length, 1, `expected one stderr JSON line, got:\n${result.stderr}`);
  return JSON.parse(lines[0]);
}
