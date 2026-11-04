// 框架无关的极简响应式 store（UI.md §8：services 只暴露纯函数与 subscribe()）。
export interface Store<T> {
  get(): T
  set(next: T): void
  subscribe(fn: (state: T) => void): () => void
}

export function createStore<T>(initial: T): Store<T> {
  let state = initial
  const subs = new Set<(s: T) => void>()
  return {
    get: () => state,
    set(next: T) {
      state = next
      for (const fn of subs) fn(state)
    },
    subscribe(fn: (s: T) => void) {
      subs.add(fn)
      return () => {
        subs.delete(fn)
      }
    },
  }
}
