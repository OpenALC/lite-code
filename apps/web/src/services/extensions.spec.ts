import { afterEach, describe, expect, it } from 'vitest'

import {
  extensionsExecuteCommand,
  extensionsList,
  extensionsOpenDir,
  extensionsReadTheme,
  extensionsReload,
  extensionsSetEnabled,
  type PluginList,
} from './extensions'

// node 环境无 window：默认走浏览器 fallback；Tauri 路径用注入的
// __TAURI_INTERNALS__ 驱动，命令名与参数在这里断言。

interface FakeInternals {
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>
}

function installTauri(fake: FakeInternals): void {
  ;(globalThis as unknown as { window: unknown }).window = {
    __TAURI_INTERNALS__: fake,
  }
}

function removeWindow(): void {
  delete (globalThis as unknown as { window?: unknown }).window
}

afterEach(removeWindow)

const sampleList: PluginList = {
  plugins: [
    {
      id: 'example.hello',
      name: 'Hello',
      version: '0.1.0',
      description: 'Demo plugin',
      runtime: 'node',
      enabled: true,
      path: 'C:\\Users\\demo\\.lite-code\\extensions\\example.hello',
      error: null,
      commands: [{ command: 'example.hello', title: 'Hello' }],
      mcpServers: ['filesystem'],
      languageServers: ['typescript'],
      themes: [{ label: 'Dusk', path: './themes/dusk.json', uiTheme: 'dark' }],
    },
  ],
  dir: 'C:\\Users\\demo\\.lite-code\\extensions',
  nodeAvailable: true,
}

describe('浏览器 fallback', () => {
  it('extensionsList 返回 node/declarative 两个演示插件', async () => {
    const res = await extensionsList()
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) {
      expect(res.data.plugins).toHaveLength(2)
      expect(res.data.plugins.map(plugin => plugin.runtime).sort()).toEqual([
        'declarative',
        'node',
      ])
      expect(res.data.dir).toContain('extensions')
      expect(res.data.nodeAvailable).toBe(true)
      expect(res.data.plugins.every(plugin => plugin.commands.length > 0)).toBe(true)
    }
  })

  it('extensionsSetEnabled 在演示状态上生效并返回更新后的列表', async () => {
    const before = await extensionsList()
    expect(before.ok).toBe(true)
    if (!before.ok) return
    const target = before.data.plugins[0]
    try {
      const res = await extensionsSetEnabled(target.id, false)
      expect(res.ok).toBe(true)
      expect(res.fallback).toBe(true)
      if (res.ok) {
        expect(res.data.plugins.find(plugin => plugin.id === target.id)?.enabled).toBe(false)
      }
    } finally {
      await extensionsSetEnabled(target.id, true)
    }
  })

  it('extensionsReload 返回演示列表并标注 fallback', async () => {
    const res = await extensionsReload()
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) {
      expect(res.data.plugins).toHaveLength(2)
      expect(res.data.nodeAvailable).toBe(true)
    }
  })

  it('extensionsOpenDir 返回 ack 且不抛错', async () => {
    const res = await extensionsOpenDir()
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) expect(res.data).toEqual({ kind: 'ack' })
  })

  it('extensionsExecuteCommand 返回 demo 结果且不抛错', async () => {
    const res = await extensionsExecuteCommand('demo.hello', { name: 'Ada' })
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) {
      expect(res.data.result).toMatchObject({
        demo: true,
        executed: false,
        command: 'demo.hello',
        args: { name: 'Ada' },
      })
    }
  })

  it('extensionsReadTheme 返回演示主题 JSON；未声明路径进 error', async () => {
    const ok = await extensionsReadTheme('demo.hello', './themes/dusk.json')
    expect(ok.ok).toBe(true)
    expect(ok.fallback).toBe(true)
    if (ok.ok) {
      expect(ok.data.json).toMatchObject({ name: 'Demo Dusk' })
    }
    const missing = await extensionsReadTheme('demo.notes', './themes/dusk.json')
    expect(missing.ok).toBe(false)
  })
})

