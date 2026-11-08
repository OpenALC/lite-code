// Agent 模式的工作区列表：只存路径元数据，不做文件系统操作。
// 工作区根仍由 Rust WorkspaceState 持有；这里负责侧栏展示与本地记忆。
const STORAGE_KEY = 'lc.agentWorkspaces'

export interface WorkspaceEntry {
  root: string
  name: string
  addedAt: number
}

function normalize(root: string): string {
  const slashed = root.replace(/\\/g, '/')
  return /^[A-Za-z]:\//.test(slashed) ? slashed.toLowerCase() : slashed
}

export function samePath(a: string, b: string): boolean {
  return normalize(a) === normalize(b)
}

export function workspaceName(root: string): string {
  const parts = root.replace(/\\/g, '/').split('/').filter(Boolean)
  return parts.length > 0 ? parts[parts.length - 1]! : root
}

export function upsertWorkspace(
  list: readonly WorkspaceEntry[],
  root: string,
  addedAt = Date.now(),
): WorkspaceEntry[] {
  if (!root.trim()) return [...list]
  const existing = list.find(item => samePath(item.root, root))
  if (existing) {
    return list.map(item =>
      item === existing ? { ...item, root, name: workspaceName(root) } : item,
    )
  }
  return [...list, { root, name: workspaceName(root), addedAt }]
}

export function removeWorkspace(
  list: readonly WorkspaceEntry[],
  root: string,
): WorkspaceEntry[] {
  return list.filter(item => !samePath(item.root, root))
}

export function loadWorkspaces(): WorkspaceEntry[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed)) return []
    return parsed.filter((item): item is WorkspaceEntry => {
      if (!item || typeof item !== 'object') return false
      const value = item as Record<string, unknown>
      return typeof value.root === 'string' && value.root.length > 0
    }).map(item => ({
      root: item.root,
      name: typeof item.name === 'string' && item.name ? item.name : workspaceName(item.root),
      addedAt: Number(item.addedAt ?? 0),
    }))
  } catch {
    return []
  }
}

export function saveWorkspaces(list: readonly WorkspaceEntry[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(list))
  } catch {
    // localStorage 不可用时只影响持久化。
  }
}
