<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { activeTab, tabsStore, type Tab } from '../services/tabs'

// 只读预览：不引入 Monaco / CodeMirror。大文件按字符与行数双阈值截断。
const MAX_PREVIEW_CHARS = 1024 * 1024
const MAX_PREVIEW_LINES = 4000

const { t } = useI18n()
const tab = ref<Tab | null>(activeTab(tabsStore.get()))
const unsubscribe = tabsStore.subscribe(state => {
  tab.value = activeTab(state)
})
onBeforeUnmount(unsubscribe)

const content = computed(() => tab.value?.content ?? '')
const truncated = computed(
  () =>
    content.value.length > MAX_PREVIEW_CHARS ||
    content.value.split('\n').length > MAX_PREVIEW_LINES,
)
const lines = computed(() => {
  if (!tab.value) return []
  const visible = content.value.slice(0, MAX_PREVIEW_CHARS).split('\n')
  return truncated.value ? visible.slice(0, MAX_PREVIEW_LINES) : visible
})
</script>

<template>
  <div class="ro-view">
    <div
      v-if="!tab"
      class="ro-empty"
    >
      {{ t('explorer.noEditors') }}
    </div>
    <template v-else>
      <header class="ro-head">
        <span
          class="ro-title"
          :title="tab.path ?? tab.title"
        >{{ tab.title }}</span>
        <span class="ro-badge">{{ t('db.readOnly') }}</span>
      </header>
      <div
        v-if="truncated"
        class="ro-truncated"
      >
        {{ t('editor.readOnlyPreview') }}
      </div>
      <div class="ro-scroll">
        <pre class="ro-code"><code><span
          v-for="(line, index) in lines"
          :key="index"
          class="ro-line"
        >{{ line }}</span></code></pre>
      </div>
    </template>
  </div>
</template>
