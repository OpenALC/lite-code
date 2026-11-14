<script setup lang="ts">
// 底部面板「终端」（P5 issue #63）：xterm.js 多会话标签（本地 + SSH）。
// store/命令在 services/terminal.ts；本组件只做 xterm 实例生命周期与 UI。
//
// 契约：pty_open / pty_write / pty_resize / pty_close / pty_list / pty_replay，
// 事件 pty:data|pty:state|pty:exit。会话在 Rust 侧存活：面板重开走 restore()。
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'
import '@xterm/xterm/css/xterm.css'

import {
  createSshDraft,
  createTerminalController,
  defaultTerminalDeps,
  isTauriLike,
  validateSshDraft,
  type SshFormDraft,
  type TerminalSession,
  type TerminalState,
} from '../services/terminal'
import { subscribePrefs, themeBaseMode, uiPrefs } from '../services/themeStore'

const { t } = useI18n()
const tauriLike = isTauriLike()

const controller = createTerminalController(defaultTerminalDeps())
const state = ref<TerminalState>(controller.getState())
const unsubController = controller.subscribe(next => {
  state.value = next
})

const sessions = computed(() => state.value.sessions)
const activeId = computed(() => state.value.activeId)
const activeSession = computed<TerminalSession | null>(
  () => sessions.value.find(session => session.info.sessionId === activeId.value) ?? null,
)

/** 会话标签文案：SSH 用 host，本地用 title。 */
function sessionLabel(session: TerminalSession): string {
  return session.info.title
}

// ==== xterm 实例池：每会话一个，切标签销毁/重建挂载 ====

interface TermEntry {
  term: Terminal
  fit: FitAddon
  resizeObserver: ResizeObserver
}

const termHost = ref<HTMLElement | null>(null)
const termEntries = new Map<string, TermEntry>()
/** 已回放过 replay 的会话，避免重复追加。 */
const replayed = new Set<string>()

function themeColors(): Record<string, string> {
  const styles = getComputedStyle(document.documentElement)
  const pick = (name: string, fallback: string): string =>
    styles.getPropertyValue(name).trim() || fallback
  const dark = themeBaseMode(uiPrefs().theme) !== 'light'
  return {
    background: pick('--bg', dark ? '#111318' : '#eef1f6'),
    foreground: pick('--fg', dark ? '#e6e9ef' : '#1c2330'),
    cursor: pick('--accent', '#0a84ff'),
    cursorAccent: pick('--accent-fg', '#ffffff'),
    selectionBackground: pick('--accent-soft', 'rgba(10,132,255,0.16)'),
  }
}

function currentFontFamily(): string {
  return getComputedStyle(document.documentElement).getPropertyValue('--font-mono').trim() ||
    'monospace'
}

function createEntry(sessionId: string): TermEntry | null {
  const host = termHost.value
  if (!host || termEntries.has(sessionId)) return termEntries.get(sessionId) ?? null
  const term = new Terminal({
    convertEol: false,
    fontFamily: currentFontFamily(),
    fontSize: 12,
    theme: themeColors(),
    scrollback: 2000,
  })
  const fit = new FitAddon()
  term.loadAddon(fit)
  term.open(host)
  term.onData(data => {
    void controller.write(sessionId, data)
  })
  const resizeObserver = new ResizeObserver(() => {
    try {
      fit.fit()
      const dims = fit.proposeDimensions()
      if (dims && dims.cols > 0 && dims.rows > 0) {
        void controller.resize(sessionId, dims.cols, dims.rows)
      }
    } catch {
      // 容器不可见时 fit 抛错：忽略，等可见后再触发。
    }
  })
  resizeObserver.observe(host)
  termEntries.set(sessionId, { term, fit, resizeObserver })
  return { term, fit, resizeObserver }
}

