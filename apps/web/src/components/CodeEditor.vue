<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import type * as monacoNs from 'monaco-editor'

import { lspDidChange, lspDidClose, lspDidOpen, writeFile } from '../services/bridge'
import { pushAgentContext } from '../services/agentContext'
import { consumeReveal, subscribeReveal } from '../services/editorReveal'
import { languageForPath } from '../services/editorLanguage'
import { diagnosticsFor, subscribeDiagnostics } from '../services/lspDiagnostics'
import type { EditorFileStrategy } from '../services/largeFile'
import { activeTab, markSaved, tabsStore, updateContent } from '../services/tabs'
import { subscribePrefs, uiPrefs, type UiPrefs } from '../services/themeStore'
import type { Tab, TabsState } from '../services/tabs'

const { t } = useI18n()

const host = ref<HTMLElement | null>(null)
let editor: monacoNs.editor.IStandaloneCodeEditor | null = null
let monacoRef: typeof monacoNs | null = null
let unsubTabs: (() => void) | null = null
let unsubPrefs: (() => void) | null = null
let unsubReveal: (() => void) | null = null
let unsubDiagnostics: (() => void) | null = null
let lspTimer: ReturnType<typeof setTimeout> | null = null
const lspOpened = new Map<string, string>()
let removeContext: (() => void) | null = null
let currentTabId: string | null = null
/** setValue 触发的 onDidChangeContent 不应回写 store */
let applying = false
/** 卸载后到达的异步挂载（Monaco 动态 import）不得再创建编辑器 */
let disposed = false

function onWindowClick(): void {
  contextMenu.value = null
}

function onWindowKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape') contextMenu.value = null
}

/** 每个标签独立 model：undo/redo、光标与折叠状态随 model 保留。 */
const models = new Map<string, monacoNs.editor.ITextModel>()
const modelDisposables = new Map<string, monacoNs.IDisposable>()
const viewStates = new Map<string, monacoNs.editor.ICodeEditorViewState | null>()

type SaveState = { kind: 'saved' | 'demo' | 'error' | 'sample' | 'readonly'; text: string }
const saveState = ref<SaveState | null>(null)
const activeStrategy = ref<EditorFileStrategy | null>(null)
const contextMenu = ref<{ x: number; y: number } | null>(null)
const hasSelection = ref(false)
/** 右键加入 Agent 的选区上限，避免整文件 Ctrl+A 撑爆草稿与上下文。 */
const MAX_CONTEXT_CHARS = 20_000

function currentTab(): Tab | null {
  return activeTab(tabsStore.get())
}

function modelUri(tab: Tab): monacoNs.Uri {
  const monaco = monacoRef!
  if (tab.path) return monaco.Uri.file(tab.path)
  return monaco.Uri.parse(`inmemory://lite-code/${encodeURIComponent(tab.id)}`)
}

function ensureModel(tab: Tab): monacoNs.editor.ITextModel {
  const existing = models.get(tab.id)
  if (existing) {
    if (existing.getValue() !== tab.content) {
      applying = true
      existing.setValue(tab.content)
      applying = false
    }
    return existing
  }
  const monaco = monacoRef!
  const model = monaco.editor.createModel(
    tab.content,
    languageForPath(tab.path ?? tab.title),
    modelUri(tab),
  )
  model.updateOptions({ tabSize: uiPrefs().tabSize })
  modelDisposables.set(
    tab.id,
    model.onDidChangeContent(() => {
      if (applying) return
      updateContent(tab.id, model.getValue())
      if (tab.path) scheduleLspChange(tab.path, model.getValue())
    }),
  )
  models.set(tab.id, model)
  return model
}

function applyStrategy(strategy: EditorFileStrategy): void {
  activeStrategy.value = strategy
  editor?.updateOptions({
    readOnly: strategy.readOnly,
    occurrencesHighlight: strategy.occurrences ? 'singleFile' : 'off',
    folding: strategy.folding,
    minimap: {
      enabled: strategy.minimap,
      renderCharacters: false,
      maxColumn: 60,
      showSlider: 'mouseover',
    },
  })
}

/** 搜索/问题面板的"打开到某行"：只对当前激活标签生效，行号做模型边界收敛。 */
function applyReveal(): void {
  if (!editor || !currentTabId) return
  const target = consumeReveal(currentTabId)
  if (!target) return
  const model = editor.getModel()
  if (!model) return
  const line = Math.min(Math.max(1, target.line), model.getLineCount())
  const column = Math.min(Math.max(1, target.column), model.getLineMaxColumn(line))
  editor.setPosition({ lineNumber: line, column })
  editor.revealLineInCenter(line)
  editor.focus()
}

