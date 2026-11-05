import { describe, expect, it } from 'vitest'

import type {
  ChangeStatus,
  ErrorCode,
  GitCommit,
  GitFileStatus,
  GitStatus,
} from '@lc-proto'

import type {
  AckResult,
  BridgeResult,
  GitDiffResult,
  GitLogResult,
  GitStatusResult,
} from './bridge'
import {
  aheadBehindLabel,
  canCommit,
  createGitController,
  createGitState,
  DEFAULT_GIT_LOG_LIMIT,
  groupFiles,
  statusLabel,
  statusTone,
  type GitControllerDeps,
  type GitState,
} from './gitStore'

function file(overrides: Partial<GitFileStatus> = {}): GitFileStatus {
  return {
    path: 'src/main.ts',
    status: 'modified',
    staged: false,
    added: 3,
    removed: 1,
    ...overrides,
  }
}

function gitStatus(
  files: GitFileStatus[] = [],
  overrides: Partial<GitStatus> = {},
): GitStatus {
  return {
    branch: 'main',
    upstream: 'origin/main',
    ahead: 0,
    behind: 0,
    clean: files.length === 0,
    merge_in_progress: false,
    rebase_in_progress: false,
    files,
    ...overrides,
  }
}

function okStatus(status: GitStatus): BridgeResult<GitStatusResult> {
  return { ok: true, data: { kind: 'git_status', status }, fallback: false }
}

function okDiff(patch = '--- a\n+++ b\n'): BridgeResult<GitDiffResult> {
  return {
    ok: true,
    data: { kind: 'git_diff', patch, files: [] },
    fallback: false,
  }
}

function okLog(commits: GitCommit[] = []): BridgeResult<GitLogResult> {
  return { ok: true, data: { kind: 'git_log', commits }, fallback: false }
}

function okAck(): BridgeResult<AckResult> {
  return { ok: true, data: { kind: 'ack' }, fallback: false }
}

function fail<T>(
  message = 'boom',
  code: ErrorCode = 'git_unavailable',
): BridgeResult<T> {
  return { ok: false, error: { code, message, detail: null }, fallback: false }
}

function recordCommit(overrides: Partial<GitCommit> = {}): GitCommit {
  return {
    sha: 'abcdef1234567890',
    short_sha: 'abcdef1',
    author: 'Dev',
    message: 'feat: demo',
    timestamp: 1_700_000_000,
    ...overrides,
  }
}

interface HarnessCalls {
  status: number
  diff: Array<{ path: string | null; staged: boolean }>
  stage: string[][]
  unstage: string[][]
  commit: string[]
  log: Array<number | null>
}

interface HarnessOptions {
  statusResults?: BridgeResult<GitStatusResult>[]
  diff?: (
    path: string | null,
    staged: boolean,
  ) => BridgeResult<GitDiffResult> | Promise<BridgeResult<GitDiffResult>>
  stageResults?: BridgeResult<AckResult>[]
  unstageResults?: BridgeResult<AckResult>[]
  commitResults?: BridgeResult<AckResult>[]
  logResults?: BridgeResult<GitLogResult>[]
}

function next<T>(queue: T[]): T {
  if (queue.length > 1) return queue.shift() as T
  return queue[0] as T
}

function harness(options: HarnessOptions = {}) {
  const calls: HarnessCalls = {
    status: 0,
    diff: [],
    stage: [],
    unstage: [],
    commit: [],
    log: [],
  }
  const statusResults = options.statusResults ?? [okStatus(gitStatus())]
  const stageResults = options.stageResults ?? [okAck()]
  const unstageResults = options.unstageResults ?? [okAck()]
  const commitResults = options.commitResults ?? [okAck()]
  const logResults = options.logResults ?? [okLog()]

  const deps: GitControllerDeps = {
    gitStatus: async () => {
      calls.status += 1
      return next(statusResults)
    },
    gitDiff: async (path, staged) => {
      calls.diff.push({ path, staged })
      if (options.diff) return options.diff(path, staged)
      return okDiff(`patch:${path}:${staged}`)
    },
    gitStage: async paths => {
      calls.stage.push(paths)
      return next(stageResults)
    },
    gitUnstage: async paths => {
      calls.unstage.push(paths)
      return next(unstageResults)
    },
    gitCommit: async message => {
      calls.commit.push(message)
      return next(commitResults)
    },
    gitLog: async limit => {
      calls.log.push(limit)
      return next(logResults)
    },
  }

  return { controller: createGitController(deps), calls }
}

