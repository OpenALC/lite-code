<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import AppIcon from './AppIcon.vue'
import type { AgentSessionController } from '../services/agentSession'
import { flushHistory, loadStoredSession } from '../services/agentHistory'
import { historyDelete, historyList, type HistorySummary } from '../services/bridge'

const props = defineProps<{ controller: AgentSessionController }>()
const emit = defineEmits<{ select: []; 'new-session': [] }>()
const { t } = useI18n()

const state = ref(props.controller.getState())
const unsubscribe = props.controller.subscribe(next => {
  state.value = next
})

const items = ref<HistorySummary[]>([])
const loading = ref(false)
const error = ref('')
const confirmId = ref<string | null>(null)
let confirmTimer: ReturnType<typeof setTimeout> | null = null

const activeId = computed(() => state.value.agent?.sessionId ?? '')

async function refresh(): Promise<void> {
  loading.value = true
  error.value = ''
  await flushHistory()
  const res = await historyList()
  loading.value = false
  if (res.ok) items.value = res.data
  else error.value = res.error.message
}

function clearConfirm(): void {
  confirmId.value = null
  if (confirmTimer !== null) {
    clearTimeout(confirmTimer)
    confirmTimer = null
  }
}

function askDelete(id: string): void {
  clearConfirm()
  confirmId.value = id
  confirmTimer = setTimeout(clearConfirm, 3000)
}

async function remove(item: HistorySummary): Promise<void> {
  if (confirmId.value !== item.id) {
    askDelete(item.id)
    return
  }
  clearConfirm()
  const res = await historyDelete(item.id)
  if (!res.ok) {
    error.value = res.error.message
    return
  }
  items.value = items.value.filter(entry => entry.id !== item.id)
}

async function open(item: HistorySummary): Promise<void> {
  if (state.value.streaming) return
  clearConfirm()
  const session = await loadStoredSession(item.id)
  if (!session) {
    error.value = t('agent.historyLoadFailed')
    return
  }
  if (props.controller.loadHistory(session)) emit('select')
}

function newSession(): void {
  if (!props.controller.resetSession()) return
  emit('new-session')
}

function modeLabel(mode: string): string {
  return t(`agent.modes.${mode}`)
}

function shortModel(model: string): string {
  if (!model) return t('agent.modelDefault')
  const slash = model.lastIndexOf('/')
  return slash >= 0 ? model.slice(slash + 1) : model
}

function formatTime(savedAt: number): string {
  if (!savedAt) return ''
  const diff = Date.now() - savedAt
  if (diff < 60_000) return t('agent.historyJustNow')
  if (diff < 3_600_000) {
    return t('agent.historyMinutes', { count: Math.floor(diff / 60_000) })
  }
  if (diff < 86_400_000) {
    return t('agent.historyHours', { count: Math.floor(diff / 3_600_000) })
  }
  return new Date(savedAt).toLocaleDateString()
}

watch(
  () => [state.value.streaming, state.value.agent?.sessionId, state.value.agent?.turn],
  () => {
    if (!state.value.streaming) void refresh()
  },
)

onMounted(() => {
  void refresh()
})

onBeforeUnmount(() => {
  unsubscribe()
  clearConfirm()
})
</script>

<template>
  <div class="sessions">
    <div class="sessions-head">
      <span class="sessions-title">{{ t('agent.historyTitle') }}</span>
      <button
        type="button"
        class="icon-btn"
        :title="t('agent.historyRefresh')"
        :aria-label="t('agent.historyRefresh')"
        @click="refresh"
      >
        <AppIcon
          name="refresh"
          :size="14"
        />
      </button>
    </div>

    <div class="sessions-list">
      <p
        v-if="loading"
        class="sessions-note"
      >
        {{ t('agent.historyLoading') }}
      </p>
      <p
        v-else-if="error"
        class="sessions-note error"
      >
        {{ error }}
      </p>
      <p
        v-else-if="items.length === 0"
        class="sessions-note"
      >
        {{ t('agent.historyEmpty') }}
      </p>
      <div
        v-for="item in items"
        :key="item.id"
        class="session-row"
        :class="{ active: item.id === activeId, danger: item.id === confirmId }"
        role="button"
        tabindex="0"
        @click="open(item)"
        @keydown.enter.prevent="open(item)"
      >
        <div class="session-main">
          <span class="session-title">{{ item.title }}</span>
          <span class="session-meta">
            {{ modeLabel(item.mode) }} · {{ shortModel(item.model) }} · {{ formatTime(item.savedAt) }}
          </span>
        </div>
        <button
          type="button"
          class="session-delete"
          :title="item.id === confirmId ? t('agent.historyDeleteConfirm') : t('agent.historyDelete')"
          :aria-label="t('agent.historyDelete')"
          @click.stop="remove(item)"
        >
          <AppIcon
            name="x"
            :size="12"
          />
        </button>
      </div>
    </div>

    <button
      type="button"
      class="sessions-new"
      @click="newSession"
    >
      <AppIcon
        name="plus"
        :size="14"
      />
      {{ t('agent.newSession') }}
    </button>
  </div>
</template>

<style scoped>
.sessions {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  background: var(--panel);
}

.sessions-head {
  flex: none;
  height: 36px;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 8px 0 12px;
  border-bottom: 1px solid var(--border);
}

.sessions-title {
  flex: 1;
  color: var(--fg-dim);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.07em;
  text-transform: uppercase;
}

.icon-btn {
  flex: none;
  width: 24px;
  height: 24px;
  display: grid;
  place-items: center;
  border-radius: var(--r-sm);
  color: var(--fg-dim);
}

.icon-btn:hover {
  background: var(--panel-3);
  color: var(--fg);
}

.sessions-list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 6px;
}

.sessions-note {
  margin: 0;
  padding: 10px 8px;
  color: var(--fg-faint);
  font-size: 11px;
  line-height: 1.6;
  text-align: center;
}

.sessions-note.error {
  color: var(--red);
}

.session-row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 7px 6px 7px 9px;
  border: 1px solid transparent;
  border-radius: var(--r-sm);
  cursor: pointer;
  transition: background var(--dur) var(--ease), border-color var(--dur) var(--ease);
}

.session-row:hover {
  background: var(--panel-2);
}

.session-row.active {
  border-color: var(--accent-line);
  background: var(--accent-soft);
}

.session-row.danger {
  border-color: var(--red);
}

.session-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.session-title {
  overflow: hidden;
  color: var(--fg-2);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.session-row.active .session-title {
  color: var(--fg);
}

.session-meta {
  overflow: hidden;
  color: var(--fg-faint);
  font: 10px var(--font-mono);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.session-delete {
  flex: none;
  width: 20px;
  height: 20px;
  display: grid;
  place-items: center;
  border-radius: 50%;
  color: var(--fg-faint);
  opacity: 0;
}

.session-row:hover .session-delete,
.session-row.danger .session-delete {
  opacity: 1;
}

.session-delete:hover {
  background: var(--panel-3);
  color: var(--red);
}

.sessions-new {
  flex: none;
  height: 32px;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  margin: 6px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  color: var(--fg-dim);
  font-size: 11.5px;
}

.sessions-new:hover {
  color: var(--fg);
  border-color: var(--accent);
  background: var(--panel-2);
}
</style>
