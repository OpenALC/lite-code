<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { detectNode, downloadNodePortable, setNodeManual } from '../services/bridge'
import type { BridgeResult, NodeStatusResult } from '../services/bridge'

const emit = defineEmits<{ close: [] }>()
const { t } = useI18n()

const status = ref<NodeStatusResult['status'] | null>(null)
const fallback = ref(false)
const loading = ref(false)
const downloading = ref(false)
const manualPath = ref('')
const error = ref<string | null>(null)
const note = ref<string | null>(null)

const ready = computed(() =>
  status.value?.kind === 'ready' ? status.value.runtime : null,
)
const attempts = computed(() =>
  status.value?.kind === 'need_manual' ? status.value.attempts : [],
)

function applyResult(res: BridgeResult<NodeStatusResult>): boolean {
  if (!res.ok) {
    error.value = res.error.message
    return false
  }
  status.value = res.data.status
  fallback.value = res.fallback
  error.value = null
  return true
}

async function refresh(): Promise<void> {
  loading.value = true
  note.value = null
  try {
    applyResult(await detectNode())
  } finally {
    loading.value = false
  }
}

async function applyManual(): Promise<void> {
  const path = manualPath.value.trim()
  if (!path) return
  loading.value = true
  note.value = null
  try {
    const res = await setNodeManual(path)
    if (applyResult(res) && res.ok && !res.fallback) note.value = t('runtime.saved')
  } finally {
    loading.value = false
  }
}

async function installPortable(): Promise<void> {
  downloading.value = true
  note.value = null
  try {
    const res = await downloadNodePortable('22')
    if (applyResult(res) && res.ok && !res.fallback) note.value = t('runtime.installed')
  } finally {
    downloading.value = false
  }
}

onMounted(refresh)
</script>

<template>
  <aside
    class="runtime-panel"
    role="dialog"
    :aria-label="t('runtime.title')"
  >
    <header class="head">
      <span class="title">{{ t('runtime.title') }}</span>
      <button
        type="button"
        class="close"
        :title="t('runtime.close')"
        @click="emit('close')"
      >
        ×
      </button>
    </header>

    <div class="body">
      <div
        v-if="fallback"
        class="banner warn"
      >
        {{ t('runtime.fallback') }}
      </div>

      <section class="block">
        <div class="block-head">
          <span>{{ t('runtime.status') }}</span>
          <button
            type="button"
            class="link"
            :disabled="loading"
            @click="refresh"
          >
            {{ loading ? t('runtime.detecting') : t('runtime.refresh') }}
          </button>
        </div>

        <div
          v-if="ready"
          class="status ready"
        >
          <div class="badge">
            {{ t('runtime.ready') }}
          </div>
          <div class="path mono">
            {{ ready.path }}
          </div>
          <div class="meta">
            v{{ ready.version.major }}.{{ ready.version.minor }}.{{ ready.version.patch }}
            · {{ t('runtime.source') }}: {{ ready.source }}
            · {{ t(`runtime.health.${ready.health}`) }}
          </div>
        </div>

        <div
          v-else-if="!loading"
          class="status missing"
        >
          <div class="badge muted">
            {{ t('runtime.needManual') }}
          </div>
          <ul
            v-if="attempts.length > 0"
            class="attempts"
          >
            <li
              v-for="attempt in attempts"
              :key="`${attempt.path}:${attempt.reason}`"
              class="mono"
              :title="attempt.detail ?? ''"
            >
              <span class="reason">{{ attempt.reason }}</span>
              <span class="attempt-path">{{ attempt.path }}</span>
            </li>
          </ul>
        </div>
      </section>

      <section class="block">
        <div class="block-head">
          {{ t('runtime.manualTitle') }}
        </div>
        <input
          v-model="manualPath"
          class="input mono"
          :placeholder="t('runtime.manualPlaceholder')"
          spellcheck="false"
        >
        <button
          type="button"
          class="action"
          :disabled="loading || manualPath.trim().length === 0"
          @click="applyManual"
        >
          {{ t('runtime.manualAction') }}
        </button>
      </section>

      <section class="block">
        <div class="block-head">
          {{ t('runtime.portableTitle') }}
        </div>
        <p class="desc">
          {{ t('runtime.portableDesc') }}
        </p>
        <button
          type="button"
          class="action"
          :disabled="downloading"
          @click="installPortable"
        >
          {{ downloading ? t('runtime.downloading') : t('runtime.portableAction') }}
        </button>
      </section>

      <div
        v-if="error"
        class="banner error"
      >
        {{ error }}
      </div>
      <div
        v-if="note"
        class="banner ok"
      >
        {{ note }}
      </div>
    </div>
  </aside>
</template>

<style scoped>
.runtime-panel {
  position: fixed;
  top: 44px;
  right: 12px;
  bottom: 32px;
  width: 380px;
  z-index: 30;
  display: flex;
  flex-direction: column;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  box-shadow: 0 12px 32px rgb(0 0 0 / 35%);
}

.head {
  display: flex;
  align-items: center;
  height: 32px;
  padding: 0 10px;
  border-bottom: 1px solid var(--border);
}

.title {
  flex: 1;
  font-size: 12px;
  color: var(--fg-2);
  letter-spacing: 0.04em;
}

.close {
  width: 22px;
  height: 22px;
  color: var(--fg-dim);
  font-size: 16px;
  line-height: 1;
}

.close:hover {
  color: var(--fg);
}

.body {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 10px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.block {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.block-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 11px;
  color: var(--fg-dim);
  letter-spacing: 0.04em;
}

.link {
  color: var(--accent);
  font-size: 11px;
}

.link:disabled {
  color: var(--fg-faint);
}

.status {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-3);
}

.badge {
  align-self: flex-start;
  padding: 1px 6px;
  border-radius: var(--r-sm);
  background: color-mix(in srgb, var(--accent) 18%, transparent);
  color: var(--accent);
  font-size: 10px;
}

.badge.muted {
  background: var(--panel-2);
  color: var(--fg-dim);
}

.path {
  font-size: 11px;
  color: var(--fg-2);
  word-break: break-all;
}

.meta {
  font-size: 11px;
  color: var(--fg-faint);
}

.attempts {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 3px;
  font-size: 10px;
  color: var(--fg-faint);
}

.attempts li {
  display: flex;
  gap: 6px;
}

.reason {
  flex: 0 0 auto;
  color: #d19a66;
}

.attempt-path {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.input {
  height: 26px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--bg);
  color: var(--fg);
  font-size: 11px;
}

.input:focus {
  border-color: var(--accent);
  outline: none;
}

.action {
  align-self: flex-start;
  height: 26px;
  padding: 0 10px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-3);
  color: var(--fg-2);
  font-size: 11px;
}

.action:hover:not(:disabled) {
  color: var(--fg);
  border-color: var(--accent);
}

.action:disabled {
  color: var(--fg-faint);
}

.desc {
  margin: 0;
  font-size: 11px;
  color: var(--fg-faint);
  line-height: 1.5;
}

.banner {
  padding: 6px 8px;
  border-radius: var(--r-sm);
  font-size: 11px;
  line-height: 1.5;
}

.banner.warn {
  background: color-mix(in srgb, #d19a66 15%, transparent);
  color: #d19a66;
}

.banner.error {
  background: color-mix(in srgb, #e06c75 15%, transparent);
  color: #e06c75;
}

.banner.ok {
  background: color-mix(in srgb, var(--accent) 15%, transparent);
  color: var(--accent);
}

.mono {
  font-family: var(--font-mono);
}
</style>
