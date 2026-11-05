import { afterEach, describe, expect, it } from 'vitest'

import type { CommandResult } from '@lc-proto'

import {
  detectNode,
  downloadNodePortable,
  isTauri,
  readFile,
  readDir,
  setNodeManual,
  workspaceRoot,
  writeFile,
  type BridgeResult,
  type DirListingResult,
} from './bridge'

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

  it('workspaceRoot 返回 null + fallback（无真实工作区）', async () => {
    const res = await workspaceRoot()
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) expect(res.data).toEqual({ kind: 'workspace_root', root: null })
  })

  it('writeFile 返回 ack + fallback（不写真实磁盘）', async () => {
    const res = await writeFile('README.md', 'demo')
    expect(res.ok).toBe(true)
    expect(res.fallback).toBe(true)
    if (res.ok) expect(res.data.kind).toBe('ack')
  })

  it('setNodeManual / downloadNodePortable 在浏览器 dev 返回 fallback', async () => {
    for (const res of [await setNodeManual('C:/node.exe'), await downloadNodePortable('22')]) {
      expect(res.ok).toBe(true)
      expect(res.fallback).toBe(true)
      if (res.ok) expect(res.data.status.kind).toBe('need_manual')
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
  it('readDir 只把 path 传给 fs_read_dir（root 由传输层注入）', async () => {
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
    expect(calls).toEqual([{ cmd: 'fs_read_dir', args: { path: 'D:/proj' } }])
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

  it('workspace_root 以空参数调用并返回根', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { kind: 'workspace_root', root: 'D:/proj' }
      },
    })
    const res = await workspaceRoot()
    expect(calls).toEqual([{ cmd: 'workspace_root', args: {} }])
    expect(res).toEqual({
      ok: true,
      data: { kind: 'workspace_root', root: 'D:/proj' },
      fallback: false,
    })
  })

  it('writeFile 以 { path, content } 调 fs_write_file', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { kind: 'ack' }
      },
    })
    const res = await writeFile('D:/proj/a.rs', 'fn main() {}')
    expect(calls).toEqual([
      { cmd: 'fs_write_file', args: { path: 'D:/proj/a.rs', content: 'fn main() {}' } },
    ])
    expect(res).toEqual({ ok: true, data: { kind: 'ack' }, fallback: false })
  })

  it('setNodeManual 以 { path } 调用并返回 ready', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return {
          kind: 'node_status',
          status: {
            kind: 'ready',
            runtime: {
              path: 'D:/node.exe',
              version: { major: 22, minor: 14, patch: 0 },
              source: 'manual',
              health: 'supported',
            },
          },
        }
      },
    })
    const res = await setNodeManual('D:/node.exe')
    expect(calls).toEqual([{ cmd: 'node_set_manual', args: { path: 'D:/node.exe' } }])
    expect(res.ok).toBe(true)
    if (res.ok) expect(res.data.status.kind).toBe('ready')
  })

  it('downloadNodePortable 以 { version: null } 调用', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = []
    installTauri({
      invoke: async (cmd, args) => {
        calls.push({ cmd, args })
        return { kind: 'node_status', status: { kind: 'need_manual', attempts: [] } }
      },
    })
    await downloadNodePortable()
    expect(calls).toEqual([{ cmd: 'node_download_portable', args: { version: null } }])
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
