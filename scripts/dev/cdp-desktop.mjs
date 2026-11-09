#!/usr/bin/env node
// 桌面 WebView2 devtools（CDP 9222）最小驱动——dev 应用无全局 __TAURI__
// 注入时的 e2e 通道：页面内用 window.__TAURI_INTERNALS__.invoke 调命令、
// transformCallback + plugin:event|listen（target: { kind: 'Any' }）挂事件。
// 前置：pnpm tauri dev 带 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
// 用法：node scripts/dev/cdp-desktop.mjs evalfile <脚本.js>（脚本为一个 async IIFE 表达式）
import fs from 'node:fs'

const PORT = process.env.DESK_CDP_PORT ?? '9222'
const HOST = `http://127.0.0.1:${PORT}`

async function target() {
  const res = await fetch(`${HOST}/json`)
  const list = await res.json()
  const page = list.find(item => item.type === 'page' && item.title === 'Lite Code')
  if (!page) throw new Error('no Lite Code page target')
  return page
}

function connect(url) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url)
    let nextId = 1
    const pending = new Map()
    ws.addEventListener('open', () => {
      resolve({
        send(method, params = {}) {
          const id = nextId++
          ws.send(JSON.stringify({ id, method, params }))
          return new Promise((res, rej) => pending.set(id, { res, rej }))
        },
        close() { ws.close() },
      })
    })
    ws.addEventListener('message', event => {
      const msg = JSON.parse(event.data)
      if (msg.id && pending.has(msg.id)) {
        const { res, rej } = pending.get(msg.id)
        pending.delete(msg.id)
        if (msg.error) rej(new Error(JSON.stringify(msg.error)))
        else res(msg.result)
      }
    })
    ws.addEventListener('error', event => reject(new Error(`ws error: ${event.message ?? event}`)))
  })
}

async function evaluate(client, expression) {
  const result = await client.send('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  })
  if (result.exceptionDetails) {
    throw new Error(`eval exception: ${result.exceptionDetails.text} ${result.exceptionDetails.exception?.description ?? ''}`)
  }
  return result.result?.value
}

const [command, ...args] = process.argv.slice(2)
const page = await target()
const client = await connect(page.webSocketDebuggerUrl)
try {
  if (command === 'evalfile') {
    const expression = fs.readFileSync(args[0], 'utf8')
    console.log(JSON.stringify(await evaluate(client, expression), null, 2))
  } else {
    throw new Error(`unknown command: ${command}`)
  }
} finally {
  client.close()
}
