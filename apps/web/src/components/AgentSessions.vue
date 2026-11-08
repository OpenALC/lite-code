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
  'new-session-in': [root: string]
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
const query = ref('')
const collapsed = ref<Set<string>>(new Set())
const pendingOpenId = ref<string | null>(null)
const confirmId = ref<string | null>(null)
const confirmWorkspace = ref<string | null>(null)
let confirmTimer: ReturnType<typeof setTimeout> | null = null
let workspaceTimer: ReturnType<typeof setTimeout> | null = null

interface WorkspaceGroup {
  workspace: WorkspaceEntry
  items: HistorySummary[]
}

function matchesQuery(item: HistorySummary): boolean {
  const term = query.value.trim().toLocaleLowerCase()
  if (!term) return true
  return (
    item.title.toLocaleLowerCase().includes(term) ||
    item.model.toLocaleLowerCase().includes(term) ||
    item.mode.toLocaleLowerCase().includes(term)
  )
}

const items = computed(() =>
  allItems.value.filter(item => {
    if (!matchesQuery(item)) return false
    if (!props.workspace) return !item.workspace
    return !item.workspace || samePath(item.workspace, props.workspace)
  }),
)

const groups = computed<WorkspaceGroup[]>(() => {
  const list = (props.workspaces ?? []).map(workspace => ({
    workspace,
    items: allItems.value.filter(
      item =>
        matchesQuery(item) &&
        (item.workspace
          ? samePath(item.workspace, workspace.root)
          : samePath(workspace.root, props.activeWorkspace ?? '')),
    ),
  }))
  const term = query.value.trim().toLocaleLowerCase()
  if (!term) return list
  return list.filter(
    group =>
      group.items.length > 0 ||
      group.workspace.name.toLocaleLowerCase().includes(term),
  )
})

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

function isCollapsed(root: string): boolean {
  return collapsed.value.has(root)
}

