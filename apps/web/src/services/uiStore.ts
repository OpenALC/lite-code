// UI 状态 store：纯 TS + subscribe，不 import 框架 API。
import { createStore, type Store } from './store'
import type { UiMode } from './layoutProfile'

export interface UiState {
  mode: UiMode
  windowWidth: number
}

export const uiStore: Store<UiState> = createStore<UiState>({
  mode: 'editor',
  windowWidth: typeof window === 'undefined' ? 1280 : window.innerWidth,
})

export function setMode(mode: UiMode): void {
  uiStore.set({ ...uiStore.get(), mode })
}

export function setWindowWidth(width: number): void {
  uiStore.set({ ...uiStore.get(), windowWidth: width })
}
