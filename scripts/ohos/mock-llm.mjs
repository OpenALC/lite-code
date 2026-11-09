#!/usr/bin/env node
// Minimal OpenAI Responses-compatible SSE endpoint for the HarmonyOS emulator.
//
// The emulator reaches the host through 10.0.2.2, so this listens on
// 0.0.0.0:8765 and accepts POST /v1/responses (the lc-core client appends
// `/responses` to providers.json's baseUrl). Authorization is ignored on
// purpose: the OHOS shell has no secret store and its providers template only
// carries an `env:` reference that is unset on the emulator.
//
// Behaviour:
//   - request already contains function_call_output -> stream a final answer
//   - latest user text mentions read/读取/解释/explain -> emit a read_file call
//   - otherwise -> stream a plain answer
//
// Usage:
//   node scripts/ohos/mock-llm.mjs [--port 8765]
//   PORT=8765 node scripts/ohos/mock-llm.mjs
//
// Logs one line per request; Ctrl+C stops the server.

import http from 'node:http';
import process from 'node:process';

const DEFAULT_PORT = 8765;
const HOST = '0.0.0.0';
const DELAY_MS = Number(process.env.MOCK_DELAY_MS ?? 30);

function parsePort(argv) {
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === '--port') {
      const value = Number(argv[index + 1]);
      if (Number.isInteger(value) && value > 0 && value < 65536) {
        return value;
      }
    }
    if (arg.startsWith('--port=')) {
      const value = Number(arg.slice('--port='.length));
      if (Number.isInteger(value) && value > 0 && value < 65536) {
        return value;
      }
    }
  }
  const env = Number(process.env.PORT);
  return Number.isInteger(env) && env > 0 && env < 65536 ? env : DEFAULT_PORT;
}

const port = parsePort(process.argv.slice(2));

function log(message) {
  console.log(`[mock-llm] ${message}`);
}

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms));
}

function sse(res, payload) {
  if (res.destroyed || res.writableEnded) {
    return false;
  }
  res.write(`data: ${JSON.stringify(payload)}\n\n`);
  return true;
}

function textDeltas(text, chunkSize = 8) {
  const chunks = [];
  for (let index = 0; index < text.length; index += chunkSize) {
    chunks.push(text.slice(index, index + chunkSize));
  }
  return chunks;
}

function lastUserIndex(input) {
  if (!Array.isArray(input)) {
    return -1;
  }
  for (let index = input.length - 1; index >= 0; index -= 1) {
    const item = input[index] ?? {};
    if (item.role === 'user' && typeof item.content === 'string') {
      return index;
    }
  }
  return -1;
}

function lastUserText(input) {
  const index = lastUserIndex(input);
  return index >= 0 ? input[index].content : '';
}

function lastFunctionCall(input) {
  if (!Array.isArray(input)) {
    return null;
  }
  for (let index = input.length - 1; index >= 0; index -= 1) {
    const item = input[index] ?? {};
    if (item.type === 'function_call' && item.name === 'read_file') {
      return item;
    }
  }
  return null;
}

// Only an output produced *after* the latest user message belongs to this turn.
// A persisted conversation can contain old function_call_output items; treating
// those as current would skip the tool call on every follow-up message.
function lastToolOutput(input) {
  const userIndex = lastUserIndex(input);
  if (!Array.isArray(input) || userIndex < 0) {
    return '';
  }
  for (let index = input.length - 1; index > userIndex; index -= 1) {
    const item = input[index] ?? {};
    if (item.type === 'function_call_output' && typeof item.output === 'string') {
      return item.output;
    }
  }
  return '';
}

function requestedPath(prompt, call) {
  const match = /([\w./-]+\.\w+)/u.exec(prompt);
  if (match) {
    return match[1];
  }
  if (call && typeof call.arguments === 'string') {
    try {
      const args = JSON.parse(call.arguments);
      if (typeof args.path === 'string' && args.path.trim() !== '') {
        return args.path;
      }
    } catch {
      // Fall through to the sample path.
    }
  }
  return 'src/util/message.rs';
}

function answerFor(fileContent) {
  if (fileContent.includes('fn greeting')) {
    return (
      '我读取了 src/util/message.rs。它只有一个公开函数 greeting(name: &str) -> String，' +
      '用 format! 拼出中文问候语；上方注释说明这是用于验证三级目录只读预览的示例文件。\n'
    );
  }
  if (fileContent.trim() === '') {
    return '文件存在但内容为空。\n';
  }
  const lines = fileContent.split('\n').length;
  return `已读取目标文件，共 ${lines} 行、${fileContent.length} 个字符。\n`;
}