function destroyEntry(sessionId: string): void {
  const entry = termEntries.get(sessionId)
  if (!entry) return
  entry.resizeObserver.disconnect()
  entry.term.dispose()
  termEntries.delete(sessionId)
  replayed.delete(sessionId)
}

/** 激活标签时挂载 xterm、回放缓冲并聚焦。 */
async function mountActive(): Promise<void> {
  const sessionId = activeId.value
  if (!sessionId || !termHost.value) return
  await nextTick()
  const session = sessions.value.find(item => item.info.sessionId === sessionId)
  if (!session) return
  let entry = termEntries.get(sessionId)
  if (!entry) {
    const created = createEntry(sessionId)
    if (!created) return
    entry = created
  }
  if (!replayed.has(sessionId)) {
    replayed.add(sessionId)
    const replayedBytes = await controller.replay(sessionId)
    if (replayedBytes !== null && replayedBytes.length > 0) {
      entry.term.write(replayedBytes)
    }
    lastRenderedLength.set(sessionId, session.buffer.length)
  }
  try {
    entry.fit.fit()
  } catch {
    // 容器尺寸未就绪时忽略。
  }
  entry.term.focus()
}

// 活跃标签的实时输出：Rust pty:data 事件 → store.buffer；这里把新增字节写进 xterm。
const lastRenderedLength = new Map<string, number>()
function drainBuffer(): void {
  const sessionId = activeId.value
  if (!sessionId) return
  const entry = termEntries.get(sessionId)
  const session = sessions.value.find(item => item.info.sessionId === sessionId)
  if (!entry || !session) return
  const start = lastRenderedLength.get(sessionId) ?? 0
  if (session.buffer.length > start) {
    entry.term.write(session.buffer.slice(start))
  } else if (session.buffer.length < start) {
    // 缓冲被截断（超上限）或 replay 重置：整段重绘代价过高，标记下次全量。
    entry.term.write(session.buffer)
  }
  lastRenderedLength.set(sessionId, session.buffer.length)
}

watch(state, drainBuffer)

// 会话标签集合变化：销毁已关闭的实例，挂载新激活的。
watch(
  () => sessions.value.map(session => session.info.sessionId).join(','),
  () => {
    const ids = new Set(sessions.value.map(session => session.info.sessionId))
    for (const id of [...termEntries.keys()]) {
      if (!ids.has(id)) destroyEntry(id)
    }
  },
)

watch(activeId, () => {
  void mountActive()
})

/** 切换主题时刷新 xterm 主题与字体。 */
function refreshTheme(): void {
  const colors = themeColors()
  const family = currentFontFamily()
  for (const { term } of termEntries.values()) {
    term.options.theme = colors
    term.options.fontFamily = family
  }
}

const unsubPrefs = subscribePrefs(() => refreshTheme())

// ==== SSH 新建浮层 ====

const sshOpen = ref(false)
const sshDraft = reactive<SshFormDraft>(createSshDraft())
const sshError = ref<string | null>(null)
const openError = ref(false)

function openSshDialog(): void {
  Object.assign(sshDraft, createSshDraft())
  sshError.value = null
  sshOpen.value = true
}

function closeSshDialog(): void {
  sshOpen.value = false
}

function submitSsh(): void {
  const errorKey = validateSshDraft(sshDraft)
  if (errorKey !== null) {
    sshError.value = t(errorKey)
    return
  }
  sshError.value = null
  const host = termHost.value
  const dims = host ? defaultDims(host) : { cols: 80, rows: 24 }
  void (async () => {
    const id = await controller.openSsh(sshDraft, dims.cols, dims.rows, sshDraft.host.trim())
    if (id === null) {
      openError.value = true
      return
    }
    openError.value = false
    sshOpen.value = false
    await mountActive()
  })()
}

