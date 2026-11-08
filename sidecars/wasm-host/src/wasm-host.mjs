#!/usr/bin/env node
/**
 * Lite Code WASM command runner.
 *
 * Usage:
 *   node wasm-host.mjs --wasm <plugin.wasm> --command <id> --args <json>
 *                      --workspace <root> [--timeout <ms>]
 *
 * The module is executed as a WASI preview1 command in a worker thread:
 * no preopened directories, no sockets, an empty environment, and no stdin
 * data. The runner passes the command/args/workspace to the module through
 * argv:
 *
 *   [<wasm basename>, "--command", <id>, "--args", <json>,
 *    "--workspace", <root>]
 *
 * On success the single JSON value written to stdout is validated and then
 * printed unchanged. On failure a JSON error object is written to stderr and
 * the process exits with status 1. The default execution timeout is 10s; the
 * module runs in a worker so the timeout can actually terminate a busy loop.
 */

import { closeSync, mkdtempSync, openSync, readFileSync, rmSync, statSync } from 'node:fs';
import { devNull, tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { Worker, isMainThread, parentPort, workerData } from 'node:worker_threads';

const DEFAULT_TIMEOUT_MS = 10_000;
const USAGE =
  'Usage: node wasm-host.mjs --wasm <path> --command <id> --args <json> ' +
  '--workspace <root> [--timeout <ms>]';

class WasiHostError extends Error {
  constructor(code, message) {
    super(message);
    this.name = 'WasiHostError';
    this.code = code;
  }
}

if (isMainThread) {
  process.exitCode = await main();
} else {
  await runWorker();
}

async function main() {
  let options;
  try {
    options = parseArgs(process.argv.slice(2));
  } catch (error) {
    return emitError('usage_error', error.message, { usage: USAGE });
  }

  if (options.help) {
    process.stdout.write(`${USAGE}\n`);
    return 0;
  }

  try {
    if (!statSync(options.wasm).isFile()) {
      return emitError('wasm_not_found', `WASM module is not a file: ${options.wasm}`);
    }
  } catch {
    return emitError('wasm_not_found', `WASM module not found: ${options.wasm}`);
  }

  try {
    JSON.parse(options.args);
  } catch (error) {
    return emitError('invalid_args', `--args is not valid JSON: ${error.message}`);
  }

  const wasmArgv = [
    basename(options.wasm),
    '--command',
    options.command,
    '--args',
    options.args,
    '--workspace',
    options.workspace,
  ];

  let tempDir;
  let worker;

  try {
    tempDir = mkdtempSync(join(tmpdir(), 'lc-wasm-'));
    const stdoutPath = join(tempDir, 'stdout');
    const stderrPath = join(tempDir, 'stderr');

    worker = new Worker(new URL(import.meta.url), {
      workerData: {
        wasmPath: options.wasm,
        argv: wasmArgv,
        stdoutPath,
        stderrPath,
      },
      // node:wasi still emits an ExperimentalWarning on current Node versions.
      // Keep worker stderr clean so it can carry the module's error details.
      execArgv: ['--no-warnings'],
    });

    const outcome = await waitForWorker(worker, options.timeoutMs);

    // The worker closes its file descriptors before reporting; terminate is a
    // no-op for an exited worker and also guarantees a stuck worker is gone.
    await worker.terminate().catch(() => {});

    const stdout = readText(stdoutPath);
    const stderr = readText(stderrPath);

    if (outcome.kind === 'timeout') {
      return emitError(
        'wasm_timeout',
        `WASM execution timed out after ${options.timeoutMs} ms`,
        { stderr, ...(stdout ? { stdout } : {}) },
      );
    }

    if (outcome.kind === 'worker_error') {
      return emitError(
        'wasm_error',
        `WASM worker failed: ${outcome.error?.message ?? String(outcome.error)}`,
        { stderr, ...(stdout ? { stdout } : {}) },
      );
    }

    if (outcome.kind === 'exit') {
      if (outcome.code !== 0) {
        return exitError(outcome.code, stdout, stderr);
      }
      return emitError('wasm_error', 'WASM worker exited without a result', {
        stderr,
        ...(stdout ? { stdout } : {}),
      });
    }

    const message = outcome.message;
    if (message?.type === 'error') {
      return emitError(message.code ?? 'wasm_error', message.message ?? 'WASM execution failed', {
        stderr,
        ...(stdout ? { stdout } : {}),
      });
    }

    if (message?.type === 'done') {
      if (message.exitCode !== 0) {
        return exitError(message.exitCode, stdout, stderr);
      }
      return succeed(stdout, stderr);
    }

    return emitError('wasm_error', 'Unexpected WASM worker message', { stderr });
  } catch (error) {
    return emitError('internal_error', error?.stack ?? String(error));
  } finally {
    if (worker) {
      await worker.terminate().catch(() => {});
    }
    if (tempDir) {
      try {
        rmSync(tempDir, { recursive: true, force: true });
      } catch {
        // Temp cleanup must not replace the real command result.
      }
    }
  }
}

function parseArgs(argv) {
  const options = { timeoutMs: DEFAULT_TIMEOUT_MS };

  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];

    if (argument === '--help' || argument === '-h') {
      options.help = true;
      continue;
    }

    if (!argument.startsWith('--')) {
      throw new Error(`Unexpected argument: ${argument}`);
    }

    const equals = argument.indexOf('=');
    const name = equals === -1 ? argument : argument.slice(0, equals);
    let value;
    if (equals === -1) {
      value = argv[index + 1];
      index += 1;
      if (value === undefined) {
        throw new Error(`Missing value for ${name}`);
      }
    } else {
      value = argument.slice(equals + 1);
    }

    switch (name) {
      case '--wasm':
        options.wasm = value;
        break;
      case '--command':
        options.command = value;
        break;
      case '--args':
        options.args = value;
        break;
      case '--workspace':
        options.workspace = value;
        break;
      case '--timeout': {
        const timeoutMs = Number(value);
        if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0) {
          throw new Error(`--timeout must be a positive integer, got: ${value}`);
        }
        options.timeoutMs = timeoutMs;
        break;
      }
      default:
        throw new Error(`Unknown option: ${name}`);
    }
  }

  if (options.help) {
    return options;
  }

  for (const [flag, key] of [
    ['--wasm', 'wasm'],
    ['--command', 'command'],
    ['--args', 'args'],
    ['--workspace', 'workspace'],
  ]) {
    if (options[key] === undefined) {
      throw new Error(`Missing required option ${flag}`);
    }
  }

  if (!options.wasm) {
    throw new Error('--wasm must not be empty');
  }
  if (!options.command) {
    throw new Error('--command must not be empty');
  }
  if (!options.workspace) {
    throw new Error('--workspace must not be empty');
  }

  return options;
}