/** 字体/换行是编辑器级选项；Tab 宽度是 model 级选项，需覆盖已有与后续 model。 */
function applyEditorPrefs(prefs: UiPrefs): void {
  editor?.updateOptions({
    fontSize: prefs.fontSize,
    lineHeight: Math.round(prefs.fontSize * 1.6),
    wordWrap: prefs.wordWrap,
  })
  for (const model of models.values()) model.updateOptions({ tabSize: prefs.tabSize })
}

/** 把 LSP 诊断同步为 Monaco markers；非活动 model 也一起刷新。 */
function applyDiagnostics(): void {
  if (!monacoRef || !editor) return
  for (const [id, model] of models) {
    const tab = tabsStore.get().tabs.find(item => item.id === id)
    if (!tab?.path) continue
    const markers = diagnosticsFor(tab.path).map(diagnostic => ({
      severity:
        diagnostic.severity === 1
          ? monacoRef!.MarkerSeverity.Error
          : diagnostic.severity === 2
            ? monacoRef!.MarkerSeverity.Warning
            : diagnostic.severity === 3
              ? monacoRef!.MarkerSeverity.Info
              : monacoRef!.MarkerSeverity.Hint,
      message: diagnostic.message,
      startLineNumber: Math.max(1, diagnostic.line),
      startColumn: Math.max(1, diagnostic.column),
      endLineNumber: Math.max(1, diagnostic.endLine),
      endColumn: Math.max(1, diagnostic.endColumn),
    }))
    monacoRef.editor.setModelMarkers(model, 'lsp', markers)
  }
}

/** LSP 全量文本同步：300ms 去抖，避免每键一次 IPC。 */
function scheduleLspChange(path: string, text: string): void {
  if (lspTimer) clearTimeout(lspTimer)
  lspTimer = setTimeout(() => {
    lspTimer = null
    void lspDidChange(path, text)
  }, 300)
}

function openLspDocument(path: string, content: string): void {
  if (lspOpened.has(path)) return
  lspOpened.set(path, languageForPath(path))
  void lspDidOpen(path, languageForPath(path), content)
}

function closeLspDocument(path: string): void {
  lspOpened.delete(path)
  void lspDidClose(path)
}

/** LSP 宿主重启后重新打开所有已打开文件，避免旧 didOpen 状态丢失。 */
function reopenLspDocuments(): void {
  lspOpened.clear()
  for (const tab of tabsStore.get().tabs) {
    if (tab.path) {
      lspOpened.set(tab.path, languageForPath(tab.path))
      void lspDidOpen(tab.path, languageForPath(tab.path), tab.content)
    }
  }
}

function disposeMissingModels(state: TabsState): void {
  const alive = new Set(state.tabs.map(t => t.id))
  for (const id of [...models.keys()]) {
    if (alive.has(id) || id === currentTabId) continue
    const path = lspOpened.get(id)
    if (path) closeLspDocument(path)
    modelDisposables.get(id)?.dispose()
    models.get(id)?.dispose()
    models.delete(id)
    modelDisposables.delete(id)
    viewStates.delete(id)
  }
}

function syncFromStore(state: TabsState): void {
  if (!editor) return
  const tab = activeTab(state)

  if (!tab) {
    if (currentTabId) viewStates.set(currentTabId, editor.saveViewState())
    currentTabId = null
    editor.setModel(null)
    activeStrategy.value = null
    disposeMissingModels(state)
    return
  }

  if (tab.id !== currentTabId) {
    if (currentTabId) viewStates.set(currentTabId, editor.saveViewState())
    saveState.value = null
  }

  const model = ensureModel(tab)
  if (editor.getModel() !== model) {
    editor.setModel(model)
    const view = viewStates.get(tab.id)
    if (view) editor.restoreViewState(view)
  }
  if (tab.path) openLspDocument(tab.path, tab.content)
  if (tab.id !== currentTabId) applyStrategy(tab.strategy)
  currentTabId = tab.id
  disposeMissingModels(state)
  applyDiagnostics()
}

async function saveActive(): Promise<void> {
  const tab = currentTab()
  if (!tab) return
  if (!tab.path) {
    saveState.value = { kind: 'sample', text: t('editor.sampleNotSaveable') }
    return
  }
  if (tab.strategy.readOnly) {
    saveState.value = { kind: 'readonly', text: t('editor.readOnlyPreview') }
    return
  }
  if (!tab.dirty) {
    saveState.value = { kind: 'saved', text: t('editor.saved') }
    return
  }
  const snapshot = tab.content
  const res = await writeFile(tab.path, snapshot)
  if (!res.ok) {
    saveState.value = { kind: 'error', text: `${t('editor.saveFailed')} (${res.error.code})` }
    return
  }
  // 保存期间可能继续输入：只有内容仍是落盘快照时才清 dirty。
  // 浏览器 fallback 的 ack 不是真实落盘，不能清 dirty。
  const latest = tabsStore.get().tabs.find(t => t.id === tab.id)
  if (!res.fallback && latest?.content === snapshot) markSaved(tab.id)
  saveState.value = res.fallback
    ? { kind: 'demo', text: t('editor.savedDemo') }
    : { kind: 'saved', text: t('editor.saved') }
}