function defaultDims(host: HTMLElement): { cols: number; rows: number } {
  // pty_open 需要初始尺寸：按 xterm 默认 cell 估列数（8px 宽 / 17px 高近似）。
  const width = host.clientWidth > 0 ? host.clientWidth : 600
  const height = host.clientHeight > 0 ? host.clientHeight : 300
  return {
    cols: Math.max(20, Math.min(500, Math.floor(width / 8))),
    rows: Math.max(6, Math.min(200, Math.floor(height / 17))),
  }
}

async function newLocal(): Promise<void> {
  const host = termHost.value
  if (!host) return
  const dims = defaultDims(host)
  const id = await controller.openLocal(dims.cols, dims.rows)
  if (id === null) {
    openError.value = true
    return
  }
  openError.value = false
  await mountActive()
}

function activateTab(sessionId: string): void {
  controller.activate(sessionId)
}

async function closeTab(sessionId: string): Promise<void> {
  await controller.close(sessionId)
}

onMounted(() => {
  void controller.restore().then(() => mountActive())
})

onBeforeUnmount(() => {
  unsubPrefs()
  unsubController()
  // 面板卸载：销毁 xterm 实例；会话在 Rust 侧存活，重开面板走 restore() 恢复。
  for (const id of [...termEntries.keys()]) destroyEntry(id)
  controller.dispose()
})
</script>

<template>
  <div class="terminal-panel">
    <div class="term-toolbar">
      <div class="term-tabs">
        <button
          v-for="session in sessions"
          :key="session.info.sessionId"
          type="button"
          class="term-tab"
          :class="{ active: session.info.sessionId === activeId }"
          :title="sessionLabel(session)"
          @click="activateTab(session.info.sessionId)"
        >
          <span
            class="term-state"
            :class="session.info.state"
          />
          <span class="term-label">{{ sessionLabel(session) }}</span>
          <span
            v-if="session.info.state === 'closed' && session.exitCode !== null"
            class="term-exit"
          >{{ t('bottom.exitCode', { code: session.exitCode }) }}</span>
          <span
            class="term-close"
            role="button"
            :title="t('bottom.closeTab')"
            @click.stop="closeTab(session.info.sessionId)"
          >×</span>
        </button>
      </div>
      <div class="term-actions">
        <button
          type="button"
          class="term-btn"
          :title="t('bottom.newLocal')"
          @click="newLocal"
        >
          + {{ t('bottom.newLocal') }}
        </button>
        <button
          type="button"
          class="term-btn"
          :title="t('bottom.newSsh')"
          @click="openSshDialog"
        >
          + {{ t('bottom.newSsh') }}
        </button>
      </div>
    </div>

    <div
      v-if="openError"
      class="term-error"
    >
      {{ t('bottom.openFailed') }}
    </div>

    <div class="term-body">
      <div
        v-show="activeSession"
        ref="termHost"
        class="term-host"
      />
      <div
        v-if="!activeSession"
        class="term-empty"
      >
        <p>{{ t('bottom.noSessions') }}</p>
        <p
          v-if="!tauriLike"
          class="term-empty-hint"
        >
          {{ t('bottom.browserHint') }}
        </p>
      </div>
    </div>

    <div
      v-if="sshOpen"
      class="ssh-mask"
      @click.self="closeSshDialog"
    >
      <form
        class="ssh-dialog"
        @submit.prevent="submitSsh"
      >
        <h3>{{ t('bottom.sshTitle') }}</h3>
        <label class="ssh-field">
          <span>{{ t('bottom.host') }}</span>
          <input
            v-model="sshDraft.host"
            type="text"
            :placeholder="t('bottom.hostPlaceholder')"
            spellcheck="false"
            autofocus
          >
        </label>
        <div class="ssh-row">
          <label class="ssh-field">
            <span>{{ t('bottom.port') }}</span>
            <input
              v-model="sshDraft.port"
              type="text"
              inputmode="numeric"
              :placeholder="t('bottom.portPlaceholder')"
            >
          </label>
          <label class="ssh-field">
            <span>{{ t('bottom.user') }}</span>
            <input
              v-model="sshDraft.user"
              type="text"
              :placeholder="t('bottom.userPlaceholder')"
              spellcheck="false"
            >
          </label>
        </div>
        <p
          v-if="sshError"
          class="ssh-error"
        >
          {{ sshError }}
        </p>
        <div class="ssh-actions">
          <button
            type="button"
            class="term-btn"
            @click="closeSshDialog"
          >
            {{ t('bottom.cancel') }}
          </button>
          <button
            type="submit"
            class="term-btn primary"
          >
            {{ t('bottom.connect') }}
          </button>
        </div>
      </form>
    </div>
  </div>
