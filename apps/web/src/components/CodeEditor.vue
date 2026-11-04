<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import type * as monacoNs from 'monaco-editor'

import { INITIAL_TABS_STATE, setDirty } from '../services/tabs'

// 样例缓冲区对应的标签 id（tabs.ts 的初始状态里同源）
const SAMPLE_TAB_ID = INITIAL_TABS_STATE.tabs[0].id

const host = ref<HTMLElement | null>(null)
let editor: monacoNs.editor.IStandaloneCodeEditor | null = null

// P0 起壳验证：能打开并编辑一个文件。FS 集成在 P1（Editor UI lane）。
const SAMPLE = `// lite-code · P0
// 这个缓冲区证明 Monaco 起壳成功：能打开、能编辑。
// 文件系统集成（打开真实文件 / 保存 / 冲突检测）在 P1 落地。
fn main() {
    println!("Hello, lite-code");
}
`

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
  editor = monaco.editor.create(host.value!, {
    value: SAMPLE,
    language: 'rust',
    theme: 'lite-code-dark',
    fontSize: 13,
    lineHeight: 21,
    minimap: { enabled: false },
    automaticLayout: true,
    renderWhitespace: 'selection',
    scrollBeyondLastLine: false,
  })
  // 主审补线：编辑内容变化 → 标签页 dirty（services/tabs.ts），让脏标记与
  // 关闭保护在真实界面可观测（此前 setDirty 只有单测没有视图接线）
  editor.onDidChangeModelContent(() => {
    setDirty(SAMPLE_TAB_ID, true)
  })
})

onBeforeUnmount(() => {
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
  </div>
</template>

<style scoped>
.editor-wrap {
  height: 100%;
  min-height: 0;
  display: flex;
}

.editor-host {
  flex: 1;
  min-height: 0;
}
</style>
