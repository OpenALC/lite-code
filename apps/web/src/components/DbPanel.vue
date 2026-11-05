<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  activeConnection,
  canConnect,
  connectionLabel,
  connectionStateLabel,
  dbKindLabel,
  nextConnectionId,
  sqlWithTableQuery,
  type DbController,
  type Translate,
} from '../services/dbStore'
import {
  DB_KINDS,
  type DbConnectionConfig,
  type DbConnectionInfo,
  type DbKind,
} from '../services/dbTypes'
import DbSchemaTree from './DbSchemaTree.vue'
import DbWorkbench from './DbWorkbench.vue'

const props = defineProps<{ controller: DbController }>()
const emit = defineEmits<{ close: [] }>()
const { t } = useI18n()

const state = ref(props.controller.getState())
const unsubscribe = props.controller.subscribe(next => {
  state.value = next
})
onBeforeUnmount(unsubscribe)

const translate: Translate = (key, params) => t(key, params ?? {})
const active = computed(() => activeConnection(state.value))
const connections = computed(() => state.value.connections)

interface ConnectionDraft {
  name: string
  kind: DbKind
  database: string
  readOnly: boolean
}

const formOpen = ref(false)
const editingId = ref<string | null>(null)
const saving = ref(false)
const formError = ref<string | null>(null)
const pendingDeleteId = ref<string | null>(null)

const draft = reactive<ConnectionDraft>({
  name: '',
  kind: 'sqlite',
  database: '',
  readOnly: false,
})

const schemaEmptyText = computed(() => {
  if (!active.value) return t('db.noConnection')
  if (active.value.state !== 'connected') return t('db.schemaEmpty')
  return state.value.loading ? t('db.schemaLoading') : t('db.schemaEmpty')
})

function openAdd(): void {
  editingId.value = null
  draft.name = ''
  draft.kind = 'sqlite'
  draft.database = ''
  draft.readOnly = false
  formError.value = null
  formOpen.value = true
}

function openEdit(connection: DbConnectionInfo): void {
  editingId.value = connection.connection_id
  draft.name = connection.name
  draft.kind = connection.kind
  draft.database = connection.database ?? ''
  draft.readOnly = connection.read_only
  formError.value = null
  formOpen.value = true
}

function closeForm(): void {
  formOpen.value = false
  formError.value = null
}

async function saveForm(): Promise<void> {
  if (saving.value) return
  formError.value = null
  if (draft.name.trim().length === 0) {
    formError.value = t('db.nameRequired')
    return
  }
  if (draft.kind !== 'sqlite') {
    formError.value = t('db.sqliteOnly')
    return
  }
  if (draft.database.trim().length === 0) {
    formError.value = t('db.pathRequired')
    return
  }
  saving.value = true
  const config: DbConnectionConfig = {
    connection_id: editingId.value ?? nextConnectionId(state.value.connections),
    name: draft.name.trim(),
    kind: 'sqlite',
    host: '',
    port: 0,
    database: draft.database.trim(),
    user: '',
    secret_ref: '',
    read_only: draft.readOnly,
  }
  const saved = await props.controller.saveConnection(config)
  saving.value = false
  if (saved) {
    formOpen.value = false
    pendingDeleteId.value = null
  }
}

async function selectConnection(connection: DbConnectionInfo): Promise<void> {
  pendingDeleteId.value = null
  await props.controller.select(connection.connection_id)
}

async function connect(connection: DbConnectionInfo): Promise<void> {
  if (!canConnect(connection) || state.value.loading) return
  await props.controller.connect(connection.connection_id)
}

async function disconnect(connection: DbConnectionInfo): Promise<void> {
  if (state.value.loading) return
  await props.controller.disconnect(connection.connection_id)
}

async function remove(connection: DbConnectionInfo): Promise<void> {
  const removed = await props.controller.deleteConnection(connection.connection_id)
  if (removed) pendingDeleteId.value = null
}