async function streamText(res, text, usage) {
  for (const delta of textDeltas(text)) {
    if (!sse(res, {
      type: 'response.output_text.delta',
      output_index: 0,
      content_index: 0,
      delta,
    })) {
      return;
    }
    await sleep(DELAY_MS);
  }
  sse(res, {
    type: 'response.completed',
    response: {
      status: 'completed',
      usage,
      output: [{ type: 'message', role: 'assistant', content: [{ type: 'output_text', text }] }],
    },
  });
  res.end();
}

async function streamToolCall(res, path) {
  const callId = 'call_mock_1';
  const itemId = 'item_mock_1';
  const args = JSON.stringify({ path });
  sse(res, {
    type: 'response.created',
    response: { status: 'in_progress', output: [] },
  });
  await sleep(DELAY_MS);
  sse(res, {
    type: 'response.reasoning_summary_text.delta',
    delta: `需要读取 ${path} 后再解释。`,
  });
  await sleep(DELAY_MS);
  sse(res, {
    type: 'response.output_item.added',
    output_index: 0,
    item: {
      type: 'function_call',
      id: itemId,
      call_id: callId,
      name: 'read_file',
      arguments: '',
    },
  });
  for (const delta of textDeltas(args, 6)) {
    await sleep(DELAY_MS);
    if (!sse(res, {
      type: 'response.function_call_arguments.delta',
      item_id: itemId,
      output_index: 0,
      delta,
    })) {
      return;
    }
  }
  sse(res, {
    type: 'response.completed',
    response: {
      status: 'completed',
      usage: { input_tokens: 64, output_tokens: 18, input_tokens_details: { cached_tokens: 0 } },
      output: [{ type: 'function_call', id: itemId, call_id: callId, name: 'read_file' }],
    },
  });
  res.end();
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    req.on('data', chunk => {
      chunks.push(chunk);
      if (chunks.reduce((total, item) => total + item.length, 0) > 4 * 1024 * 1024) {
        reject(new Error('request body too large'));
        req.destroy();
      }
    });
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
    req.on('error', reject);
  });
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url ?? '/', `http://${req.headers.host ?? 'localhost'}`);

  if (req.method === 'GET' && (url.pathname === '/health' || url.pathname === '/')) {
    res.writeHead(200, { 'Content-Type': 'text/plain' });
    res.end('mock-llm ok\n');
    return;
  }

  if (req.method !== 'POST' || !/^\/(v1\/)?responses$/u.test(url.pathname)) {
    res.writeHead(404, { 'Content-Type': 'text/plain' });
    res.end('not found\n');
    return;
  }

  let body;
  try {
    body = JSON.parse(await readBody(req));
  } catch (error) {
    res.writeHead(400, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ error: { message: `invalid json: ${error.message}` } }));
    return;
  }

  const input = Array.isArray(body.input) ? body.input : [];
  const userText = lastUserText(input);
  const toolOutput = lastToolOutput(input);
  const toolCall = lastFunctionCall(input);
  const hasReadTool = Array.isArray(body.tools)
    && body.tools.some(tool => tool && tool.name === 'read_file');
  const wantsRead = /读取|解释|read\b|explain|summari[sz]e/iu.test(userText) && hasReadTool;

  res.writeHead(200, {
    'Content-Type': 'text/event-stream; charset=utf-8',
    'Cache-Control': 'no-cache, no-transform',
    Connection: 'keep-alive',
    'X-Accel-Buffering': 'no',
  });
  res.flushHeaders();

  if (toolOutput !== '') {
    const path = requestedPath(userText, toolCall);
    log(`POST ${url.pathname} model=${body.model ?? '?'} -> final answer (${path})`);
    await streamText(res, answerFor(toolOutput), {
      input_tokens: 120,
      output_tokens: 40,
      input_tokens_details: { cached_tokens: 0 },
    });
    return;
  }

  if (wantsRead) {
    const path = requestedPath(userText, null);
    log(`POST ${url.pathname} model=${body.model ?? '?'} -> read_file ${path}`);
    await streamToolCall(res, path);
    return;
  }

  log(`POST ${url.pathname} model=${body.model ?? '?'} -> text answer`);
  await streamText(
    res,
    '这是 mock-llm 的离线回答。若要验证工具调用，请在 prompt 里写「读取 src/util/message.rs 并解释」。\n',
    { input_tokens: 20, output_tokens: 24, input_tokens_details: { cached_tokens: 0 } },
  );
});

server.on('error', error => {
  console.error(`[mock-llm] server error: ${error.message}`);
  process.exitCode = 1;
});

server.listen(port, HOST, () => {
  log(`listening on http://${HOST}:${port} (emulator: http://10.0.2.2:${port}/v1)`);
  log('Authorization is ignored; no API key required.');
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => {
    log(`${signal} received, shutting down`);
    server.close(() => process.exit(0));
    setTimeout(() => process.exit(0), 500).unref();
  });
}
