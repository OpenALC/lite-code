<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import type * as monacoNs from 'monaco-editor'

import { writeFile } from '../services/bridge'
import { languageForPath } from '../services/editorLanguage'
import type { EditorFileStrategy } from '../services/largeFile'
import { activeTab, markSaved, tabsStore, updateContent } from '../services/tabs'
import { subscribePrefs, uiPrefs } from '../services/themeStore'
import type { Tab, TabsState } from '../services/tabs'

const { t } = useI18n()

const host = ref<HTMLElement | null>(null)
let editor: monacoNs.editor.IStandaloneCodeEditor | null = null
let monacoRef: typeof monacoNs | null = null
let unsubTabs: (() => void) | null = null
let unsubTheme: (() => void) | null = null
let removeContext: (() => void) | null = null
let currentTabId: string | null = null
/** setValue 触发的 onDidChangeContent 不应回写 store */
let applying = false

/** 每个标签独立 model：undo/redo、光标与折叠状态随 model 保留。 */
const models = new Map<string, monacoNs.editor.ITextModel>()
const modelDisposables = new Map<string, monacoNs.IDisposable>()
const viewStates = new Map<string, monacoNs.editor.ICodeEditorViewState | null>()

type SaveState = { kind: 'saved' | 'demo' | 'error' | 'sample' | 'readonly'; text: string }
const saveState = ref<SaveState | null>(null)
const activeStrategy = ref<EditorFileStrategy | null>(null)
const contextMenu = ref<{ x: number; y: number } | null>(null)

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
  modelDisposables.set(
    tab.id,
    model.onDidChangeContent(() => {
      if (applying) return
      updateContent(tab.id, model.getValue())
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
    minimap: { enabled: strategy.minimap },
  })
}

function disposeMissingModels(state: TabsState): void {
  const alive = new Set(state.tabs.map(t => t.id))
  for (const id of [...models.keys()]) {
    if (alive.has(id) || id === currentTabId) continue
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
  if (tab.id !== currentTabId) applyStrategy(tab.strategy)
  currentTabId = tab.id
  disposeMissingModels(state)
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

  editor = monaco.editor.create(host.value!, {
    model: null,
    theme: uiPrefs().theme === 'light' ? 'lite-code-light' : 'lite-code-dark',
    fontSize: 13,
    lineHeight: 21,
    minimap: { enabled: false },
    automaticLayout: true,
    renderWhitespace: 'selection',
    scrollBeyondLastLine: false,
  })

  // 窗口级监听：焦点在文件树/标签栏时 Ctrl+S 也生效。
  window.addEventListener('keydown', onSaveKeydown)
  unsubTabs = tabsStore.subscribe(syncFromStore)
  unsubTheme = subscribePrefs(prefs => {
    monacoRef?.editor.setTheme(prefs.theme === 'light' ? 'lite-code-light' : 'lite-code-dark')
  })

  // 自定义中文右键菜单：capture 阶段拦截 Monaco 默认英文菜单。
  const openMenu = (event: MouseEvent): void => {
    event.preventDefault()
    event.stopPropagation()
    contextMenu.value = { x: event.clientX, y: event.clientY }
  }
  window.addEventListener('click', () => { contextMenu.value = null })
  window.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') contextMenu.value = null
  })
  host.value!.addEventListener('contextmenu', openMenu, true)
  removeContext = () => host.value?.removeEventListener('contextmenu', openMenu, true)

  syncFromStore(tabsStore.get())
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onSaveKeydown)
  unsubTabs?.()
  unsubTheme?.()
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