function toggleGroup(root: string): void {
  const next = new Set(collapsed.value)
  if (next.has(root)) next.delete(root)
  else next.add(root)
  collapsed.value = next
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

async function showItem(item: HistorySummary): Promise<void> {
  if (state.value.streaming) return
  const session = await loadStoredSession(item.id)
  if (!session) {
    error.value = t('agent.historyLoadFailed')
    return
  }
  if (props.controller.loadHistory(session)) emit('select')
}

async function open(item: HistorySummary): Promise<void> {
  clearConfirm()
  if (
    item.workspace &&
    props.activeWorkspace &&
    !samePath(item.workspace, props.activeWorkspace)
  ) {
    pendingOpenId.value = item.id
    emit('select-workspace', item.workspace)
    return
  }
  await showItem(item)
}

function newSession(): void {
  if (!props.controller.resetSession()) return
  emit('new-session')
}

function newSessionIn(root: string): void {
  if (samePath(root, props.activeWorkspace ?? '')) {
    newSession()
    return
  }
  emit('new-session-in', root)
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

watch(
  () => props.activeWorkspace,
  () => {
    const queued = pendingOpenId.value
    pendingOpenId.value = null
    if (queued) {
      const item = allItems.value.find(entry => entry.id === queued)
      if (item) void showItem(item)
    }
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
    <template v-if="props.showWorkspaces">
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
            name="folder"
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

      <label class="thread-search">
        <AppIcon
          name="search"
          :size="13"
        />
        <input
          v-model="query"
          type="text"
          :placeholder="t('agent.searchChats')"
          spellcheck="false"
        >
      </label>

      <div class="project-list">
        <p
          v-if="loading && groups.length === 0"
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
          v-else-if="groups.length === 0"
          class="sessions-note"
        >
          {{ t('agent.noWorkspaces') }}
        </p>

        <section
          v-for="group in groups"
          :key="group.workspace.root"
          class="project-group"
        >
          <div
            class="project-row"
            :class="{
              active: samePath(group.workspace.root, props.activeWorkspace ?? ''),
              danger: group.workspace.root === confirmWorkspace,
            }"
          >
            <button
              type="button"
              class="project-caret"
              :class="{ open: !isCollapsed(group.workspace.root) }"
              :title="isCollapsed(group.workspace.root) ? t('agent.expandProject') : t('agent.collapseProject')"
              :aria-expanded="!isCollapsed(group.workspace.root)"
              @click="toggleGroup(group.workspace.root)"
            >
              <AppIcon
                name="chevron"
                :size="12"
              />
            </button>
            <button
              type="button"
              class="project-select"
              :title="group.workspace.root"
              @click="selectWorkspace(group.workspace.root)"
            >
              <AppIcon
                name="folder"
                :size="14"
              />
              <span class="project-name">{{ group.workspace.name }}</span>
              <span class="project-count">{{ group.items.length }}</span>
            </button>
            <button
              type="button"
              class="project-action"
              :title="t('agent.newChatInWorkspace')"
              :aria-label="t('agent.newChatInWorkspace')"
              @click.stop="newSessionIn(group.workspace.root)"
            >
              <AppIcon
                name="plus"
                :size="13"
              />
            </button>
            <button
              type="button"
              class="project-action"
              :title="group.workspace.root === confirmWorkspace ? t('agent.removeWorkspaceConfirm') : t('agent.removeWorkspace')"
              :aria-label="t('agent.removeWorkspace')"
              @click.stop="removeWorkspace(group.workspace.root)"
            >
              <AppIcon
                name="x"
                :size="12"
              />
            </button>
          </div>

          <div
            v-if="!isCollapsed(group.workspace.root)"
            class="project-threads"
          >
            <div
              v-for="item in group.items"
              :key="item.id"
              class="thread-row"
              :class="{ active: item.id === activeId, danger: item.id === confirmId }"
              role="button"
              tabindex="0"
              @click="open(item)"
              @keydown.enter.prevent="open(item)"
            >
              <span class="thread-title">{{ item.title }}</span>
              <span class="thread-time">{{ formatTime(item.savedAt) }}</span>
              <button
                type="button"
                class="thread-delete"
                :title="item.id === confirmId ? t('agent.historyDeleteConfirm') : t('agent.historyDelete')"
                :aria-label="t('agent.historyDelete')"
                @click.stop="remove(item)"
              >
                <AppIcon
                  name="x"
                  :size="11"
                />
              </button>
            </div>
            <p
              v-if="group.items.length === 0"
              class="thread-empty"
            >
              {{ t('agent.noChatsInWorkspace') }}
            </p>
          </div>
        </section>

        <button
          v-if="groups.length === 0"
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
    </template>

    <template v-else>
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
    </template>
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
  height: 34px;
  display: flex;
  align-items: center;
  gap: 4px;
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

.thread-search {
  flex: none;
  display: flex;
  align-items: center;
  gap: 7px;
  height: 30px;
  margin: 6px 8px;
  padding: 0 9px;
  border: 1px solid transparent;
  border-radius: var(--r-sm);
  background: var(--panel-2);
  color: var(--fg-faint);
}

.thread-search:focus-within {
  border-color: var(--border-strong);
  background: var(--panel-3);
  color: var(--fg-dim);
}

.thread-search input {
  flex: 1;
  min-width: 0;
  border: 0;
  outline: 0;
  background: transparent;
  color: var(--fg);
  font-size: 11.5px;
}

.thread-search input::placeholder {
  color: var(--fg-faint);
}

.project-list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 0 6px 6px;
}

.project-group {
  margin-bottom: 4px;
}

.project-row {
  display: flex;
  align-items: center;
  gap: 2px;
  min-height: 30px;
  padding: 0 2px;
  border-radius: var(--r-sm);
}

.project-row:hover {
  background: var(--panel-2);
}

.project-row.active {
  background: color-mix(in srgb, var(--accent) 8%, transparent);
}

.project-row.danger {
  outline: 1px solid var(--red);
}

.project-caret {
  flex: none;
  width: 20px;
  height: 24px;
  display: grid;
  place-items: center;
  color: var(--fg-faint);
}

.project-caret svg {
  transition: transform 120ms var(--ease);
}

.project-caret.open svg {
  transform: rotate(90deg);
}

.project-select {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  color: var(--fg-2);
  font-size: 12px;
  text-align: left;
}

.project-row.active .project-select {
  color: var(--fg);
}

.project-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.project-count {
  flex: none;
  color: var(--fg-faint);
  font: 9.5px var(--font-mono);
}

.project-action {
  flex: none;
  width: 22px;
  height: 22px;
  display: grid;
  place-items: center;
  border-radius: var(--r-sm);
  color: var(--fg-faint);
  opacity: 0;
}

.project-row:hover .project-action,
.project-row.active .project-action,
.project-row.danger .project-action {
  opacity: 1;
}

.project-action:hover {
  background: var(--panel-3);
  color: var(--fg);
}

.project-threads {
  padding-left: 18px;
}

.thread-row {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 29px;
  padding: 0 4px 0 9px;
  border-radius: var(--r-sm);
  color: var(--fg-dim);
  cursor: pointer;
}

.thread-row:hover {
  background: var(--panel-2);
  color: var(--fg-2);
}

.thread-row.active {
  background: var(--panel-3);
  color: var(--fg);
}

.thread-row.danger {
  outline: 1px solid var(--red);
}

.thread-title {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  font-size: 11.5px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.thread-time {
  flex: none;
  color: var(--fg-faint);
  font: 9px var(--font-mono);
}

.thread-delete {
  flex: none;
  width: 18px;
  height: 18px;
  display: grid;
  place-items: center;
  border-radius: 50%;
  color: var(--fg-faint);
  opacity: 0;
}

.thread-row:hover .thread-delete,
.thread-row.danger .thread-delete {
  opacity: 1;
}

.thread-delete:hover {
  background: var(--panel-4);
  color: var(--red);
}

.thread-empty {
  margin: 0;
  padding: 4px 9px 6px;
  color: var(--fg-faint);
  font-size: 10.5px;
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

.workspace-add {
  flex: none;
  height: 30px;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  margin-top: 6px;
  border: 1px dashed var(--border-strong);
  border-radius: var(--r-sm);
  color: var(--fg-dim);
  font-size: 11px;
}

.workspace-add:hover {
  color: var(--accent);
  border-color: var(--accent);
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