function onSaveKeydown(e: KeyboardEvent): void {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
    e.preventDefault()
    void saveActive()
  }
}

// worker 用 new URL 模式（Vite 原生支持，不走 ?worker 查询——
// rolldown 无法对 exports-map 子路径加查询解析，只能给真实文件路径）
function createEditorWorker(): Worker {
  return new Worker(
    new URL('../../node_modules/monaco-editor/esm/vs/editor/editor.worker.js', import.meta.url),
    { type: 'module' },
  )
}

function addSelectionToAgent(): void {
  const tab = currentTab()
  const model = editor?.getModel()
  const selection = editor?.getSelection()
  if (!tab || !model || !selection || selection.isEmpty()) return
  let text = model.getValueInRange(selection)
  if (!text.trim()) return
  if (text.length > MAX_CONTEXT_CHARS) {
    text = `${text.slice(0, MAX_CONTEXT_CHARS)}\n${t('menu.addToAgentTruncated')}`
  }
  pushAgentContext({
    // 未落盘的演示/临时缓冲区没有路径，用标签标题作为来源标识。
    path: tab.path ?? tab.title,
    language: model.getLanguageId(),
    text,
    startLine: selection.startLineNumber,
    endLine: selection.endLineNumber,
  })
  contextMenu.value = null
}

function menuAction(action: string): void {
  contextMenu.value = null
  editor?.trigger('context-menu', action, null)
  editor?.focus()
}

onMounted(async () => {
  ;(self as unknown as { MonacoEnvironment: unknown }).MonacoEnvironment = {
    getWorker: createEditorWorker,
  }
  const monaco = await import('monaco-editor')
  if (disposed || !host.value) return
  monacoRef = monaco
  monaco.editor.defineTheme('lite-code-dark', {
    base: 'vs-dark',
    inherit: true,
    rules: [],
    colors: {
      'editor.background': '#17181c',
      'editorGutter.background': '#17181c',
      'editor.lineHighlightBackground': '#1d1f24',
      'editorLineNumber.foreground': '#5c616b',
      'editorLineNumber.activeForeground': '#d9dade',
    },
  })

  monaco.editor.defineTheme('lite-code-light', {
    base: 'vs',
    inherit: true,
    rules: [],
    colors: {
      'editor.background': '#ffffff',
      'editorGutter.background': '#f7f9fc',
      'editor.lineHighlightBackground': '#f1f5fb',
      'editorLineNumber.foreground': '#99a3b5',
      'editorLineNumber.activeForeground': '#1c2330',
    },
  })

  const initialPrefs = uiPrefs()
  editor = monaco.editor.create(host.value!, {
    model: null,
    theme: initialPrefs.theme === 'light' ? 'lite-code-light' : 'lite-code-dark',
    fontSize: initialPrefs.fontSize,
    lineHeight: Math.round(initialPrefs.fontSize * 1.6),
    wordWrap: initialPrefs.wordWrap,
    tabSize: initialPrefs.tabSize,
    minimap: { enabled: false },
    automaticLayout: true,
    renderWhitespace: 'selection',
    scrollBeyondLastLine: false,
  })

  // 窗口级监听：焦点在文件树/标签栏时 Ctrl+S 也生效。
  window.addEventListener('keydown', onSaveKeydown)
  unsubTabs = tabsStore.subscribe(syncFromStore)
  unsubReveal = subscribeReveal(() => applyReveal())
  unsubDiagnostics = subscribeDiagnostics(() => applyDiagnostics())
  unsubPrefs = subscribePrefs(prefs => {
    monacoRef?.editor.setTheme(prefs.theme === 'light' ? 'lite-code-light' : 'lite-code-dark')
    applyEditorPrefs(prefs)
  })

  // 自定义中文右键菜单：capture 阶段拦截 Monaco 默认英文菜单。
  const openMenu = (event: MouseEvent): void => {
    event.preventDefault()
    event.stopPropagation()
    const selection = editor?.getSelection()
    hasSelection.value = Boolean(selection && !selection.isEmpty())
    contextMenu.value = { x: event.clientX, y: event.clientY }
  }
  window.addEventListener('click', onWindowClick)
  window.addEventListener('keydown', onWindowKeydown)
  window.addEventListener('lsp:started', reopenLspDocuments)
  host.value.addEventListener('contextmenu', openMenu, true)
  removeContext = () => host.value?.removeEventListener('contextmenu', openMenu, true)

  syncFromStore(tabsStore.get())
  applyReveal()
  applyDiagnostics()
})

