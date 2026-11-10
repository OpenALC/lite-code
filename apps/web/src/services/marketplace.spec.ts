import { afterEach, describe, expect, it } from 'vitest'

import type { PluginList } from './extensions'
import {
  marketplaceAddRegistry,
  marketplaceInstall,
  marketplaceListRegistries,
  marketplaceRemoveRegistry,
  marketplaceSearch,
  marketplaceUninstall,
  marketplaceUpdate,
  type MarketPlugin,
  type MarketSearchResult,
} from './marketplace'

// node 环境无 window：默认走浏览器 fallback；Tauri 路径用注入的
// __TAURI_INTERNALS__ 驱动，命令名与参数在这里断言（仿 extensions.spec.ts）。

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

function marketPlugin(overrides: Partial<MarketPlugin> = {}): MarketPlugin {
  return {
    id: 'example.hello',
    version: '0.1.0',
    description: 'Demo plugin',
    runtime: 'node',
    engines: { 'lite-code': '>=0.6.1' },
    deps: [],
    permissions: ['filesystem.read'],
    source: 'official',
    fileSource: false,
    signatureState: 'unsigned',
    installedVersion: null,
    updateAvailable: false,
    ...overrides,
  }
}

const sampleSearch: MarketSearchResult = {
  results: [marketPlugin()],
  staleSources: ['mirror-a'],
}

const sampleList: PluginList = {
  plugins: [],
  dir: 'C:\\Users\\demo\\.lite-code\\extensions',
  nodeAvailable: true,
}

const sampleRegistry = {
  sources: [
    {
      name: 'official',
      url: 'https://plugins.example.com/index.json',
      builtin: true,
      requireSignatures: false,
      hasPubkey: false,
    },
  ],
}

describe('浏览器 fallback', () => {
  it('marketplaceSearch 返回两条演示条目并标注 fallback', async () => {
    const res = await marketplaceSearch()
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) {
      expect(res.data.results).toHaveLength(2)
      expect(res.data.staleSources).toEqual([])
      // 演示集同时覆盖 https+signed 与 file://+unsigned 两种徽标形态。
      const states = res.data.results.map(plugin => plugin.signatureState).sort()
      expect(states).toEqual(['signed', 'unsigned'])
      expect(res.data.results.some(plugin => plugin.fileSource)).toBe(true)
    }
  })

  it('marketplaceSearch 按 query 本地过滤演示条目', async () => {
    const res = await marketplaceSearch('local')
    expect(res.ok).toBe(true)
    if (res.ok) {
      expect(res.data.results).toHaveLength(1)
      expect(res.data.results[0].id).toBe('demo.local')
    }
  })

  it('marketplaceInstall/Uninstall 在演示状态上生效并返回 fallback 列表', async () => {
    const installRes = await marketplaceInstall('demo.hello', { version: '0.1.0' })
    expect(installRes.ok).toBe(true)
    expect(installRes.fallback).toBe(true)
    if (installRes.ok) {
      expect(installRes.data.plugins.map(plugin => plugin.id)).toContain('demo.hello')
    }
    const searchRes = await marketplaceSearch('hello')
    expect(searchRes.ok).toBe(true)
    if (searchRes.ok) {
      expect(searchRes.data.results[0].installedVersion).toBe('0.1.0')
    }
    const uninstallRes = await marketplaceUninstall('demo.hello')
    expect(uninstallRes.ok).toBe(true)
    const after = await marketplaceSearch('hello')
    expect(after.ok).toBe(true)
    if (after.ok) expect(after.data.results[0].installedVersion).toBeNull()
  })

  it('marketplaceUpdate 返回演示列表并标注 fallback', async () => {
    const res = await marketplaceUpdate()
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
  })

  it('registry 命令返回演示源并标注 fallback', async () => {
    const listRes = await marketplaceListRegistries()
    expect(listRes.ok).toBe(true)
    expect(listRes.fallback).toBe(true)
    if (listRes.ok) expect(listRes.data.sources.length).toBeGreaterThan(0)

    const addRes = await marketplaceAddRegistry('extra', 'file:///tmp/index.json')
    expect(addRes.ok).toBe(true)
    expect(addRes.fallback).toBe(true)

    const removeRes = await marketplaceRemoveRegistry('extra')
    expect(removeRes.ok).toBe(true)
    expect(removeRes.fallback).toBe(true)
  })
})