</template>

<style scoped>
.terminal-panel {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.term-toolbar {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 8px;
  border-bottom: 1px solid var(--border);
}

.term-tabs {
  display: flex;
  align-items: center;
  gap: 2px;
  overflow-x: auto;
  min-width: 0;
}

.term-tab {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 24px;
  padding: 0 6px;
  border-radius: var(--r-xs);
  color: var(--fg-dim);
  font-size: 11px;
  white-space: nowrap;
  cursor: pointer;
}

.term-tab:hover {
  background: var(--panel-3);
  color: var(--fg);
}

.term-tab.active {
  background: var(--accent-soft);
  color: var(--accent);
}

.term-state {
  flex: none;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--green);
}

.term-state.reconnecting {
  background: var(--yellow);
}

.term-state.closed {
  background: var(--red);
}

.term-label {
  max-width: 140px;
  overflow: hidden;
  text-overflow: ellipsis;
}

.term-exit {
  color: var(--fg-faint);
  font-size: 10px;
}

.term-close {
  color: var(--fg-faint);
  font-size: 12px;
  padding: 0 2px;
}

.term-close:hover {
  color: var(--red);
}

.term-actions {
  margin-left: auto;
  display: flex;
  gap: 4px;
}

.term-btn {
  height: 22px;
  padding: 0 8px;
  border-radius: var(--r-xs);
  color: var(--fg-dim);
  font-size: 11px;
  cursor: pointer;
}

.term-btn:hover {
  background: var(--panel-3);
  color: var(--fg);
}

.term-btn.primary {
  background: var(--accent);
  color: var(--accent-fg);
}

.term-btn.primary:hover {
  background: var(--accent-hov);
}

.term-error {
  padding: 4px 10px;
  color: var(--red);
  font-size: 11px;
  border-bottom: 1px solid var(--border);
}

.term-body {
  flex: 1;
  min-height: 0;
  position: relative;
}

.term-host {
  position: absolute;
  inset: 0;
  padding: 4px 8px;
}

.term-empty {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 4px;
  color: var(--fg-faint);
  font-size: 12px;
}

.term-empty-hint {
  font-size: 11px;
  opacity: 0.75;
}

.ssh-mask {
  position: fixed;
  inset: 0;
  z-index: 95;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgb(0 0 0 / 45%);
}

.ssh-dialog {
  width: min(360px, calc(100vw - 48px));
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 14px 16px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--r);
  box-shadow: var(--shadow);
}

.ssh-dialog h3 {
  margin: 0;
  color: var(--fg);
  font-size: 13px;
  font-weight: 650;
}

.ssh-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  flex: 1;
}

.ssh-field span {
  color: var(--fg-dim);
  font-size: 11px;
}

.ssh-field input {
  height: 28px;
  padding: 0 8px;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: var(--r-xs);
  color: var(--fg);
  font-size: 12px;
}

.ssh-field input:focus {
  border-color: var(--accent-line);
  outline: none;
}

.ssh-row {
  display: flex;
  gap: 8px;
}

.ssh-error {
  margin: 0;
  color: var(--red);
  font-size: 11px;
}

.ssh-actions {
  display: flex;
  justify-content: flex-end;
  gap: 6px;
  margin-top: 2px;
}
</style>
