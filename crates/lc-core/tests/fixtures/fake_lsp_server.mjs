#!/usr/bin/env node
// Minimal fake LSP server used by lc-core integration tests.
//
// Usage: node fake_lsp_server.mjs --log <path>
//
// It implements just enough of LSP v0: initialize/initialized, didOpen,
// didChange, didClose, shutdown/exit, and publishDiagnostics. Every received
// method is appended to the log as one JSON line so the Rust test can verify
// ordering and payloads without a second IPC channel.

import { appendFileSync } from 'node:fs'

const logIndex = process.argv.indexOf('--log')
const logPath = logIndex >= 0 ? process.argv[logIndex + 1] : process.env.LC_FAKE_LSP_LOG

function log(entry) {
  if (!logPath) return
  try {
    appendFileSync(logPath, `${JSON.stringify(entry)}\n`)
  } catch {
    // Logging must never take the fake server down.
  }
}

function send(message) {
  const body = Buffer.from(JSON.stringify(message), 'utf8')
  const header = Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, 'ascii')
  process.stdout.write(Buffer.concat([header, body]))
}

function publish(uri, diagnostics) {
  send({
    jsonrpc: '2.0',
    method: 'textDocument/publishDiagnostics',
    params: { uri, diagnostics },
  })
}

function handle(message) {
  const method = typeof message.method === 'string' ? message.method : ''
  if (method === 'initialize') {
    log({ method, params: message.params })
    send({
      jsonrpc: '2.0',
      id: message.id,
      result: {
        capabilities: { textDocumentSync: 1 },
        serverInfo: { name: 'fake-lsp', version: '0.0.1' },
      },
    })
    return
  }

  if (method === 'initialized') {
    log({ method })
    return
  }

  if (method === 'textDocument/didOpen') {
    const document = message.params.textDocument
    log({
      method,
      uri: document.uri,
      languageId: document.languageId,
      version: document.version,
      text: document.text,
    })
    publish(document.uri, [
      {
        range: {
          start: { line: 1, character: 2 },
          end: { line: 1, character: 7 },
        },
        severity: 1,
        source: 'fake-lsp',
        message: 'fake: unknown token',
      },
    ])
    return
  }

  if (method === 'textDocument/didChange') {
    const uri = message.params.textDocument.uri
    const change = message.params.contentChanges[0]
    log({ method, uri, version: message.params.textDocument.version, text: change.text })
    publish(uri, [
      {
        range: {
          start: { line: 2, character: 0 },
          end: { line: 2, character: 3 },
        },
        severity: 2,
        source: 'fake-lsp',
        message: `changed:${change.text}`,
      },
    ])
    return
  }

  if (method === 'textDocument/didClose') {
    const uri = message.params.textDocument.uri
    log({ method, uri })
    publish(uri, [])
    return
  }

  if (method === 'shutdown') {
    log({ method })
    send({ jsonrpc: '2.0', id: message.id, result: null })
    return
  }

  if (method === 'exit') {
    log({ method })
    process.exit(0)
  }

  if (message.id !== undefined) {
    log({ method, unhandled: true })
    send({ jsonrpc: '2.0', id: message.id, result: null })
    return
  }

  log({ method, ignored: true })
}

let buffer = Buffer.alloc(0)

function parseFrames() {
  for (;;) {
    const headerEnd = buffer.indexOf('\r\n\r\n')
    if (headerEnd < 0) return
    const header = buffer.subarray(0, headerEnd).toString('ascii')
    const match = /Content-Length:\s*(\d+)/i.exec(header)
    if (!match) {
      log({ error: 'missing Content-Length', header })
      process.exit(2)
    }
    const length = Number(match[1])
    const bodyStart = headerEnd + 4
    if (buffer.length < bodyStart + length) return
    const body = buffer.subarray(bodyStart, bodyStart + length).toString('utf8')
    buffer = buffer.subarray(bodyStart + length)
    try {
      handle(JSON.parse(body))
    } catch (error) {
      log({ error: String(error), body })
    }
  }
}

process.stdin.on('data', chunk => {
  buffer = Buffer.concat([buffer, chunk])
  parseFrames()
})

process.stdin.on('end', () => {
  log({ method: 'stdin-end' })
  process.exit(0)
})