function stateWith(overrides: Partial<GitState> = {}): GitState {
  return { ...createGitState(), ...overrides }
}

describe('Git 纯函数', () => {
  it('groupFiles 按 staged 分组，同一路径可同时出现在两侧', () => {
    const modified = file({ path: 'a.ts', staged: false })
    const staged = file({ path: 'a.ts', staged: true })
    const added = file({ path: 'b.ts', status: 'added', staged: true, added: 9, removed: 0 })
    const grouped = groupFiles(gitStatus([modified, staged, added]))
    expect(grouped.staged.map(item => item.path)).toEqual(['a.ts', 'b.ts'])
    expect(grouped.unstaged.map(item => item.path)).toEqual(['a.ts'])
  })

  it('groupFiles(null) 返回空分组', () => {
    expect(groupFiles(null)).toEqual({ staged: [], unstaged: [] })
  })

  it('statusLabel 映射 M/A/D/R', () => {
    const statuses: ChangeStatus[] = ['modified', 'added', 'deleted', 'renamed']
    expect(statuses.map(statusLabel)).toEqual(['M', 'A', 'D', 'R'])
  })

  it('statusTone 映射黄/绿/红/强调色', () => {
    expect(statusTone('modified')).toBe('warning')
    expect(statusTone('added')).toBe('success')
    expect(statusTone('deleted')).toBe('danger')
    expect(statusTone('renamed')).toBe('accent')
  })

  it('canCommit 要求已暂存文件、非空消息且不在加载中', () => {
    const staged = gitStatus([file({ staged: true })])
    const unstaged = gitStatus([file({ staged: false })])
    expect(canCommit(createGitState())).toBe(false)
    expect(canCommit(stateWith({ status: staged }))).toBe(false)
    expect(canCommit(stateWith({ status: unstaged, commitMessage: 'fix' }))).toBe(false)
    expect(
      canCommit(stateWith({ status: staged, commitMessage: '   ', loading: false })),
    ).toBe(false)
    expect(
      canCommit(stateWith({ status: staged, commitMessage: 'fix: demo' })),
    ).toBe(true)
    expect(
      canCommit(stateWith({ status: staged, commitMessage: 'fix: demo', loading: true })),
    ).toBe(false)
  })

  it('aheadBehindLabel 只显示非零方向', () => {
    expect(aheadBehindLabel(null)).toBe('')
    expect(aheadBehindLabel({ ahead: 0, behind: 0 })).toBe('')
    expect(aheadBehindLabel({ ahead: 2, behind: 0 })).toBe('↑2')
    expect(aheadBehindLabel({ ahead: 0, behind: 3 })).toBe('↓3')
    expect(aheadBehindLabel({ ahead: 2, behind: 3 })).toBe('↑2 ↓3')
  })
})

