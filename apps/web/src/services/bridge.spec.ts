import { afterEach, describe, expect, it } from 'vitest'

import {
  detectNode,
  isTauri,
  readFile,
  readDir,
  type BridgeResult,
} from './bridge'
import type { CommandResult, DirListingResult } from './contract'

// node 环境无 window：默认走 fallback 占位路径；Tauri 路径用注入的假
// __TAURI_INTERNALS__ 驱动（invoke 的入参与 reject 载荷形状都在这里断言）。

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

describe('isTauri', () => {
  it('无 window（node 测试环境）返回 false', () => {
    expect(isTauri()).toBe(false)
  })

  it('有 __TAURI_INTERNALS__.invoke 返回 true', () => {
    installTauri({ invoke: async () => ({}) })
    expect(isTauri()).toBe(true)
  })

  it('internals 缺 invoke 方法视为非 Tauri', () => {
    ;(globalThis as unknown as { window: unknown }).window = { __TAURI_INTERNALS__: {} }
    expect(isTauri()).toBe(false)
  })
})

describe('fallback 占位（浏览器 dev）', () => {
  it('readDir 返回带 fallback 标注的 dir_listing', async () => {
    const res = await readDir('', '')
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) {
      expect(res.data.kind).toBe('dir_listing')
      expect(res.data.entries.length).toBeGreaterThan(0)
      expect(res.data.generation).toBe(0)
    }
  })

  it('readDir 按 root 前缀归一层键，未知层为空目录', async () => {
    const nested = await readDir('D:/proj', 'D:/proj/docs')
    if (nested.ok) expect(nested.data.entries.map(e => e.name)).toContain('ARCHITECTURE.md')
    const unknown = await readDir('D:/proj', 'D:/proj/nope')
    if (unknown.ok) expect(unknown.data.entries).toEqual([])
  })

  it('readFile 返回标注占位的文本', async () => {
    const res = await readFile('', 'README.md')
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) {
      expect(res.data.kind).toBe('file_content')
      expect(res.data.text).toContain('占位')
    }
  })

  it('detectNode 返回 need_manual 占位（不伪造可用的 Node）', async () => {
    const res = await detectNode()
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) {
      expect(res.data.kind).toBe('node_status')
      if (res.data.kind === 'node_status') {
        expect(res.data.status.kind).toBe('need_manual')
      }
    }
  })
})

describe('Tauri 路径', () => {
  it('readDir 以 { root, path } 调 fs_read_dir，返回去掉 fallback 标注的契约结果', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    const listing: DirListingResult = {
      kind: 'dir_listing',
      path: 'D:/proj',
      entries: [{ name: 'a.rs', path: 'D:/proj/a.rs', kind: 'file', size: 3 }],
      generation: 7,
    }
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return listing
      },
    })
    const res: BridgeResult<DirListingResult> = await readDir('D:/proj', 'D:/proj')
    expect(calls).toEqual([{ cmd: 'fs_read_dir', args: { root: 'D:/proj', path: 'D:/proj' } }])
    expect(res).toEqual({ ok: true, data: listing, fallback: false })
  })

  it('命令 Err：reject 的 ErrorBody 原样进入 error 分支', async () => {
    installTauri({
      invoke: async () => {
        throw { code: 'path_outside_root', message: 'path outside project root: x', detail: null }
      },
    })
    const res = await readDir('D:/proj', 'D:/elsewhere')
    expect(res.ok).toBe(false)
    if (!res.ok) {
      expect(res.fallback).toBe(false)
      expect(res.error.code).toBe('path_outside_root')
      expect(res.error.message).toContain('outside')
    }
  })

  it('reject 载荷非 ErrorBody 时归入 internal（不盲信 IPC 对端）', async () => {
    installTauri({ invoke: async () => Promise.reject('boom') })
    const res = await readDir('D:/proj', 'D:/proj')
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.code).toBe('internal')
  })

  it('命令返回非预期 kind 归入 internal（契约形状校验）', async () => {
    installTauri({ invoke: async () => ({ kind: 'ack' }) satisfies CommandResult })
    const res = await readDir('D:/proj', 'D:/proj')
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error.code).toBe('internal')
  })

  it('node_detect 以空参数调用', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { kind: 'node_status', status: { kind: 'need_manual', attempts: [] } }
      },
    })
    const res = await detectNode()
    expect(calls).toEqual([{ cmd: 'node_detect', args: {} }])
    expect(res.ok).toBe(true)
  })
})
