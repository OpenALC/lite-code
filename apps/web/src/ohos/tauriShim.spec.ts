import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const HISTORY_KEY = 'lite-code.ohos.history'

interface TauriInternals {
  invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>
}

class MemoryStorage {
  private readonly data = new Map<string, string>()

  get length(): number {
    return this.data.size
  }

  clear(): void {
    this.data.clear()
  }

  getItem(key: string): string | null {
    return this.data.get(key) ?? null
  }

  key(index: number): string | null {
    return [...this.data.keys()][index] ?? null
  }

  removeItem(key: string): void {
    this.data.delete(key)
  }

  setItem(key: string, value: string): void {
    this.data.set(key, value)
  }
}

type NativeInvoke = (cmd: string, argsJson: string) => Promise<string>

function installStorage(storage: MemoryStorage): void {
  ;(globalThis as unknown as { localStorage: Storage }).localStorage =
    storage as unknown as Storage
}

function installWindow(invoke: NativeInvoke): Record<string, unknown> {
  const target: Record<string, unknown> = {
    __lcNative: { invoke },
  }
  ;(globalThis as unknown as { window: Record<string, unknown> }).window = target
  return target
}

function internals(target: Record<string, unknown>): TauriInternals {
  return target.__TAURI_INTERNALS__ as TauriInternals
}

async function loadShim(): Promise<void> {
  vi.resetModules()
  await import('./tauriShim')
}

function historyPayload(title: string): Record<string, unknown> {
  return {
    title,
    savedAt: 1,
    workspace: '/work',
    mode: 'plan',
    model: 'stub-chat',
    agent: {
      sessionId: 'legacy',
      turn: 1,
      messages: [{ role: 'user', text: title }],
    },
  }
}

describe('ohos tauriShim 历史迁移', () => {
  beforeEach(() => {
    vi.spyOn(console, 'info').mockImplementation(() => {})
  })

  afterEach(() => {
    vi.restoreAllMocks()
    vi.resetModules()
    delete (globalThis as unknown as { window?: unknown }).window
    delete (globalThis as unknown as { localStorage?: unknown }).localStorage
  })

  it('首次检测到原生桥时逐条导入并清除 localStorage，重复调用不重复导入', async () => {
    const storage = new MemoryStorage()
    storage.setItem(
      HISTORY_KEY,
      JSON.stringify({
        'legacy-1': historyPayload('旧会话一'),
        'legacy-2': historyPayload('旧会话二'),
      }),
    )
    installStorage(storage)

    const calls: Array<{ cmd: string; args: Record<string, unknown> }> = []
    const target = installWindow(async (cmd, argsJson) => {
      calls.push({ cmd, args: JSON.parse(argsJson) as Record<string, unknown> })
      return cmd === 'history_list' ? '[]' : 'null'
    })

    await loadShim()
    expect(await internals(target).invoke('history_list')).toEqual([])

    const saves = calls.filter(call => call.cmd === 'history_save')
    expect(saves.map(call => call.args.id).sort()).toEqual(['legacy-1', 'legacy-2'])
    expect(storage.getItem(HISTORY_KEY)).toBeNull()
    expect(console.info).toHaveBeenCalledWith(
      expect.stringContaining('已迁移 2 条历史并清除'),
    )

    await internals(target).invoke('history_list')
    expect(calls.filter(call => call.cmd === 'history_save')).toHaveLength(2)
    expect(storage.getItem(HISTORY_KEY)).toBeNull()
  })

  it('任一记录导入失败时保留 localStorage，下一次命令重试成功后清除', async () => {
    const storage = new MemoryStorage()
    storage.setItem(HISTORY_KEY, JSON.stringify({ 'legacy-1': historyPayload('旧会话') }))
    installStorage(storage)

    let fail = true
    let saves = 0
    const target = installWindow(async (cmd) => {
      if (cmd === 'history_list') return '[]'
      if (cmd === 'history_save') {
        saves += 1
        if (fail) {
          throw { code: 'io', message: 'disk full', detail: null }
        }
      }
      return 'null'
    })

    await loadShim()
    // 启动时的首次尝试失败；下一条 history 命令才会再次尝试。
    await new Promise(resolve => setTimeout(resolve, 0))
    expect(storage.getItem(HISTORY_KEY)).not.toBeNull()
    expect(saves).toBe(1)
    expect(console.info).toHaveBeenCalledWith(
      expect.stringContaining('保留 localStorage 待下次重试'),
    )

    fail = false
    await internals(target).invoke('history_list')
    expect(storage.getItem(HISTORY_KEY)).toBeNull()
    expect(saves).toBe(2)
  })

  it('迁移成功后重复执行 history 命令不再触发导入', async () => {
    const storage = new MemoryStorage()
    installStorage(storage)

    const target = installWindow(async cmd => (cmd === 'history_list' ? '[]' : 'null'))
    await loadShim()
    await internals(target).invoke('history_list')

    storage.setItem(HISTORY_KEY, JSON.stringify({ late: historyPayload('迟到的旧会话') }))
    await internals(target).invoke('history_list')
    await internals(target).invoke('history_list')
    // 一次性迁移完成后，即使 key 再次出现也不再导入。
    expect(storage.getItem(HISTORY_KEY)).not.toBeNull()
  })

  it('history_rename / history_archive 优先走原生命令', async () => {
    installStorage(new MemoryStorage())
    const calls: Array<{ cmd: string; args: Record<string, unknown> }> = []
    const target = installWindow(async (cmd, argsJson) => {
      calls.push({ cmd, args: JSON.parse(argsJson) as Record<string, unknown> })
      return 'null'
    })

    await loadShim()
    await internals(target).invoke('history_rename', { id: 'ohos-1', title: '新标题' })
    await internals(target).invoke('history_archive', { id: 'ohos-1', archived: true })

    expect(calls).toEqual([
      { cmd: 'history_rename', args: { id: 'ohos-1', title: '新标题' } },
      { cmd: 'history_archive', args: { id: 'ohos-1', archived: true } },
    ])
  })
})
