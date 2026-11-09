<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import AppIcon from './AppIcon.vue'
import { normalizePreviewUrl } from '../services/previewUrl'

const emit = defineEmits<{ close: [] }>()
const { t } = useI18n()

const STORAGE_KEY = 'lc.agentPreviewUrl'
const draft = ref('')
const url = ref('')
const error = ref('')
const revision = ref(0)

function errorText(code: string): string {
  if (code === 'empty') return t('agent.previewUrlRequired')
  if (code === 'scheme') return t('agent.previewScheme')
  return t('agent.previewUrlInvalid')
}

function apply(value: string): void {
  const result = normalizePreviewUrl(value)
  error.value = result.error ? errorText(result.error) : ''
  if (!result.url) return
  url.value = result.url
  draft.value = result.url
  revision.value += 1
  try {
    localStorage.setItem(STORAGE_KEY, result.url)
  } catch {
    // 隐私模式只影响持久化。
  }
}

function load(): void {
  apply(draft.value)
}

function clear(): void {
  url.value = ''
  draft.value = ''
  error.value = ''
  try {
    localStorage.removeItem(STORAGE_KEY)
  } catch {
    // ignore
  }
}

function refresh(): void {
  if (url.value) revision.value += 1
}

onMounted(() => {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    if (saved) apply(saved)
  } catch {
    // ignore
  }
})
</script>

<template>
  <section class="agent-preview">
    <header class="preview-head">
      <span class="preview-title">{{ t('agent.previewTitle') }}</span>
      <button
        type="button"
        class="head-btn"
        :disabled="!url"
        :title="t('agent.previewRefresh')"
        @click="refresh"
      >
        {{ t('agent.previewRefresh') }}
      </button>
      <button
        type="button"
        class="head-btn close"
        :title="t('agent.previewClose')"
        @click="emit('close')"
      >
        ×
      </button>
    </header>

    <div class="preview-bar">
      <input
        v-model="draft"
        class="preview-url"
        type="text"
        spellcheck="false"
        :placeholder="t('agent.previewUrlPlaceholder')"
        @keydown.enter.prevent="load"
      >
      <button
        type="button"
        class="preview-btn"
        :disabled="draft.trim().length === 0"
        @click="load"
      >
        {{ t('agent.previewOpen') }}
      </button>
      <button
        v-if="url"
        type="button"
        class="preview-btn"
        @click="clear"
      >
        {{ t('agent.previewClear') }}
      </button>
    </div>
    <p
      v-if="error"
      class="preview-error"
    >
      {{ error }}
    </p>

    <iframe
      v-if="url"
      :key="revision"
      class="preview-frame"
      :src="url"
      :title="t('agent.previewTitle')"
      sandbox="allow-scripts allow-same-origin allow-forms allow-popups"
      referrerpolicy="no-referrer"
    />
    <div
      v-else
      class="preview-empty"
    >
      <AppIcon
        name="preview"
        :size="22"
      />
      <span>{{ t('agent.previewEmpty') }}</span>
    </div>
  </section>
</template>

<style scoped>
.agent-preview {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--panel);
}

.preview-head {
  flex: none;
  height: 36px;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 8px 0 12px;
  border-bottom: 1px solid var(--border);
}

.preview-title {
  flex: 1;
  color: var(--fg-dim);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.07em;
  text-transform: uppercase;
}

.head-btn {
  flex: none;
  height: 22px;
  padding: 0 6px;
  border-radius: var(--r-xs, 4px);
  color: var(--fg-faint);
  font-size: 10.5px;
}

.head-btn:hover:not(:disabled) {
  background: var(--panel-3);
  color: var(--fg);
}

.head-btn:disabled {
  opacity: 0.4;
  cursor: default;
}

.head-btn.close {
  width: 22px;
  padding: 0;
  font-size: 15px;
}

.preview-bar {
  flex: none;
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 8px;
  border-bottom: 1px solid var(--border);
}

.preview-url {
  flex: 1;
  min-width: 0;
  height: 26px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-xs, 4px);
  outline: none;
  background: var(--bg);
  color: var(--fg);
  font: 11px var(--font-mono);
}

.preview-url:focus {
  border-color: var(--accent);
}

.preview-url::placeholder {
  color: var(--fg-faint);
}

.preview-btn {
  flex: none;
  height: 26px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-xs, 4px);
  color: var(--fg-dim);
  font-size: 11px;
}

.preview-btn:hover:not(:disabled) {
  color: var(--fg);
  border-color: var(--accent);
}

.preview-btn:disabled {
  opacity: 0.45;
  cursor: default;
}

.preview-error {
  flex: none;
  margin: 0;
  padding: 6px 10px;
  color: var(--red);
  font-size: 10.5px;
}

.preview-frame {
  flex: 1;
  min-height: 0;
  width: 100%;
  border: 0;
  background: #fff;
}

.preview-empty {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 20px;
  color: var(--fg-faint);
  font-size: 11.5px;
  text-align: center;
}
</style>