function reloadSchema(): void {
  const connection = active.value
  if (!connection || state.value.loading) return
  void props.controller.loadSchema(connection.connection_id)
}

function insertTable(path: string[]): void {
  props.controller.setSql(sqlWithTableQuery(state.value.sql, path))
}
</script>

<template>
  <section class="db-panel">
    <header class="db-head">
      <span class="db-title">{{ t('db.title') }}</span>
      <span
        v-if="active"
        class="head-conn"
      >
        <span class="head-name">{{ connectionLabel(active) }}</span>
        <span class="head-meta">
          {{ dbKindLabel(active.kind, translate) }} ·
          {{ connectionStateLabel(active.state, translate) }}
        </span>
      </span>
      <span
        v-if="state.readOnly"
        class="ro-badge"
      >{{ t('db.readOnly') }}</span>
      <span
        v-if="state.loading"
        class="head-busy"
      >{{ t('db.running') }}</span>
      <button
        type="button"
        class="db-close"
        :title="t('db.close')"
        @click="emit('close')"
      >
        ×
      </button>
    </header>

    <div class="db-body">
      <aside class="db-side">
        <div class="side-head">
          <span class="side-title">{{ t('db.connections') }}</span>
          <button
            type="button"
            class="add-btn"
            :title="t('db.add')"
            @click="openAdd"
          >
            ＋
          </button>
        </div>

        <form
          v-if="formOpen"
          class="conn-form"
          @submit.prevent="saveForm"
        >
          <label class="field">
            <span class="field-label">{{ t('db.name') }}</span>
            <input
              v-model="draft.name"
              class="field-input"
              type="text"
              spellcheck="false"
            >
          </label>
          <label class="field">
            <span class="field-label">{{ t('db.kind') }}</span>
            <select
              v-model="draft.kind"
              class="field-input"
            >
              <option
                v-for="kind in DB_KINDS"
                :key="kind"
                :value="kind"
              >
                {{ dbKindLabel(kind, translate) }}
              </option>
            </select>
          </label>
          <template v-if="draft.kind === 'sqlite'">
            <label class="field">
              <span class="field-label">{{ t('db.databasePath') }}</span>
              <input
                v-model="draft.database"
                class="field-input mono"
                type="text"
                spellcheck="false"
                :placeholder="t('db.databasePlaceholder')"
              >
            </label>
            <label class="check">
              <input
                v-model="draft.readOnly"
                type="checkbox"
              >
              <span>{{ t('db.readOnly') }}</span>
            </label>
          </template>
          <div
            v-else
            class="unsupported-note"
          >
            {{ t('db.sqliteOnly') }}
          </div>
          <div
            v-if="formError"
            class="form-error"
          >
            {{ formError }}
          </div>
          <div class="form-actions">
            <button
              type="submit"
              class="primary"
              :disabled="saving || draft.kind !== 'sqlite'"
            >
              {{ saving ? t('db.saving') : t('db.save') }}
            </button>
            <button
              type="button"
              @click="closeForm"
            >
              {{ t('db.cancel') }}
            </button>
          </div>
        </form>

        <div class="conn-list">
          <div
            v-for="connection in connections"
            :key="connection.connection_id"
            class="conn"
            :class="{ active: connection.connection_id === state.activeConnectionId }"
          >
            <button
              type="button"
              class="conn-main"
              @click="selectConnection(connection)"
            >
              <span class="conn-top">
                <span
                  class="conn-name"
                  :title="connectionLabel(connection)"
                >{{ connectionLabel(connection) }}</span>
                <span
                  v-if="connection.read_only"
                  class="ro-mini"
                >{{ t('db.readOnly') }}</span>
              </span>
              <span class="conn-sub">
                <span class="conn-kind">{{ dbKindLabel(connection.kind, translate) }}</span>
                <span
                  class="conn-state"
                  :class="`state-${connection.state}`"
                >{{ connectionStateLabel(connection.state, translate) }}</span>
              </span>
              <span
                v-if="connection.database"
                class="conn-db"
                :title="connection.database"
              >{{ connection.database }}</span>
              <span
                v-if="connection.error"
                class="conn-error"
                :title="connection.error"
              >{{ connection.error }}</span>
            </button>
            <div class="conn-actions">
              <template v-if="connection.kind === 'sqlite'">
                <button
                  v-if="connection.state === 'connected'"
                  type="button"
                  class="mini"
                  :disabled="state.loading"
                  @click="disconnect(connection)"
                >
                  {{ t('db.disconnect') }}
                </button>
                <button
                  v-else
                  type="button"
                  class="mini"
                  :disabled="!canConnect(connection) || state.loading"
                  @click="connect(connection)"
                >
                  {{ connection.state === 'connecting' ? t('db.connecting') : t('db.connect') }}
                </button>
              </template>
              <span
                v-else
                class="unsupported-mini"
              >{{ t('db.sqliteOnly') }}</span>
              <button
                type="button"
                class="mini"
                @click="openEdit(connection)"
              >
                {{ t('db.edit') }}
              </button>
              <template v-if="pendingDeleteId === connection.connection_id">
                <button
                  type="button"
                  class="mini danger"
                  @click="remove(connection)"
                >
                  {{ t('db.confirmDelete') }}
                </button>
                <button
                  type="button"
                  class="mini"
                  @click="pendingDeleteId = null"
                >
                  {{ t('db.cancel') }}
                </button>
              </template>
              <button
                v-else
                type="button"
                class="mini danger"
                @click="pendingDeleteId = connection.connection_id"
              >
                {{ t('db.delete') }}
              </button>
            </div>
          </div>
          <div
            v-if="connections.length === 0"
            class="empty"
          >
            {{ t('db.noConnections') }}
          </div>
        </div>

        <div class="schema-head">
          <span class="side-title">{{ t('db.schema') }}</span>
          <button
            v-if="active?.state === 'connected'"
            type="button"
            class="mini"
            :disabled="state.loading"
            @click="reloadSchema"
          >
            {{ t('db.reload') }}
          </button>
        </div>
        <div class="schema-body">
          <DbSchemaTree
            v-if="state.schemaNodes.length > 0"
            :nodes="state.schemaNodes"
            @insert-table="insertTable"
          />
          <div
            v-else
            class="empty schema-empty"
          >
            {{ schemaEmptyText }}
          </div>
        </div>
      </aside>

      <DbWorkbench
        class="db-workbench"
        :controller="props.controller"
      />
    </div>
  </section>