onBeforeUnmount(() => {
  disposed = true
  window.removeEventListener('keydown', onSaveKeydown)
  window.removeEventListener('click', onWindowClick)
  window.removeEventListener('keydown', onWindowKeydown)
  window.removeEventListener('lsp:started', reopenLspDocuments)
  unsubTabs?.()
  unsubPrefs?.()
  unsubReveal?.()
  unsubDiagnostics?.()
  if (lspTimer) clearTimeout(lspTimer)
  for (const path of lspOpened.keys()) void lspDidClose(path)
  lspOpened.clear()
  removeContext?.()
  editor?.dispose()
  editor = null
  for (const d of modelDisposables.values()) d.dispose()
  for (const m of models.values()) m.dispose()
  modelDisposables.clear()
  models.clear()
  viewStates.clear()
})
</script>

<template>
  <div class="editor-wrap">
    <div
      ref="host"
      class="editor-host"
    />
    <div
      v-if="activeStrategy?.readOnly"
      class="readonly-banner"
      role="note"
    >
      {{ t('editor.readOnlyPreview') }}
    </div>
    <div
      v-if="saveState"
      class="save-note"
      :class="`save-${saveState.kind}`"
      role="status"
    >
      {{ saveState.text }}
    </div>
    <Teleport to="body">
      <div
        v-if="contextMenu"
        class="editor-menu"
        :style="{ left: `${contextMenu.x}px`, top: `${contextMenu.y}px` }"
        @click.stop
      >
        <button
          type="button"
          @click="menuAction('undo')"
        >
          {{ t('menu.undo') }}
        </button>
        <button
          type="button"
          @click="menuAction('redo')"
        >
          {{ t('menu.redo') }}
        </button>
        <div class="menu-sep" />
        <button
          type="button"
          @click="menuAction('editor.action.clipboardCutAction')"
        >
          {{ t('menu.cut') }}
        </button>
        <button
          type="button"
          @click="menuAction('editor.action.clipboardCopyAction')"
        >
          {{ t('menu.copy') }}
        </button>
        <button
          type="button"
          @click="menuAction('editor.action.clipboardPasteAction')"
        >
          {{ t('menu.paste') }}
        </button>
        <div class="menu-sep" />
        <button
          type="button"
          @click="menuAction('editor.action.selectAll')"
        >
          {{ t('menu.selectAll') }}
        </button>
        <button
          v-if="hasSelection"
          type="button"
          @click="addSelectionToAgent"
        >
          {{ t('menu.addToAgent') }}
        </button>
        <div
          v-if="hasSelection"
          class="menu-sep"
        />
        <button
          type="button"
          @click="menuAction('actions.find')"
        >
          {{ t('menu.find') }}
        </button>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.editor-wrap {
  position: relative;
  flex: 1;
  width: 100%;
  min-width: 0;
  height: 100%;
  min-height: 0;
  display: flex;
}

.editor-host {
  flex: 1;
  min-height: 0;
}

.readonly-banner {
  position: absolute;
  top: 8px;
  right: 12px;
  padding: 3px 8px;
  border: 1px solid color-mix(in srgb, #d19a66 45%, var(--border));
  border-radius: var(--r-sm);
  background: color-mix(in srgb, #d19a66 15%, var(--panel));
  color: #d19a66;
  font-size: 11px;
  pointer-events: none;
}

.editor-menu {
  position: fixed;
  z-index: 100;
  min-width: 170px;
  padding: 5px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--r);
  box-shadow: var(--shadow);
}

.editor-menu button {
  display: block;
  width: 100%;
  padding: 6px 10px;
  text-align: left;
  border-radius: var(--r-xs);
  color: var(--fg-2);
  font-size: 12px;
}

.editor-menu button:hover {
  background: var(--accent-soft);
  color: var(--fg);
}

.menu-sep {
  height: 1px;
  margin: 4px 6px;
  background: var(--border);
}

.save-note {
  position: absolute;
  right: 12px;
  bottom: 8px;
  max-width: 60%;
  padding: 3px 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-3);
  color: var(--fg-dim);
  font-size: 11px;
  pointer-events: none;
}

.save-note.save-saved {
  color: var(--accent);
}

.save-note.save-error {
  color: #e06c75;
}

.save-note.save-demo,
.save-note.save-readonly {
  color: #d19a66;
}
</style>
