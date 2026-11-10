#!/usr/bin/env node
// Minimal fake LSP server used by lc-core integration tests.
//
// Usage: node fake_lsp_server.mjs --log <path> [--hang <method>] [--rpc-error <method>]
//
// It implements just enough of LSP v0: initialize/initialized, didOpen,
// didChange, didClose, shutdown/exit, publishDiagnostics, plus the editor
// feature requests (completion/hover/definition). Every received method is
// appended to the log as one JSON line so the Rust test can verify ordering
// and payloads without a second IPC channel.
//
// --hang <method>:     the method is logged but never answered (timeout tests).
// --rpc-error <method>: the method is answered with a JSON-RPC error.

import { appendFileSync } from 'node:fs'

const logIndex = process.argv.indexOf('--log')
const logPath = logIndex >= 0 ? process.argv[logIndex + 1] : process.env.LC_FAKE_LSP_LOG

function flagValue(flag) {
  const index = process.argv.indexOf(flag)
  return index >= 0 ? process.argv[index + 1] : undefined
}

const HANG_ON = flagValue('--hang')
const RPC_ERROR_ON = flagValue('--rpc-error')

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

  if (message.id !== undefined) {
    if (method === HANG_ON) {
      log({ method, hang: true })
      return
    }
    if (method === RPC_ERROR_ON) {
      log({ method, rpcError: true })
      send({
        jsonrpc: '2.0',
        id: message.id,
        error: { code: -32603, message: 'fake: method failed' },
      })
      return
    }
  }

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

  if (method === 'textDocument/completion') {
    const position = message.params.position
    log({
      method,
      uri: message.params.textDocument.uri,
      line: position.line,
      character: position.character,
    })
    send({
      jsonrpc: '2.0',
      id: message.id,
      result: [
        { label: 'alpha', kind: 6, detail: 'first item' },
        { label: 'beta', kind: 3, insertText: 'beta()' },
        {
          label: 'gamma',
          kind: 14,
          textEdit: {
            range: {
              start: { line: position.line, character: 0 },
              end: position,
            },
            newText: 'gamma!',
          },
        },
      ],
    })
    return
  }

  if (method === 'textDocument/hover') {
    const position = message.params.position
    log({
      method,
      uri: message.params.textDocument.uri,
      line: position.line,
      character: position.character,
    })
    send({
      jsonrpc: '2.0',
      id: message.id,
      result: {
        contents: { kind: 'markdown', value: '**fake** hover' },
      },
    })
    return
  }

  if (method === 'textDocument/definition') {
    const uri = message.params.textDocument.uri
    const position = message.params.position
    log({ method, uri, line: position.line, character: position.character })
    send({
      jsonrpc: '2.0',
      id: message.id,
      result: [
        {
          uri,
          range: {
            start: { line: 0, character: 0 },
            end: { line: 0, character: 4 },
          },
        },
        {
          targetUri: uri,
          targetRange: {
            start: { line: 2, character: 0 },
            end: { line: 2, character: 3 },
          },
          targetSelectionRange: {
            start: { line: 2, character: 1 },
            end: { line: 2, character: 3 },
          },
        },
      ],
    })
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