</template>

<style scoped>
.db-panel {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--bg);
}

.db-head {
  flex: none;
  height: 32px;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 10px;
  border-bottom: 1px solid var(--border);
  background: var(--panel);
}

.db-title {
  color: var(--fg);
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.04em;
}

.head-conn {
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: 6px;
}

.head-name {
  max-width: 220px;
  overflow: hidden;
  color: var(--fg-2);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.head-meta {
  color: var(--fg-faint);
  font-size: 10px;
}

.ro-badge {
  flex: none;
  padding: 0 5px;
  border: 1px solid var(--yellow);
  border-radius: var(--r-sm);
  color: var(--yellow);
  font-size: 10px;
  line-height: 15px;
}

.head-busy {
  color: var(--accent);
  font-size: 10px;
}

.db-close {
  margin-left: auto;
  width: 22px;
  height: 22px;
  color: var(--fg-faint);
  font-size: 15px;
  line-height: 1;
}

.db-close:hover {
  color: var(--fg);
}

.db-body {
  flex: 1;
  min-height: 0;
  display: flex;
}

.db-side {
  flex: none;
  width: 292px;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow-y: auto;
  border-right: 1px solid var(--border);
  background: var(--panel);
}

.side-head,
.schema-head {
  flex: none;
  display: flex;
  align-items: center;
  height: 26px;
  padding: 0 8px;
  border-bottom: 1px solid var(--border);
}

.schema-head {
  margin-top: 4px;
  border-top: 1px solid var(--border);
}

.side-title {
  flex: 1;
  color: var(--fg-dim);
  font-size: 10px;
  letter-spacing: 0.05em;
  text-transform: uppercase;
}

.add-btn {
  width: 20px;
  height: 20px;
  color: var(--fg-dim);
  font-size: 13px;
  line-height: 1;
}

.add-btn:hover {
  color: var(--accent);
}

.conn-form {
  flex: none;
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 8px;
  border-bottom: 1px solid var(--border);
  background: var(--panel-2);
}

.field {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.field-label {
  color: var(--fg-faint);
  font-size: 10px;
}

.field-input {
  height: 23px;
  padding: 0 6px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  outline: none;
  background: var(--bg);
  color: var(--fg);
  font-size: 11px;
}

.field-input:focus {
  border-color: var(--accent);
}

.field-input.mono {
  font-family: var(--font-mono);
  font-size: 10px;
}

.check {
  display: flex;
  align-items: center;
  gap: 5px;
  color: var(--fg-dim);
  font-size: 11px;
}

.check input {
  accent-color: var(--accent);
}

.unsupported-note {
  padding: 4px 6px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-3);
  color: var(--yellow);
  font-size: 10px;
}

.form-error {
  color: var(--red);
  font-size: 10px;
  line-height: 1.4;
}

.form-actions {
  display: flex;
  gap: 6px;
}

.form-actions button {
  height: 22px;
  padding: 0 9px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  color: var(--fg-2);
  font-size: 11px;
}

.form-actions button:hover:not(:disabled) {
  border-color: var(--accent);
  color: var(--fg);
}

.form-actions button.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--accent-fg);
}

