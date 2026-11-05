// Git 面板状态与控制器（docs/MILESTONES.md P5 周边；契约 v0.7）。
// 框架无关：不可变状态 + 依赖注入的 bridge 控制器（UI.md §8）。
//
// Git 命令没有事件流：每次写操作成功后主动 refresh status；
// commit 额外刷新 log。diff 请求做序号校验，快速切换文件时旧响应不会覆盖新选择。

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

/** 最近提交默认条数；loadLog(null) 表示不设上限（契约 git_log.limit）。 */
export const DEFAULT_GIT_LOG_LIMIT = 20

export interface GitError {
  code: ErrorCode
  message: string
}

export interface GitState {
  status: GitStatus | null
  selectedPath: string | null
  diff: string | null
  /** 当前 diff 读的是 index（true）还是工作区（false）。 */
  diffStaged: boolean
  log: GitCommit[]
  commitMessage: string
  loading: boolean
  error: GitError | null
}

export function createGitState(): GitState {
  return {
    status: null,
    selectedPath: null,
    diff: null,
    diffStaged: false,
    log: [],
    commitMessage: '',
    loading: false,
    error: null,
  }
}

export interface GroupedGitFiles {
  staged: GitFileStatus[]
  unstaged: GitFileStatus[]
}

/** 同一路径可能同时出现在 staged 与 unstaged（部分暂存），两边都要保留。 */
export function groupFiles(status: GitStatus | null): GroupedGitFiles {
  const staged: GitFileStatus[] = []
  const unstaged: GitFileStatus[] = []
  for (const file of status?.files ?? []) {
    if (file.staged) staged.push(file)
    else unstaged.push(file)
  }
  return { staged, unstaged }
}

const STATUS_LABELS: Record<ChangeStatus, string> = {
  modified: 'M',
  added: 'A',
  deleted: 'D',
  renamed: 'R',
}

/** 单字母状态（UI.md §2 的 M/A/D/R；颜色由 statusTone 决定）。 */
export function statusLabel(status: ChangeStatus): string {
  return STATUS_LABELS[status]
}

export type GitStatusTone = 'warning' | 'success' | 'danger' | 'accent'

const STATUS_TONES: Record<ChangeStatus, GitStatusTone> = {
  modified: 'warning',
  added: 'success',
  deleted: 'danger',
  renamed: 'accent',
}

/** 状态色调：M 黄 / A 绿 / D 红 / R 强调色，视图层映射到 CSS 变量。 */
export function statusTone(status: ChangeStatus): GitStatusTone {
  return STATUS_TONES[status]
}

/** 提交按钮可用条件：有暂存文件、消息非空、且没有进行中的命令。 */
export function canCommit(state: GitState): boolean {
  return (
    state.status !== null &&
    !state.loading &&
    state.commitMessage.trim().length > 0 &&
    groupFiles(state.status).staged.length > 0
  )
}

/** "↑2 ↓1"；分叉为 0 的方向省略，干净时返回空串。 */
export function aheadBehindLabel(
  status: Pick<GitStatus, 'ahead' | 'behind'> | null,
): string {
  if (!status) return ''
  const ahead = Math.max(0, Math.trunc(status.ahead))
  const behind = Math.max(0, Math.trunc(status.behind))
  const parts: string[] = []
  if (ahead > 0) parts.push(`↑${ahead}`)
  if (behind > 0) parts.push(`↓${behind}`)
  return parts.join(' ')
}

function hasFile(status: GitStatus, path: string, staged: boolean): boolean {
  return status.files.some(file => file.path === path && file.staged === staged)
}

function targetsOf(paths: readonly string[]): string[] {
  return [...new Set(paths.filter(path => path.length > 0))]
}

export interface GitControllerDeps {
  gitStatus: () => Promise<BridgeResult<GitStatusResult>>
  gitDiff: (path: string | null, staged: boolean) => Promise<BridgeResult<GitDiffResult>>
  gitStage: (paths: string[]) => Promise<BridgeResult<AckResult>>
  gitUnstage: (paths: string[]) => Promise<BridgeResult<AckResult>>
  gitCommit: (message: string) => Promise<BridgeResult<AckResult>>
  gitLog: (limit: number | null) => Promise<BridgeResult<GitLogResult>>
}

export interface GitController {
  getState(): GitState
  subscribe(listener: (state: GitState) => void): () => void
  refresh(): Promise<boolean>
  /** 选中文件并加载 diff；staged 决定读暂存区还是工作区。 */
  selectFile(path: string, staged: boolean): Promise<boolean>
  stage(paths: string[]): Promise<boolean>
  unstage(paths: string[]): Promise<boolean>
  commit(message: string): Promise<boolean>
  loadLog(limit?: number | null): Promise<boolean>
  setCommitMessage(message: string): void
  clearError(): void
  dispose(): void
}

