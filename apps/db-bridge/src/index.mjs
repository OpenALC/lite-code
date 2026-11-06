#!/usr/bin/env node
// stdin/stdout NDJSON JSON-RPC 2.0 runner。请求串行处理，避免同一连接上的
// 驱动调用交叉；Rust 侧本身也是一次一个请求。

import readline from 'node:readline';

import { createBridge } from './bridge.mjs';
import { BridgeError, invalidArgument, messageOf } from './errors.mjs';

const bridge = createBridge();

function write(message) {
  try {
    process.stdout.write(`${JSON.stringify(message)}\n`);
  } catch {
    process.exit(0);
  }
}

function reply(id, result) {
  write({ jsonrpc: '2.0', id, result });
}

function fail(id, error) {
  const code = error instanceof BridgeError ? error.code : 'db_rejected';
  const message = messageOf(error);
  write({ jsonrpc: '2.0', id, error: { code, message } });
}

async function handle(request) {
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

  const handlers = {
    initialize: bridge.initialize,
    connect: bridge.connect,
    query: bridge.query,
    execute: bridge.execute,
    schema: bridge.schema,
    explain: bridge.explain,
    disconnect: bridge.disconnect,
  };
  const handler = handlers[request.method];
  if (!handler) {
    fail(id, invalidArgument(`unknown method: ${request.method}`));
    return;
  }

  try {
    reply(id, await handler(request.params ?? {}));
  } catch (error) {
    fail(id, error);
  }
}

const input = readline.createInterface({
  input: process.stdin,
  crlfDelay: Infinity,
});

let queue = Promise.resolve();

input.on('line', (line) => {
  const trimmed = line.trim();
  if (trimmed.length === 0) return;

  queue = queue
    .then(async () => {
      let request;
      try {
        request = JSON.parse(trimmed);
      } catch {
        fail(null, invalidArgument('invalid JSON'));
        return;
      }
      await handle(request);
    })
    .catch(() => {
      // handle 已把可预期错误写回；这里兜底防止队列永久 rejected。
    });
});

input.on('close', () => {
  // 不显式 exit：等 stdout 的最后一条响应写完，closeAll 的 socket 关闭后
  // 事件循环自然退出。
  void queue.then(() => bridge.closeAll());
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => {
    void bridge.closeAll().finally(() => process.exit(0));
  });
}