describe('Git 控制器', () => {
  it('refresh 加载 status 并清理错误', async () => {
    const initial = gitStatus([file()], { branch: 'agent/git-ui', ahead: 1 })
    const { controller, calls } = harness({ statusResults: [okStatus(initial)] })
    expect(await controller.refresh()).toBe(true)
    expect(calls.status).toBe(1)
    expect(controller.getState().status).toEqual(initial)
    expect(controller.getState().loading).toBe(false)
    expect(controller.getState().error).toBeNull()
  })

  it('refresh 失败保留旧 status 并写入错误码', async () => {
    const { controller } = harness({
      statusResults: [fail('not a repository', 'git_unavailable')],
    })
    expect(await controller.refresh()).toBe(false)
    expect(controller.getState().error).toEqual({
      code: 'git_unavailable',
      message: 'not a repository',
    })
    expect(controller.getState().loading).toBe(false)
  })

  it('selectFile 记录选择、分段标记与 patch', async () => {
    const { controller, calls } = harness()
    expect(await controller.selectFile('src/main.ts', true)).toBe(true)
    expect(calls.diff).toEqual([{ path: 'src/main.ts', staged: true }])
    expect(controller.getState()).toMatchObject({
      selectedPath: 'src/main.ts',
      diffStaged: true,
      diff: 'patch:src/main.ts:true',
      loading: false,
      error: null,
    })
  })

  it('selectFile 失败清空 diff 并暴露错误', async () => {
    const { controller } = harness({ diff: async () => fail('bad path', 'git_rejected') })
    expect(await controller.selectFile('missing.ts', false)).toBe(false)
    expect(controller.getState().diff).toBeNull()
    expect(controller.getState().error?.code).toBe('git_rejected')
  })

  it('selectFile 快速切换时丢弃旧 diff 响应', async () => {
    let release!: () => void
    const gate = new Promise<void>(resolve => {
      release = resolve
    })
    const { controller } = harness({
      diff: async path => {
        if (path === 'a.ts') {
          await gate
          return okDiff('patch:a')
        }
        return okDiff('patch:b')
      },
    })
    const first = controller.selectFile('a.ts', false)
    const second = controller.selectFile('b.ts', false)
    expect(await second).toBe(true)
    release()
    expect(await first).toBe(false)
    expect(controller.getState().selectedPath).toBe('b.ts')
    expect(controller.getState().diff).toBe('patch:b')
  })

  it('stage 去重空路径、刷新 status 并返回成功', async () => {
    const before = gitStatus([file({ path: 'a.ts' })])
    const after = gitStatus([file({ path: 'a.ts', staged: true })])
    const { controller, calls } = harness({
      statusResults: [okStatus(before), okStatus(after)],
    })
    await controller.refresh()
    expect(await controller.stage(['a.ts', 'a.ts', ''])).toBe(true)
    expect(calls.stage).toEqual([['a.ts']])
    expect(calls.status).toBe(2)
    expect(controller.getState().status).toEqual(after)
  })

  it('stage 失败不刷新 status 并保留错误', async () => {
    const { controller, calls } = harness({
      statusResults: [okStatus(gitStatus([file()]))],
      stageResults: [fail('index locked', 'git_rejected')],
    })
    expect(await controller.stage(['a.ts'])).toBe(false)
    expect(calls.status).toBe(0)
    expect(controller.getState().error?.message).toBe('index locked')
  })

  it('stage 空列表不调用 bridge', async () => {
    const { controller, calls } = harness()
    expect(await controller.stage([])).toBe(false)
    expect(calls.stage).toEqual([])
    expect(calls.status).toBe(0)
  })

  it('stage 当前选中文件后重新加载暂存区 diff', async () => {
    const before = gitStatus([file({ path: 'a.ts', staged: false })])
    const after = gitStatus([file({ path: 'a.ts', staged: true, added: 5 })])
    const { controller, calls } = harness({
      statusResults: [okStatus(before), okStatus(after)],
      diff: async (_path, staged) => okDiff(staged ? 'staged patch' : 'worktree patch'),
    })
    await controller.selectFile('a.ts', false)
    expect(await controller.stage(['a.ts'])).toBe(true)
    expect(calls.diff).toEqual([
      { path: 'a.ts', staged: false },
      { path: 'a.ts', staged: true },
    ])
    expect(controller.getState()).toMatchObject({
      selectedPath: 'a.ts',
      diffStaged: true,
      diff: 'staged patch',
    })
  })

  it('unstage 刷新 status 并回到工作区 diff', async () => {
    const before = gitStatus([file({ path: 'a.ts', staged: true })])
    const after = gitStatus([file({ path: 'a.ts', staged: false })])
    const { controller, calls } = harness({
      statusResults: [okStatus(before), okStatus(after)],
      diff: async (_path, staged) => okDiff(staged ? 'indexed' : 'worktree'),
    })
    await controller.selectFile('a.ts', true)
    expect(await controller.unstage(['a.ts'])).toBe(true)
    expect(calls.unstage).toEqual([['a.ts']])
    expect(calls.status).toBe(1)
    expect(controller.getState().diffStaged).toBe(false)
    expect(controller.getState().diff).toBe('worktree')
  })

  it('commit 在没有暂存文件或消息为空时不调用 bridge', async () => {
    const { controller, calls } = harness({
      statusResults: [okStatus(gitStatus([file({ staged: false })]))],
    })
    await controller.refresh()
    controller.setCommitMessage('fix')
    expect(await controller.commit('fix')).toBe(false)
    controller.setCommitMessage('')
    expect(await controller.commit('')).toBe(false)
    expect(calls.commit).toEqual([])
  })

  it('commit 成功后清空消息、刷新 status 与 log', async () => {
    const staged = gitStatus([file({ path: 'a.ts', staged: true })])
    const clean = gitStatus([])
    const entry = recordCommit()
    const { controller, calls } = harness({
      statusResults: [okStatus(staged), okStatus(clean)],
      logResults: [okLog([entry])],
    })
    await controller.refresh()
    controller.setCommitMessage('  feat: git panel  ')
    expect(await controller.commit('  feat: git panel  ')).toBe(true)
    expect(calls.commit).toEqual(['feat: git panel'])
    expect(calls.status).toBe(2)
    expect(calls.log).toEqual([DEFAULT_GIT_LOG_LIMIT])
    expect(controller.getState().commitMessage).toBe('')
    expect(controller.getState().status).toEqual(clean)
    expect(controller.getState().log).toEqual([entry])
  })

  it('commit 失败保留消息并写入错误', async () => {
    const staged = gitStatus([file({ staged: true })])
    const { controller } = harness({
      statusResults: [okStatus(staged)],
      commitResults: [fail('nothing to commit', 'git_rejected')],
    })
    await controller.refresh()
    controller.setCommitMessage('fix')
    expect(await controller.commit('fix')).toBe(false)
    expect(controller.getState().commitMessage).toBe('fix')
    expect(controller.getState().error).toEqual({
      code: 'git_rejected',
      message: 'nothing to commit',
    })
  })

  it('loadLog 默认 20 条，可显式传 null / limit', async () => {
    const entry = recordCommit()
    const { controller, calls } = harness({ logResults: [okLog([entry])] })
    expect(await controller.loadLog()).toBe(true)
    expect(controller.getState().log).toEqual([entry])
    expect(await controller.loadLog(null)).toBe(true)
    expect(await controller.loadLog(5)).toBe(true)
    expect(calls.log).toEqual([DEFAULT_GIT_LOG_LIMIT, null, 5])
  })

  it('loadLog 失败保留旧列表并写入错误', async () => {
    const entry = recordCommit()
    const { controller } = harness({
      logResults: [okLog([entry]), fail('log failed', 'git_unavailable')],
    })
    await controller.loadLog()
    expect(await controller.loadLog()).toBe(false)
    expect(controller.getState().log).toEqual([entry])
    expect(controller.getState().error?.message).toBe('log failed')
  })

  it('refresh 发现选中文件消失时收起 diff', async () => {
    const withFile = gitStatus([file({ path: 'a.ts' })])
    const { controller } = harness({
      statusResults: [okStatus(withFile), okStatus(gitStatus([]))],
    })
    await controller.refresh()
    await controller.selectFile('a.ts', false)
    await controller.refresh()
    expect(controller.getState()).toMatchObject({
      selectedPath: null,
      diff: null,
    })
  })

  it('setCommitMessage 更新输入并通知订阅者', () => {
    const { controller } = harness()
    const seen: string[] = []
    const off = controller.subscribe(state => seen.push(state.commitMessage))
    controller.setCommitMessage('hello')
    controller.setCommitMessage('hello')
    controller.setCommitMessage('world')
    off()
    controller.setCommitMessage('ignored')
    expect(seen).toEqual(['hello', 'world'])
  })

  it('dispose 后不再通知且命令返回 false', async () => {
    const { controller, calls } = harness()
    const seen: GitState[] = []
    controller.subscribe(state => seen.push(state))
    controller.dispose()
    expect(await controller.refresh()).toBe(false)
    controller.setCommitMessage('x')
    expect(calls.status).toBe(0)
    expect(seen).toEqual([])
  })
})