function waitForWorker(worker, timeoutMs) {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timer);
      // Keep the listeners attached: terminate() can emit an 'error' event
      // after the timeout path has already settled, and an unhandled Worker
      // error would take down the runner instead of producing error JSON.
      resolve(result);
    };
    const timer = setTimeout(() => finish({ kind: 'timeout' }), timeoutMs);
    const onMessage = (message) => finish({ kind: 'message', message });
    const onError = (error) => finish({ kind: 'worker_error', error });
    const onExit = (code) => finish({ kind: 'exit', code });

    worker.on('message', onMessage);
    worker.on('error', onError);
    worker.on('exit', onExit);
  });
}

function succeed(stdout, stderr) {
  const text = stripBom(stdout).trim();
  if (!text) {
    return emitError('wasm_invalid_stdout', 'WASM module wrote no JSON to stdout', { stderr });
  }

  try {
    JSON.parse(text);
  } catch (error) {
    return emitError(
      'wasm_invalid_stdout',
      `WASM stdout is not a single JSON value: ${error.message}`,
      { stdout: text, stderr },
    );
  }

  process.stdout.write(text.endsWith('\n') ? text : `${text}\n`);
  if (stderr) {
    process.stderr.write(stderr);
  }
  return 0;
}

function exitError(exitCode, stdout, stderr) {
  return emitError('wasm_exit_nonzero', `WASM command exited with code ${exitCode}`, {
    exitCode,
    stderr,
    ...(stdout ? { stdout } : {}),
  });
}