describe('Tauri 路径', () => {
  it('ext_search 传 query 与 refresh，接受 camelCase DTO', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return sampleSearch
      },
    })
    const res = await marketplaceSearch('hello', true)
    expect(calls).toEqual([
      { cmd: 'ext_search', args: { query: 'hello', refresh: true } },
    ])
    expect(res).toEqual({ ok: true, data: sampleSearch, fallback: false })
  })

  it('ext_search 缺省参数传 null', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return sampleSearch
      },
    })
    await marketplaceSearch()
    expect(calls).toEqual([
      { cmd: 'ext_search', args: { query: null, refresh: null } },
    ])
  })

  it('ext_install 透传 id/version/source/force', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return sampleList
      },
    })
    const res = await marketplaceInstall('example.hello', {
      version: '0.1.0',
      source: 'official',
      force: true,
    })
    expect(calls).toEqual([
      {
        cmd: 'ext_install',
        args: {
          id: 'example.hello',
          version: '0.1.0',
          source: 'official',
          force: true,
        },
      },
    ])
    expect(res.ok).toBe(true)

    const minimal = await marketplaceInstall('example.hello')
    expect(calls[1]).toEqual({
      cmd: 'ext_install',
      args: { id: 'example.hello', version: null, source: null, force: null },
    })
    expect(minimal.ok).toBe(true)
  })

  it('ext_update 透传 id 与 force', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return sampleList
      },
    })
    await marketplaceUpdate('example.hello', true)
    await marketplaceUpdate()
    expect(calls).toEqual([
      { cmd: 'ext_update', args: { id: 'example.hello', force: true } },
      { cmd: 'ext_update', args: { id: null, force: null } },
    ])
  })

  it('ext_uninstall 传 id', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return sampleList
      },
    })
    const res = await marketplaceUninstall('example.hello')
    expect(calls).toEqual([{ cmd: 'ext_uninstall', args: { id: 'example.hello' } }])
    expect(res.ok).toBe(true)
  })

  it('registry 三命令的参数透传', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return sampleRegistry
      },
    })
    await marketplaceListRegistries()
    await marketplaceAddRegistry('extra', 'file:///tmp/index.json', {
      requireSignatures: true,
      pubkey: 'abc',
    })
    await marketplaceRemoveRegistry('extra')
    expect(calls).toEqual([
      { cmd: 'ext_registry_list', args: {} },
      {
        cmd: 'ext_registry_add',
        args: {
          name: 'extra',
          url: 'file:///tmp/index.json',
          requireSignatures: true,
          pubkey: 'abc',
        },
      },
      { cmd: 'ext_registry_remove', args: { name: 'extra' } },
    ])
  })

  it('registry 可选参数缺省传 null', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return sampleRegistry
      },
    })
    await marketplaceAddRegistry('extra', 'https://plugins.example.com/index.json')
    expect(calls).toEqual([
      {
        cmd: 'ext_registry_add',
        args: {
          name: 'extra',
          url: 'https://plugins.example.com/index.json',
          requireSignatures: null,
          pubkey: null,
        },
      },
    ])
  })
})

describe('错误归一', () => {
  it('ErrorBody reject 原样进入 error 分支（ohos not_implemented 走此路径）', async () => {
    installTauri({
      invoke: async () => {
        throw { code: 'not_implemented', message: 'unsupported command on ohos shell: ext_search', detail: null }
      },
    })
    const res = await marketplaceSearch()
    expect(res.ok).toBe(false)
    if (!res.ok) {
      expect(res.fallback).toBe(false)
      expect(res.error.code).toBe('not_implemented')
    }
  })

  it('安装预检失败（invalid_argument）原样传播，供 UI 走 force 二次确认', async () => {
    installTauri({
      invoke: async () => {
        throw {
          code: 'invalid_argument',
          message: 'engines 不满足：lite-code >=99.0.0',
          detail: null,
        }
      },
    })
    const res = await marketplaceInstall('example.hello')
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.code).toBe('invalid_argument')
  })

  it('非 ErrorBody reject 归入 internal', async () => {
    installTauri({ invoke: async () => Promise.reject('boom') })
    const res = await marketplaceUninstall('example.hello')
    expect(res.ok).toBe(false)
    if (!res.ok) {
      expect(res.error.code).toBe('internal')
      expect(res.error.message).toContain('boom')
    }
  })

  it('非法 MarketSearchResult 载荷归入 internal', async () => {
    installTauri({ invoke: async () => ({ results: [{ id: 1 }], staleSources: [] }) })
    const res = await marketplaceSearch()
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.message).toContain('ext_search')
  })

  it('非法 RegistryList 载荷归入 internal', async () => {
    installTauri({ invoke: async () => ({ sources: [{ name: 'x' }] }) })
    const res = await marketplaceListRegistries()
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.message).toContain('ext_registry_list')
  })

  it('非法 PluginList 载荷归入 internal', async () => {
    installTauri({ invoke: async () => ({ plugins: [], dir: 'x' }) })
    const res = await marketplaceInstall('example.hello')
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.code).toBe('internal')
  })
})