describe('Tauri 路径', () => {
  it('ext_list 无参数，接受 camelCase DTO', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return sampleList
      },
    })
    const res = await extensionsList()
    expect(calls).toEqual([{ cmd: 'ext_list', args: {} }])
    expect(res).toEqual({ ok: true, data: sampleList, fallback: false })
  })

  it('ext_set_enabled 传 id 与 enabled', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return sampleList
      },
    })
    const res = await extensionsSetEnabled('example.hello', false)
    expect(calls).toEqual([
      { cmd: 'ext_set_enabled', args: { id: 'example.hello', enabled: false } },
    ])
    expect(res.ok).toBe(true)
  })

  it('ext_reload / ext_open_dir 使用空参数', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return cmd === 'ext_open_dir' ? { kind: 'ack' } : sampleList
      },
    })
    const reloadRes = await extensionsReload()
    const openRes = await extensionsOpenDir()
    expect(calls).toEqual([
      { cmd: 'ext_reload', args: {} },
      { cmd: 'ext_open_dir', args: {} },
    ])
    expect(reloadRes.ok).toBe(true)
    expect(openRes.ok).toBe(true)
    if (openRes.ok) expect(openRes.data.kind).toBe('ack')
  })

  it('ext_execute_command 传 command 与可选 args', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { result: { done: true } }
      },
    })
    const withArgs = await extensionsExecuteCommand('example.hello', { name: 'Ada' })
    const withoutArgs = await extensionsExecuteCommand('example.hello')
    expect(calls).toEqual([
      {
        cmd: 'ext_execute_command',
        args: { command: 'example.hello', args: { name: 'Ada' } },
      },
      {
        cmd: 'ext_execute_command',
        args: { command: 'example.hello', args: null },
      },
    ])
    expect(withArgs.ok).toBe(true)
    if (withArgs.ok) expect(withArgs.data.result).toEqual({ done: true })
    expect(withoutArgs.ok).toBe(true)
  })

  it('ext_read_theme 传 pluginId 与 path，校验 { json } 载荷', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { json: { name: 'Dusk', colors: { '--bg': '#000' } } }
      },
    })
    const res = await extensionsReadTheme('example.hello', './themes/dusk.json')
    expect(calls).toEqual([
      {
        cmd: 'ext_read_theme',
        args: { pluginId: 'example.hello', path: './themes/dusk.json' },
      },
    ])
    expect(res.ok).toBe(true)
    if (res.ok) expect(res.data.json).toMatchObject({ name: 'Dusk' })
  })
})

describe('错误归一', () => {
  it('ErrorBody reject 原样进入 error 分支', async () => {
    installTauri({
      invoke: async () => {
        throw { code: 'node_unavailable', message: 'Node missing', detail: 'install Node' }
      },
    })
    const res = await extensionsList()
    expect(res.ok).toBe(false)
    if (!res.ok) {
      expect(res.fallback).toBe(false)
      expect(res.error).toEqual({
        code: 'node_unavailable',
        message: 'Node missing',
        detail: 'install Node',
      })
    }
  })

  it('非 ErrorBody reject 归入 internal', async () => {
    installTauri({ invoke: async () => Promise.reject('boom') })
    const res = await extensionsExecuteCommand('example.hello')
    expect(res.ok).toBe(false)
    if (!res.ok) {
      expect(res.error.code).toBe('internal')
      expect(res.error.message).toContain('boom')
    }
  })

  it('非法 PluginList 载荷归入 internal', async () => {
    installTauri({ invoke: async () => ({ plugins: [], dir: 'x' }) })
    const res = await extensionsList()
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.code).toBe('internal')
  })

  it('ext_execute_command 缺 result 字段归入 internal', async () => {
    installTauri({ invoke: async () => ({ nope: true }) })
    const res = await extensionsExecuteCommand('example.hello')
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.message).toContain('ext_execute_command')
  })

  it('ext_open_dir 非 ack 载荷归入 internal', async () => {
    installTauri({ invoke: async () => ({ kind: 'other' }) })
    const res = await extensionsOpenDir()
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.message).toContain('ext_open_dir')
  })
})
