/**
 * Minimal WASI preview1 command fixtures, assembled by hand.
 *
 * The fixtures have no toolchain dependency: the binary encoding below is the
 * WebAssembly core format plus the four preview1 imports needed by the tests.
 * Run this file to (re)generate the example plugin's module:
 *
 *   node sidecars/wasm-host/test/fixtures/wasm-fixtures.mjs \
 *     examples/plugins/hello-wasm hello
 *
 * Tests call {@link buildFixtures} with a temporary directory.
 */

import { mkdirSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const I32 = 0x7f;
const WASI_MODULE = 'wasi_snapshot_preview1';

export const HELLO_JSON = '{"ok":true,"runtime":"wasm","message":"hello from wasm"}';

export function buildFixtures(outputDir, only) {
  const dir = resolve(outputDir);
  mkdirSync(dir, { recursive: true });
  const wanted = only && only.length > 0 ? new Set(only) : null;
  const fixtures = [
    ['hello', 'hello.wasm', () => buildFixedOutput(1, HELLO_JSON)],
    ['echoArgs', 'echo-args.wasm', buildEchoArgs],
    ['stderrExit', 'stderr-exit.wasm', () => buildFixedOutput(2, 'fixture exploded', 3)],
    ['badJson', 'bad-json.wasm', () => buildFixedOutput(1, 'not json')],
    ['empty', 'empty.wasm', () => buildFixedOutput(1, '')],
    ['hang', 'hang.wasm', buildHang],
    ['trap', 'trap.wasm', buildTrap],
    ['noStart', 'no-start.wasm', buildNoStart],
    ['socketImport', 'socket-import.wasm', buildSocketImport],
  ];

  const files = {};
  for (const [name, filename, build] of fixtures) {
    if (wanted && !wanted.has(name)) {
      continue;
    }
    const file = join(dir, filename);
    writeFileSync(file, build());
    files[name] = file;
  }
  return files;
}

class WasmBuilder {
  constructor() {
    this.types = [];
    this.imports = [];
    this.functions = [];
    this.memoryPages = 0;
    this.globals = [];
    this.exports = [];
    this.dataSegments = [];
  }

  type(params, results) {
    this.types.push({ params, results });
    return this.types.length - 1;
  }

  importFunc(module, name, typeIndex) {
    this.imports.push({ module, name, typeIndex });
    return this.imports.length - 1;
  }

  function(typeIndex, locals, body) {
    this.functions.push({ typeIndex, locals, body });
    return this.imports.length + this.functions.length - 1;
  }

  memory(pages) {
    this.memoryPages = pages;
  }

  global(type, mutable, init) {
    this.globals.push({ type, mutable, init });
    return this.globals.length - 1;
  }

  exportFunc(name, index) {
    this.exports.push({ name, kind: 0x00, index });
  }

  exportMemory(name, index) {
    this.exports.push({ name, kind: 0x02, index });
  }

  data(offset, value) {
    const bytes = typeof value === 'string' ? [...new TextEncoder().encode(value)] : [...value];
    this.dataSegments.push({ offset, bytes });
  }

  build() {
    const parts = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

    if (this.types.length > 0) {
      const payload = [u32(this.types.length)];
      for (const type of this.types) {
        payload.push(0x60, ...u32(type.params.length), ...type.params);
        payload.push(...u32(type.results.length), ...type.results);
      }
      parts.push(...section(1, payload));
    }

    if (this.imports.length > 0) {
      const payload = [u32(this.imports.length)];
      for (const item of this.imports) {
        payload.push(...name(item.module), ...name(item.name), 0x00, ...u32(item.typeIndex));
      }
      parts.push(...section(2, payload));
    }

    if (this.functions.length > 0) {
      const payload = [u32(this.functions.length)];
      for (const func of this.functions) {
        payload.push(...u32(func.typeIndex));
      }
      parts.push(...section(3, payload));
    }

    if (this.memoryPages > 0) {
      parts.push(...section(5, [0x01, 0x00, ...u32(this.memoryPages)]));
    }

    if (this.globals.length > 0) {
      const payload = [u32(this.globals.length)];
      for (const global of this.globals) {
        payload.push(global.type, global.mutable ? 0x01 : 0x00, ...global.init, 0x0b);
      }
      parts.push(...section(6, payload));
    }

    if (this.exports.length > 0) {
      const payload = [u32(this.exports.length)];
      for (const item of this.exports) {
        payload.push(...name(item.name), item.kind, ...u32(item.index));
      }
      parts.push(...section(7, payload));
    }

    if (this.functions.length > 0) {
      const bodies = [];
      for (const func of this.functions) {
        const locals = [];
        for (const [count, type] of func.locals) {
          locals.push(...u32(count), type);
        }
        const body = [...u32(func.locals.length), ...locals, ...func.body, 0x0b];
        bodies.push(...u32(body.length), ...body);
      }
      parts.push(...section(10, [u32(this.functions.length), ...bodies]));
    }

    if (this.dataSegments.length > 0) {
      const payload = [u32(this.dataSegments.length)];
      for (const segment of this.dataSegments) {
        payload.push(0x00, 0x41, ...i32(segment.offset), 0x0b);
        payload.push(...u32(segment.bytes.length), ...segment.bytes);
      }
      parts.push(...section(11, payload));
    }

    return Buffer.from(parts);
  }
}

function buildFixedOutput(fd, text, exitCode) {
  const builder = new WasmBuilder();
  const typeVoid = builder.type([], []);
  const typeOneI32 = builder.type([I32], []);
  const typeFdWrite = builder.type([I32, I32, I32, I32], [I32]);
  const fdWrite = builder.importFunc(WASI_MODULE, 'fd_write', typeFdWrite);
  const procExit = builder.importFunc(WASI_MODULE, 'proc_exit', typeOneI32);
  const bytes = [...new TextEncoder().encode(text)];
  const messageOffset = 16;

  builder.memory(1);
  builder.data(messageOffset, bytes);

  const body = [
    ...op.i32Const(0),
    ...op.i32Const(messageOffset),
    ...op.i32Store(),
    ...op.i32Const(4),
    ...op.i32Const(bytes.length),
    ...op.i32Store(),
    ...op.i32Const(fd),
    ...op.i32Const(0),
    ...op.i32Const(1),
    ...op.i32Const(8),
    ...op.call(fdWrite),
    ...op.drop(),
  ];
  if (exitCode !== undefined) {
    body.push(...op.i32Const(exitCode), ...op.call(procExit));
  }

  const start = builder.function(typeVoid, [], body);
  builder.exportMemory('memory', 0);
  builder.exportFunc('_start', start);
  return builder.build();
}

function buildEchoArgs() {
  const builder = new WasmBuilder();
  const outBase = 65_536;
  const hexBase = 196_608;
  const argvPointers = 16;
  const argvBuffer = 1_024;

  const typeVoid = builder.type([], []);
  const typeOneI32 = builder.type([I32], []);
  const typeFdWrite = builder.type([I32, I32, I32, I32], [I32]);
  const typeTwoI32 = builder.type([I32, I32], [I32]);
  const fdWrite = builder.importFunc(WASI_MODULE, 'fd_write', typeFdWrite);
  builder.importFunc(WASI_MODULE, 'proc_exit', typeOneI32);
  const argsSizesGet = builder.importFunc(WASI_MODULE, 'args_sizes_get', typeTwoI32);
  const argsGet = builder.importFunc(WASI_MODULE, 'args_get', typeTwoI32);

  const outLength = builder.global(I32, true, op.i32Const(0));
  builder.memory(4);
  const prefix = '{"argv":[';
  builder.data(outBase, prefix);
  builder.data(hexBase, '0123456789abcdef');

  const put = builder.function(
    typeOneI32,
    [],
    [
      ...op.i32Const(outBase),
      ...op.globalGet(outLength),
      ...op.i32Add(),
      ...op.localGet(0),
      ...op.i32Store8(),
      ...op.globalGet(outLength),
      ...op.i32Const(1),
      ...op.i32Add(),
      ...op.globalSet(outLength),
    ],
  );

  const startBody = [
    // args_sizes_get(&argc, &argv_buf_size)
    ...op.i32Const(0),
    ...op.i32Const(4),
    ...op.call(argsSizesGet),
    ...op.drop(),
    // args_get(argv, argv_buf)
    ...op.i32Const(argvPointers),
    ...op.i32Const(argvBuffer),
    ...op.call(argsGet),
    ...op.drop(),
    // The JSON prefix is pre-seeded at outBase.
    ...op.i32Const(prefix.length),
    ...op.globalSet(outLength),
    ...op.block(),
    ...op.loop(),
    // while (i < argc)
    ...op.localGet(0),
    ...op.i32Const(0),
    ...op.i32Load(),
    ...op.i32GeU(),
    ...op.brIf(1),
    // if (i > 0) put(',')
    ...op.localGet(0),
    ...op.i32Const(0),
    ...op.i32GtU(),
    ...op.if(),
    ...op.i32Const(0x2c),
    ...op.call(put),
    ...op.end(),
    ...op.i32Const(0x22),
    ...op.call(put),
    // ptr = argv[i]
    ...op.i32Const(argvPointers),
    ...op.localGet(0),
    ...op.i32Const(4),
    ...op.i32Mul(),
    ...op.i32Add(),
    ...op.i32Load(),
    ...op.localSet(1),
    ...op.block(),
    ...op.loop(),
    // c = *ptr; if (c == 0) break
    ...op.localGet(1),
    ...op.i32Load8U(),
    ...op.localTee(2),
    ...op.i32Eqz(),
    ...op.brIf(1),
    // if (c < 0x20) write \u00XX
    ...op.localGet(2),
    ...op.i32Const(0x20),
    ...op.i32LtU(),
    ...op.if(),
    ...op.i32Const(0x5c),
    ...op.call(put),
    ...op.i32Const(0x75),
    ...op.call(put),
    ...op.i32Const(0x30),
    ...op.call(put),
    ...op.i32Const(0x30),
    ...op.call(put),
    ...op.i32Const(hexBase),
    ...op.localGet(2),
    ...op.i32Const(4),
    ...op.i32ShrU(),
    ...op.i32Const(0x0f),
    ...op.i32And(),
    ...op.i32Add(),
    ...op.i32Load8U(),
    ...op.call(put),
    ...op.i32Const(hexBase),
    ...op.localGet(2),
    ...op.i32Const(0x0f),
    ...op.i32And(),
    ...op.i32Add(),
    ...op.i32Load8U(),
    ...op.call(put),
    ...op.else(),
    // if (c == '"' || c == '\\') put('\\')
    ...op.localGet(2),
    ...op.i32Const(0x22),
    ...op.i32Eq(),
    ...op.localGet(2),
    ...op.i32Const(0x5c),
    ...op.i32Eq(),
    ...op.i32Or(),
    ...op.if(),
    ...op.i32Const(0x5c),
    ...op.call(put),
    ...op.end(),
    ...op.localGet(2),
    ...op.call(put),
    ...op.end(),
    ...op.localGet(1),
    ...op.i32Const(1),
    ...op.i32Add(),
    ...op.localSet(1),
    ...op.br(0),
    ...op.end(),
    ...op.end(),
    ...op.i32Const(0x22),
    ...op.call(put),
    ...op.localGet(0),
    ...op.i32Const(1),
    ...op.i32Add(),
    ...op.localSet(0),
    ...op.br(0),
    ...op.end(),
    ...op.end(),
    ...op.i32Const(0x5d),
    ...op.call(put),
    ...op.i32Const(0x7d),
    ...op.call(put),
    // fd_write(stdout, iovec, 1, &nwritten)
    ...op.i32Const(0),
    ...op.i32Const(outBase),
    ...op.i32Store(),
    ...op.i32Const(4),
    ...op.globalGet(outLength),
    ...op.i32Store(),
    ...op.i32Const(1),
    ...op.i32Const(0),
    ...op.i32Const(1),
    ...op.i32Const(8),
    ...op.call(fdWrite),
    ...op.drop(),
  ];

  const start = builder.function(typeVoid, [[3, I32]], startBody);
  builder.exportMemory('memory', 0);
  builder.exportFunc('_start', start);
  return builder.build();
}

function buildHang() {
  const builder = new WasmBuilder();
  const typeVoid = builder.type([], []);
  builder.memory(1);
  const start = builder.function(typeVoid, [], [...op.loop(), ...op.br(0), ...op.end()]);
  builder.exportMemory('memory', 0);
  builder.exportFunc('_start', start);
  return builder.build();
}

function buildTrap() {
  const builder = new WasmBuilder();
  const typeVoid = builder.type([], []);
  builder.memory(1);
  const start = builder.function(typeVoid, [], [...op.unreachable()]);
  builder.exportMemory('memory', 0);
  builder.exportFunc('_start', start);
  return builder.build();
}

function buildNoStart() {
  const builder = new WasmBuilder();
  builder.memory(1);
  builder.exportMemory('memory', 0);
  return builder.build();
}

function buildSocketImport() {
  const builder = new WasmBuilder();
  const typeVoid = builder.type([], []);
  const typeSockAccept = builder.type([I32, I32, I32], [I32]);
  builder.importFunc(WASI_MODULE, 'sock_accept', typeSockAccept);
  builder.memory(1);
  const start = builder.function(typeVoid, [], []);
  builder.exportMemory('memory', 0);
  builder.exportFunc('_start', start);
  return builder.build();
}

function section(id, payload) {
  return [id, ...u32(payload.length), ...payload];
}

function name(text) {
  const bytes = [...new TextEncoder().encode(text)];
  return [...u32(bytes.length), ...bytes];
}

function u32(value) {
  let remaining = value >>> 0;
  const bytes = [];
  do {
    let byte = remaining & 0x7f;
    remaining >>>= 7;
    if (remaining !== 0) {
      byte |= 0x80;
    }
    bytes.push(byte);
  } while (remaining !== 0);
  return bytes;
}

function i32(value) {
  let remaining = value | 0;
  const bytes = [];
  let more = true;
  while (more) {
    let byte = remaining & 0x7f;
    remaining >>= 7;
    if (
      (remaining === 0 && (byte & 0x40) === 0) ||
      (remaining === -1 && (byte & 0x40) !== 0)
    ) {
      more = false;
    } else {
      byte |= 0x80;
    }
    bytes.push(byte);
  }
  return bytes;
}

const op = {
  unreachable: () => [0x00],
  block: () => [0x02, 0x40],
  loop: () => [0x03, 0x40],
  if: () => [0x04, 0x40],
  else: () => [0x05],
  end: () => [0x0b],
  br: (depth) => [0x0c, ...u32(depth)],
  brIf: (depth) => [0x0d, ...u32(depth)],
  call: (index) => [0x10, ...u32(index)],
  drop: () => [0x1a],
  localGet: (index) => [0x20, ...u32(index)],
  localSet: (index) => [0x21, ...u32(index)],
  localTee: (index) => [0x22, ...u32(index)],
  globalGet: (index) => [0x23, ...u32(index)],
  globalSet: (index) => [0x24, ...u32(index)],
  i32Const: (value) => [0x41, ...i32(value)],
  i32Load: () => [0x28, 0x02, 0x00],
  i32Load8U: () => [0x2d, 0x00, 0x00],
  i32Store: () => [0x36, 0x02, 0x00],
  i32Store8: () => [0x3a, 0x00, 0x00],
  i32Eqz: () => [0x45],
  i32Eq: () => [0x46],
  i32LtU: () => [0x49],
  i32GtU: () => [0x4b],
  i32GeU: () => [0x4f],
  i32Add: () => [0x6a],
  i32Mul: () => [0x6c],
  i32And: () => [0x71],
  i32Or: () => [0x72],
  i32ShrU: () => [0x76],
};

const isMain =
  process.argv[1] !== undefined && resolve(process.argv[1]) === fileURLToPath(import.meta.url);

if (isMain) {
  const outputDir = process.argv[2] ?? process.cwd();
  const only = process.argv.slice(3);
  const files = buildFixtures(outputDir, only);
  for (const file of Object.values(files)) {
    process.stdout.write(`${file}\n`);
  }
}
