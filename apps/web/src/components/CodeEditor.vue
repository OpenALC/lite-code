<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import type * as monacoNs from 'monaco-editor'

import { writeFile } from '../services/bridge'
import { languageForPath } from '../services/editorLanguage'
import { activeTab, markSaved, tabsStore, updateContent } from '../services/tabs'
import type { Tab, TabsState } from '../services/tabs'

const { t } = useI18n()

const host = ref<HTMLElement | null>(null)
let editor: monacoNs.editor.IStandaloneCodeEditor | null = null
let monacoRef: typeof monacoNs | null = null
let unsubTabs: (() => void) | null = null
let currentTabId: string | null = null
/** setValue 触发的 onDidChangeModelContent 不应回写 store */
let applying = false

type SaveState = { kind: 'saved' | 'demo' | 'error' | 'sample'; text: string }
const saveState = ref<SaveState | null>(null)

function currentTab(): Tab | null {
  return activeTab(tabsStore.get())
}

function syncFromStore(state: TabsState): void {
  if (!editor) return
  const tab = activeTab(state)
  if (!tab) {
    currentTabId = null
    applying = true
    editor.setValue('')
    applying = false
    return
  }
  const changedTab = tab.id !== currentTabId
  if (!changedTab && tab.content === editor.getValue()) return
  if (changedTab) saveState.value = null
  const model = editor.getModel()
  if (model) monacoRef?.editor.setModelLanguage(model, languageForPath(tab.path ?? tab.title))
  currentTabId = tab.id
  applying = true
  editor.setValue(tab.content)
  applying = false
}

async function saveActive(): Promise<void> {
  const tab = currentTab()
  if (!tab) return
  if (!tab.path) {
    saveState.value = { kind: 'sample', text: t('editor.sampleNotSaveable') }
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
  const latest = tabsStore.get().tabs.find(t => t.id === tab.id)
  if (latest?.content === snapshot) markSaved(tab.id)
  saveState.value = res.fallback
    ? { kind: 'demo', text: t('editor.savedDemo') }
    : { kind: 'saved', text: t('editor.saved') }
}

// worker 用 new URL 模式（Vite 原生支持，不走 ?worker 查询——
// rolldown 无法对 exports-map 子路径加查询解析，只能给真实文件路径）
function createEditorWorker(): Worker {
  return new Worker(
    new URL('../../node_modules/monaco-editor/esm/vs/editor/editor.worker.js', import.meta.url),
    { type: 'module' },
  )
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

  const initial = currentTab()
  currentTabId = initial?.id ?? null
  editor = monaco.editor.create(host.value!, {
    value: initial?.content ?? '',
    language: languageForPath(initial?.path ?? initial?.title ?? null),
    theme: 'lite-code-dark',
    fontSize: 13,
    lineHeight: 21,
    minimap: { enabled: false },
    automaticLayout: true,
    renderWhitespace: 'selection',
    scrollBeyondLastLine: false,
  })

  editor.onDidChangeModelContent(() => {
    if (applying || !currentTabId) return
    updateContent(currentTabId, editor!.getValue())
  })
  editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => {
    void saveActive()
  })
  unsubTabs = tabsStore.subscribe(syncFromStore)
  syncFromStore(tabsStore.get())
})

onBeforeUnmount(() => {
  unsubTabs?.()
  editor?.dispose()
  editor = null
})
</script>

<template>
  <div class="editor-wrap">
    <div
      ref="host"
      class="editor-host"
    />
    <div
      v-if="saveState"
      class="save-note"
      :class="`save-${saveState.kind}`"
      role="status"
    >
      {{ saveState.text }}
    </div>
  </div>
</template>

<style scoped>
.editor-wrap {
  position: relative;
  height: 100%;
  min-height: 0;
  display: flex;
}

.editor-host {
  flex: 1;
  min-height: 0;
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

.save-note.save-demo {
  color: #d19a66;
}
</style>