function emitError(code, message, details = {}) {
  const payload = { error: message, code };
  if (details.exitCode !== undefined) {
    payload.exitCode = details.exitCode;
  }
  if (details.usage !== undefined) {
    payload.usage = details.usage;
  }
  if (details.stderr !== undefined) {
    payload.stderr = details.stderr;
  }
  if (details.stdout !== undefined) {
    payload.stdout = details.stdout;
  }
  process.stderr.write(`${JSON.stringify(payload)}\n`);
  return 1;
}

function stripBom(text) {
  return text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
}

function readText(path) {
  try {
    return readFileSync(path, 'utf8');
  } catch {
    return '';
  }
}

async function runWorker() {
  const { wasmPath, argv, stdoutPath, stderrPath } = workerData;
  let stdinFd;
  let stdoutFd;
  let stderrFd;
  let message;

  try {
    suppressWasiWarning();
    const { WASI } = await import('node:wasi');

    const bytes = readFileSync(wasmPath);
    let module;
    try {
      module = new WebAssembly.Module(bytes);
    } catch (error) {
      throw new WasiHostError('wasm_compile_failed', `Failed to compile WASM module: ${error.message}`);
    }

    stdinFd = openSync(devNull, 'r');
    stdoutFd = openSync(stdoutPath, 'w+');
    stderrFd = openSync(stderrPath, 'w+');

    const wasi = new WASI({
      version: 'preview1',
      args: argv,
      env: {},
      preopens: {},
      stdin: stdinFd,
      stdout: stdoutFd,
      stderr: stderrFd,
      returnOnExit: true,
    });

    const imports = { ...wasi.wasiImport };
    for (const name of Object.keys(imports)) {
      // WASI preview1 command modules have no business with sockets, and
      // proc_raise could signal the host process instead of the module.
      if (name.startsWith('sock_') || name === 'proc_raise') {
        delete imports[name];
      }
    }

    let instance;
    try {
      instance = await WebAssembly.instantiate(module, {
        wasi_snapshot_preview1: imports,
      });
    } catch (error) {
      throw new WasiHostError(
        'wasm_instantiate_failed',
        `Failed to instantiate WASM module: ${error.message}`,
      );
    }

    let exitCode;
    try {
      exitCode = wasi.start(instance);
    } catch (error) {
      throw new WasiHostError('wasm_start_failed', `WASM command failed: ${error.message}`);
    }

    message = {
      type: 'done',
      exitCode: typeof exitCode === 'number' ? exitCode : 0,
    };
  } catch (error) {
    message = {
      type: 'error',
      code: error instanceof WasiHostError ? error.code : 'wasm_error',
      message: error?.message ?? String(error),
    };
  } finally {
    for (const fd of [stdinFd, stdoutFd, stderrFd]) {
      if (typeof fd === 'number') {
        try {
          closeSync(fd);
        } catch {
          // The module may have closed a stdio descriptor itself.
        }
      }
    }
  }

  parentPort.postMessage(message);
}

function suppressWasiWarning() {
  const original = process.emitWarning;
  process.emitWarning = function patchedEmitWarning(warning, ...rest) {
    const text = typeof warning === 'string' ? warning : warning?.message ?? '';
    const type = typeof rest[0] === 'string' ? rest[0] : rest[0]?.type;
    if (type === 'ExperimentalWarning' && /WASI/i.test(text)) {
      return;
    }
    return original.apply(process, [warning, ...rest]);
  };
}