.form-actions button.primary:hover:not(:disabled) {
  background: var(--accent-hov);
}

.form-actions button:disabled {
  color: var(--fg-faint);
  cursor: default;
}

.conn-list {
  flex: none;
}

.conn {
  border-bottom: 1px solid var(--border);
}

.conn.active {
  background: var(--accent-soft);
  box-shadow: inset 2px 0 0 var(--accent);
}

.conn-main {
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 1px;
  padding: 5px 8px 2px;
  text-align: left;
}

.conn-top {
  display: flex;
  align-items: center;
  gap: 5px;
  min-width: 0;
}

.conn-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  color: var(--fg);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ro-mini {
  flex: none;
  padding: 0 3px;
  border: 1px solid var(--yellow);
  border-radius: 2px;
  color: var(--yellow);
  font-size: 9px;
  line-height: 12px;
}

.conn-sub {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--fg-faint);
  font-size: 10px;
}

.conn-kind {
  color: var(--fg-dim);
}

.conn-state.state-connected {
  color: var(--green);
}

.conn-state.state-connecting {
  color: var(--yellow);
}

.conn-state.state-error {
  color: var(--red);
}

.conn-db,
.conn-error {
  overflow: hidden;
  color: var(--fg-faint);
  font-family: var(--font-mono);
  font-size: 10px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.conn-error {
  color: var(--red);
}

.conn-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  padding: 2px 8px 5px;
}

.mini {
  height: 18px;
  padding: 0 5px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  color: var(--fg-dim);
  font-size: 10px;
}

.mini:hover:not(:disabled) {
  border-color: var(--accent);
  color: var(--fg);
}

.mini:disabled {
  color: var(--fg-faint);
  cursor: default;
}

.mini.danger:hover:not(:disabled) {
  border-color: var(--red);
  color: var(--red);
}

.unsupported-mini {
  align-self: center;
  color: var(--yellow);
  font-size: 9px;
}

.schema-body {
  flex: 1;
  min-height: 120px;
  padding: 2px 4px;
  overflow: auto;
}

.db-workbench {
  flex: 1;
  min-width: 0;
}

.empty {
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 52px;
  padding: 10px;
  color: var(--fg-faint);
  font-size: 10px;
  text-align: center;
}

.schema-empty {
  height: 100%;
}
</style>