export function createGitController(deps: GitControllerDeps): GitController {
  let state = createGitState()
  const listeners = new Set<(state: GitState) => void>()
  let disposed = false
  let diffRequest = 0

  function commitState(next: GitState): void {
    state = next
    if (disposed) return
    for (const listener of [...listeners]) listener(state)
  }

  function failed(error: GitError): void {
    commitState({
      ...state,
      loading: false,
      error: { code: error.code, message: error.message },
    })
  }

  async function refresh(): Promise<boolean> {
    if (disposed) return false
    commitState({ ...state, loading: true, error: null })
    const res = await deps.gitStatus()
    if (disposed) return false
    if (!res.ok) {
      failed(res.error)
      return false
    }
    const status = res.data.status
    let next: GitState = { ...state, status, loading: false, error: null }
    // 文件被提交/还原后不再出现在同一分组：收起旧 diff，避免展示过期内容。
    if (
      next.selectedPath !== null &&
      !hasFile(status, next.selectedPath, next.diffStaged)
    ) {
      next = { ...next, selectedPath: null, diff: null }
    }
    commitState(next)
    return true
  }

  async function selectFile(path: string, staged: boolean): Promise<boolean> {
    if (disposed) return false
    const request = ++diffRequest
    commitState({
      ...state,
      selectedPath: path,
      diffStaged: staged,
      diff: null,
      loading: true,
      error: null,
    })
    const res = await deps.gitDiff(path, staged)
    if (disposed) return false
    // 只接受当前选择对应的响应；旧请求直接丢弃。
    if (
      request !== diffRequest ||
      state.selectedPath !== path ||
      state.diffStaged !== staged
    ) {
      return false
    }
    if (!res.ok) {
      failed(res.error)
      return false
    }
    commitState({ ...state, loading: false, error: null, diff: res.data.patch })
    return true
  }

  async function reloadSelected(path: string, staged: boolean): Promise<void> {
    if (state.selectedPath !== path) return
    await selectFile(path, staged)
  }

  async function stage(paths: string[]): Promise<boolean> {
    if (disposed) return false
    const targets = targetsOf(paths)
    if (targets.length === 0) return false
    const selected = state.selectedPath
    commitState({ ...state, loading: true, error: null })
    const res = await deps.gitStage(targets)
    if (disposed) return false
    if (!res.ok) {
      failed(res.error)
      return false
    }
    commitState({ ...state, loading: false, error: null })
    await refresh()
    if (disposed) return false
    if (selected !== null && targets.includes(selected)) {
      await reloadSelected(selected, true)
    }
    return true
  }

  async function unstage(paths: string[]): Promise<boolean> {
    if (disposed) return false
    const targets = targetsOf(paths)
    if (targets.length === 0) return false
    const selected = state.selectedPath
    commitState({ ...state, loading: true, error: null })
    const res = await deps.gitUnstage(targets)
    if (disposed) return false
    if (!res.ok) {
      failed(res.error)
      return false
    }
    commitState({ ...state, loading: false, error: null })
    await refresh()
    if (disposed) return false
    if (selected !== null && targets.includes(selected)) {
      await reloadSelected(selected, false)
    }
    return true
  }

  async function loadLog(
    limit: number | null = DEFAULT_GIT_LOG_LIMIT,
  ): Promise<boolean> {
    if (disposed) return false
    commitState({ ...state, loading: true, error: null })
    const res = await deps.gitLog(limit)
    if (disposed) return false
    if (!res.ok) {
      failed(res.error)
      return false
    }
    commitState({ ...state, loading: false, error: null, log: res.data.commits })
    return true
  }

  async function commit(message: string): Promise<boolean> {
    if (disposed) return false
    const text = message.trim()
    if (!canCommit({ ...state, commitMessage: message })) return false
    commitState({ ...state, loading: true, error: null })
    const res = await deps.gitCommit(text)
    if (disposed) return false
    if (!res.ok) {
      failed(res.error)
      return false
    }
    commitState({ ...state, loading: false, error: null, commitMessage: '' })
    await refresh()
    if (disposed) return false
    await loadLog()
    return true
  }

  function setCommitMessage(message: string): void {
    if (disposed || state.commitMessage === message) return
    commitState({ ...state, commitMessage: message })
  }

  function clearError(): void {
    if (disposed || state.error === null) return
    commitState({ ...state, error: null })
  }

  function getState(): GitState {
    return state
  }

  function subscribe(listener: (state: GitState) => void): () => void {
    listeners.add(listener)
    return () => {
      listeners.delete(listener)
    }
  }

  function dispose(): void {
    if (disposed) return
    disposed = true
    diffRequest += 1
    listeners.clear()
  }

  return {
    getState,
    subscribe,
    refresh,
    selectFile,
    stage,
    unstage,
    commit,
    loadLog,
    setCommitMessage,
    clearError,
    dispose,
  }
}
