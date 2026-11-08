<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import AppIcon from './AppIcon.vue'
import type { AgentSessionController } from '../services/agentSession'
import { flushHistory, loadStoredSession } from '../services/agentHistory'
import { historyDelete, historyList, type HistorySummary } from '../services/bridge'
import { samePath, type WorkspaceEntry } from '../services/workspaceStore'

const props = defineProps<{
  controller: AgentSessionController
  closable?: boolean
  workspace?: string
  workspaces?: WorkspaceEntry[]
  activeWorkspace?: string
  showWorkspaces?: boolean
}>()
const emit = defineEmits<{
  select: []
  'new-session': []
  close: []
  'select-workspace': [root: string]
  'remove-workspace': [root: string]
  'add-workspace': []
}>()
const { t } = useI18n()

const state = ref(props.controller.getState())
const unsubscribe = props.controller.subscribe(next => {
  state.value = next
})

const allItems = ref<HistorySummary[]>([])
const loading = ref(false)
const error = ref('')
const confirmId = ref<string | null>(null)
const confirmWorkspace = ref<string | null>(null)
let confirmTimer: ReturnType<typeof setTimeout> | null = null
let workspaceTimer: ReturnType<typeof setTimeout> | null = null

const items = computed(() =>
  allItems.value.filter(item => {
    if (!props.workspace) return !item.workspace
    return !item.workspace || samePath(item.workspace, props.workspace)
  }),
)

const activeId = computed(() => state.value.agent?.sessionId ?? '')

async function refresh(): Promise<void> {
  loading.value = true
  error.value = ''
  await flushHistory()
  const res = await historyList()
  loading.value = false
  if (res.ok) allItems.value = res.data
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

function clearWorkspaceConfirm(): void {
  confirmWorkspace.value = null
  if (workspaceTimer !== null) {
    clearTimeout(workspaceTimer)
    workspaceTimer = null
  }
}

function askRemoveWorkspace(root: string): void {
  clearWorkspaceConfirm()
  confirmWorkspace.value = root
  workspaceTimer = setTimeout(clearWorkspaceConfirm, 3000)
}

function removeWorkspace(root: string): void {
  if (confirmWorkspace.value !== root) {
    askRemoveWorkspace(root)
    return
  }
  clearWorkspaceConfirm()
  emit('remove-workspace', root)
}

function selectWorkspace(root: string): void {
  emit('select-workspace', root)
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
  allItems.value = allItems.value.filter(entry => entry.id !== item.id)
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

watch(
  () => props.workspace,
  () => {
    void refresh()
  },
)

function onHistoryChanged(): void {
  void refresh()
}

onMounted(() => {
  window.addEventListener('history:changed', onHistoryChanged)
  void refresh()
})

onBeforeUnmount(() => {
  window.removeEventListener('history:changed', onHistoryChanged)
  unsubscribe()
  clearConfirm()
  clearWorkspaceConfirm()
})
</script>

<template>
  <div class="sessions">
    <div
      v-if="props.showWorkspaces"
      class="workspace-section"
    >
      <div class="sessions-head">
        <span class="sessions-title">{{ t('agent.workspaces') }}</span>
        <button
          type="button"
          class="icon-btn"
          :title="t('agent.addWorkspace')"
          :aria-label="t('agent.addWorkspace')"
          @click="emit('add-workspace')"
        >
          <AppIcon
            name="plus"
            :size="14"
          />
        </button>
      </div>
      <div class="workspace-list">
        <p
          v-if="!props.workspaces || props.workspaces.length === 0"
          class="sessions-note"
        >
          {{ t('agent.noWorkspaces') }}
        </p>
        <div
          v-for="item in props.workspaces"
          :key="item.root"
          class="workspace-row"
          :class="{
            active: samePath(item.root, props.activeWorkspace ?? ''),
            danger: item.root === confirmWorkspace,
          }"
          role="button"
          tabindex="0"
          @click="selectWorkspace(item.root)"
          @keydown.enter.prevent="selectWorkspace(item.root)"
        >
          <AppIcon
            name="folder"
            :size="13"
            class="workspace-icon"
          />
          <div class="workspace-main">
            <span class="workspace-name">{{ item.name }}</span>
            <span
              class="workspace-path"
              :title="item.root"
            >{{ item.root }}</span>
          </div>
          <button
            type="button"
            class="session-delete"
            :title="item.root === confirmWorkspace ? t('agent.removeWorkspaceConfirm') : t('agent.removeWorkspace')"
            :aria-label="t('agent.removeWorkspace')"
            @click.stop="removeWorkspace(item.root)"
          >
            <AppIcon
              name="x"
              :size="12"
            />
          </button>
        </div>
        <button
          v-if="!props.workspaces || props.workspaces.length === 0"
          type="button"
          class="workspace-add"
          @click="emit('add-workspace')"
        >
          <AppIcon
            name="folder"
            :size="13"
          />
          {{ t('agent.addWorkspace') }}
        </button>
      </div>
    </div>

    <div class="sessions-head">
      <span class="sessions-title">{{ props.showWorkspaces ? t('agent.conversations') : t('agent.historyTitle') }}</span>
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
      <button
        v-if="props.closable"
        type="button"
        class="icon-btn"
        :title="t('agent.historyClose')"
        :aria-label="t('agent.historyClose')"
        @click="emit('close')"
      >
        <AppIcon
          name="x"
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

.workspace-section {
  flex: none;
  display: flex;
  flex-direction: column;
  max-height: 42%;
  border-bottom: 1px solid var(--border);
}

.workspace-section .sessions-head {
  height: 32px;
}

.workspace-list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 4px 6px 6px;
}

.workspace-row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 6px 6px 8px;
  border: 1px solid transparent;
  border-radius: var(--r-sm);
  cursor: pointer;
  transition: background var(--dur) var(--ease), border-color var(--dur) var(--ease);
}

.workspace-row:hover {
  background: var(--panel-2);
}

.workspace-row.active {
  border-color: var(--accent-line);
  background: var(--accent-soft);
}

.workspace-row.danger {
  border-color: var(--red);
}

.workspace-icon {
  flex: none;
  color: var(--fg-faint);
}

.workspace-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.workspace-name {
  overflow: hidden;
  color: var(--fg-2);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.workspace-row.active .workspace-name {
  color: var(--fg);
}

.workspace-path {
  overflow: hidden;
  color: var(--fg-faint);
  font: 9.5px var(--font-mono);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.workspace-row:hover .session-delete,
.workspace-row.danger .session-delete {
  opacity: 1;
}

.workspace-add {
  flex: none;
  height: 28px;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  margin-top: 2px;
  border: 1px dashed var(--border-strong);
  border-radius: var(--r-sm);
  color: var(--fg-dim);
  font-size: 11px;
}

.workspace-add:hover {
  color: var(--accent);
  border-color: var(--accent);
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
