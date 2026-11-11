import { afterEach, describe, expect, it } from 'vitest'

import {
  extensionsExecuteCommand,
  extensionsList,
  extensionsOpenDir,
  extensionsReadGrammar,
  extensionsReadTheme,
  extensionsReload,
  extensionsSetEnabled,
  extensionsUiValues,
  extensionsViewChildren,
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
      languages: [
        {
          id: 'demo-lang',
          extensions: ['.demo'],
          aliases: ['Demo'],
          grammar: './grammars/demo.json',
        },
      ],
      views: [{ id: 'demo-tree', name: 'Demo Tree' }],
      webviews: [],
      uiViews: [
        {
          id: 'demo-board',
          name: 'Demo Board',
          openCommand: 'example.hello.board',
          sections: [{ id: 'intro', kind: 'markdown', text: '# Demo' }],
        },
      ],
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

  it('extensionsReadGrammar 返回演示语法 JSON；未声明路径进 error', async () => {
    const ok = await extensionsReadGrammar('demo.hello', './grammars/demo.json')
    expect(ok.ok).toBe(true)
    expect(ok.fallback).toBe(true)
    if (ok.ok) {
      expect(ok.data.json).toMatchObject({ tokenizer: { root: expect.anything() } })
    }
    const missing = await extensionsReadGrammar('demo.notes', './grammars/demo.json')
    expect(missing.ok).toBe(false)
  })

  it('extensionsViewChildren 返回演示节点；未声明视图进 error', async () => {
    const roots = await extensionsViewChildren('demo.hello', 'demo-tree')
    expect(roots.ok).toBe(true)
    expect(roots.fallback).toBe(true)
    if (roots.ok) {
      expect(roots.data.nodes).toHaveLength(2)
      expect(roots.data.nodes[0]).toMatchObject({ id: 'group', label: 'Group' })
    }
    const children = await extensionsViewChildren('demo.hello', 'demo-tree', 'group')
    expect(children.ok).toBe(true)
    if (children.ok) {
      expect(children.data.nodes).toEqual([{ id: 'leaf', label: 'Leaf item' }])
    }
    const missing = await extensionsViewChildren('demo.notes', 'demo-tree')
    expect(missing.ok).toBe(false)
  })

  it('extensionsUiValues 返回演示节值；未声明节进 error', async () => {
    const kv = await extensionsUiValues('demo.hello', 'demo-board', 'stats')
    expect(kv.ok).toBe(true)
    expect(kv.fallback).toBe(true)
    if (kv.ok) {
      expect(kv.data.values).toEqual({
        items: [
          { key: '版本', value: '0.1.0' },
          { key: '消息数', value: '2' },
        ],
      })
    }
    // 声明式 list 无 source 不回读，演示数据无该节 → error。
    const missing = await extensionsUiValues('demo.notes', 'notes-panel', 'tasks')
    expect(missing.ok).toBe(false)
    if (!missing.ok) expect(missing.error.code).toBe('internal')
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

  it('ext_read_grammar 传 pluginId 与 path，校验 { json } 载荷', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { json: { tokenizer: { root: [] } } }
      },
    })
    const res = await extensionsReadGrammar('example.hello', './grammars/demo.json')
    expect(calls).toEqual([
      {
        cmd: 'ext_read_grammar',
        args: { pluginId: 'example.hello', path: './grammars/demo.json' },
      },
    ])
    expect(res.ok).toBe(true)
    if (res.ok) expect(res.data.json).toMatchObject({ tokenizer: { root: [] } })
  })

  it('ext_view_children 传 pluginId/viewId/nodeId，校验 { nodes } 载荷', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { nodes: [{ id: 'n1', label: 'Node 1' }] }
      },
    })
    const withNode = await extensionsViewChildren('example.hello', 'demo-tree', 'n1')
    const withoutNode = await extensionsViewChildren('example.hello', 'demo-tree')
    expect(calls).toEqual([
      {
        cmd: 'ext_view_children',
        args: { pluginId: 'example.hello', viewId: 'demo-tree', nodeId: 'n1' },
      },
      {
        cmd: 'ext_view_children',
        args: { pluginId: 'example.hello', viewId: 'demo-tree', nodeId: null },
      },
    ])
    expect(withNode.ok).toBe(true)
    if (withNode.ok) {
      expect(withNode.data.nodes).toEqual([{ id: 'n1', label: 'Node 1' }])
    }
    expect(withoutNode.ok).toBe(true)
  })

  it('ext_view_children 缺 nodes 字段归入 internal', async () => {
    installTauri({ invoke: async () => ({ nope: true }) })
    const res = await extensionsViewChildren('example.hello', 'demo-tree')
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.message).toContain('ext_view_children')
  })

  it('ext_ui_values 传 pluginId/viewId/sectionId，校验 { values } 载荷', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { values: { rows: [{ title: 't1' }] } }
      },
    })
    const res = await extensionsUiValues('example.hello', 'demo-board', 'stats')
    expect(calls).toEqual([
      {
        cmd: 'ext_ui_values',
        args: { pluginId: 'example.hello', viewId: 'demo-board', sectionId: 'stats' },
      },
    ])
    expect(res.ok).toBe(true)
    if (res.ok) expect(res.data.values).toEqual({ rows: [{ title: 't1' }] })
  })

  it('ext_ui_values 缺 values 字段归入 internal', async () => {
    installTauri({ invoke: async () => ({ nope: true }) })
    const res = await extensionsUiValues('example.hello', 'demo-board', 'stats')
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.message).toContain('ext_ui_values')
  })

  it('ext_ui_values reject 原样进入 error 分支（ohos 未转发场景）', async () => {
    installTauri({
      invoke: async () => {
        throw { code: 'not_found', message: 'unknown command', detail: null }
      },
    })
    const res = await extensionsUiValues('example.hello', 'demo-board', 'stats')
    expect(res.ok).toBe(false)
    if (!res.ok) {
      expect(res.fallback).toBe(false)
      expect(res.error.code).toBe('not_found')
    }
  })

  it('ext_list 的 uiViews 守卫：节级形状由 pluginUi 归一，视图级形状非法整包拒绝', async () => {
    // 视图级合法（sections 只要求是数组）→ 通过；
    const valid = structuredClone(sampleList)
    installTauri({ invoke: async () => valid })
    const ok = await extensionsList()
    expect(ok.ok).toBe(true)

    // 视图级非法（sections 不是数组）→ 整包归入 internal（fail-closed）。
    const broken = structuredClone(sampleList)
    broken.plugins[0].uiViews = [
      { id: 'x', name: 'X', openCommand: null, sections: 'nope' as unknown as unknown[] },
    ]
    installTauri({ invoke: async () => broken })
    const res = await extensionsList()
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.code).toBe('internal')
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
