<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  activeConnection,
  connectionLabel,
  connectionStateLabel,
  dbKindLabel,
  isUntrusted,
  resultSummary,
  type DbController,
  type Translate,
} from '../services/dbStore'

const props = defineProps<{ controller: DbController }>()
const { t } = useI18n()

const state = ref(props.controller.getState())
const unsubscribe = props.controller.subscribe(next => {
  state.value = next
})
onBeforeUnmount(unsubscribe)

const translate: Translate = (key, params) => t(key, params ?? {})
const active = computed(() => activeConnection(state.value))
const connected = computed(() => active.value?.state === 'connected')
const canQuery = computed(() => connected.value && !state.value.loading)
const canExecute = computed(() => canQuery.value && !state.value.readOnly)
const summary = computed(() => resultSummary(state.value.result))
const untrusted = computed(() => isUntrusted(state.value.result))
const sql = computed({
  get: () => state.value.sql,
  set: value => props.controller.setSql(value),
})

type RunKind = 'query' | 'execute' | 'explain'

function run(kind: RunKind): void {
  if (state.value.loading) return
  if (kind === 'query') {
    if (canQuery.value) void props.controller.query()
  } else if (kind === 'execute') {
    if (canExecute.value) void props.controller.execute()
  } else if (canQuery.value) {
    void props.controller.explain()
  }
}

function onSqlKeydown(event: KeyboardEvent): void {
  if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') {
    event.preventDefault()
    run('query')
  }
}
</script>

<template>
  <section class="db-workbench">
    <div class="wb-toolbar">
      <div class="wb-status">
        <template v-if="active">
          <span class="active-name">{{ connectionLabel(active) }}</span>
          <span class="meta">
            {{ dbKindLabel(active.kind, translate) }} ·
            {{ connectionStateLabel(active.state, translate) }}
          </span>
          <span
            v-if="state.readOnly"
            class="ro-badge"
          >{{ t('db.readOnly') }}</span>
        </template>
        <span
          v-else
          class="meta"
        >{{ t('db.noConnection') }}</span>
      </div>
      <div class="wb-actions">
        <span
          v-if="state.loading"
          class="busy"
        >{{ t('db.running') }}</span>
        <button
          type="button"
          class="run-btn"
          :disabled="!canQuery"
          :title="t('db.queryHint')"
          @click="run('query')"
        >
          {{ t('db.query') }}
        </button>
        <button
          type="button"
          class="run-btn"
          :disabled="!canExecute"
          :title="state.readOnly ? t('db.readOnlyHint') : undefined"
          @click="run('execute')"
        >
          {{ t('db.execute') }}
        </button>
        <button
          type="button"
          class="run-btn ghost"
          :disabled="!canQuery"
          @click="run('explain')"
        >
          {{ t('db.explain') }}
        </button>
      </div>
    </div>

    <textarea
      v-model="sql"
      class="sql-editor"
      spellcheck="false"
      :placeholder="t('db.sqlPlaceholder')"
      @keydown="onSqlKeydown"
    />

    <div
      v-if="state.error"
      class="error-bar"
      role="alert"
    >
      <span class="error-code">{{ state.error.code }}</span>
      <span class="error-msg">{{ state.error.message }}</span>
      <button
        type="button"
        class="error-dismiss"
        @click="props.controller.clearError()"
      >
        {{ t('db.dismiss') }}
      </button>
    </div>

    <div class="output">
      <div
        v-if="!connected"
        class="empty"
      >
        {{ t('db.noConnection') }}
      </div>
      <template v-else>
        <div
          v-if="state.result"
          class="pane"
        >
          <div class="pane-head">
            <span>{{ t('db.resultRows', { count: summary.rows }) }}</span>
            <span>{{ t('db.resultColumns', { count: summary.columns }) }}</span>
            <span>{{ t('db.resultElapsed', { ms: summary.elapsedMs }) }}</span>
          </div>
          <div
            v-if="untrusted"
            class="untrusted"
          >
            {{ t('db.untrusted') }}
          </div>
          <div
            v-if="summary.truncated"
            class="truncated"
          >
            {{ t('db.truncated', { count: summary.sampleRows }) }}
          </div>
          <div class="grid-scroll">
            <table class="grid">
              <thead>
                <tr>
                  <th
                    v-for="(column, columnIndex) in state.result.columns"
                    :key="`${column.name}-${columnIndex}`"
                    :title="column.type_name"
                  >
                    <span class="col-name">{{ column.name }}</span>
                    <span class="col-type">{{ column.type_name }}</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="(row, rowIndex) in state.result.rows"
                  :key="rowIndex"
                >
                  <td
                    v-for="(cell, cellIndex) in row"
                    :key="cellIndex"
                  >
                    <span
                      v-if="cell === null"
                      class="null"
                    >NULL</span>
                    <span
                      v-else
                      class="cell"
                    >{{ cell }}</span>
                  </td>
                </tr>
              </tbody>
            </table>
            <div
              v-if="state.result.rows.length === 0"
              class="empty"
            >
              {{ t('db.emptyRows') }}
            </div>
          </div>
        </div>
        <div
          v-else-if="state.execResult"
          class="pane"
        >
          <div class="pane-head">
            <span>{{ t('db.affectedRows', { count: state.execResult.affected_rows }) }}</span>
            <span>{{ t('db.resultElapsed', { ms: state.execResult.elapsed_ms }) }}</span>
          </div>
        </div>
        <div
          v-else-if="state.explainResult"
          class="pane"
        >
          <div class="pane-head">
            <span>{{ t('db.explainPlan') }}</span>
            <span>{{ t('db.resultElapsed', { ms: state.explainResult.elapsed_ms }) }}</span>
          </div>
          <pre class="plan">{{ state.explainResult.plan }}</pre>
        </div>
        <div
          v-else-if="!state.loading"
          class="empty"
        >
          {{ t('db.emptyResult') }}
        </div>
      </template>
    </div>
  </section>
</template>

<style scoped>
.db-workbench {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--bg);
}

.wb-toolbar {
  flex: none;
  min-height: 30px;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 8px;
  border-bottom: 1px solid var(--border);
  background: var(--panel);
}

.wb-status {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
}

.active-name {
  max-width: 240px;
  overflow: hidden;
  color: var(--fg);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.meta {
  overflow: hidden;
  color: var(--fg-faint);
  text-overflow: ellipsis;
  white-space: nowrap;
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

.wb-actions {
  flex: none;
  display: flex;
  align-items: center;
  gap: 5px;
}

.busy {
  color: var(--accent);
  font-size: 10px;
}

.run-btn {
  height: 21px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-3);
  color: var(--fg-2);
  font-size: 11px;
}

.run-btn:hover:not(:disabled) {
  border-color: var(--accent);
  color: var(--fg);
}

.run-btn:disabled {
  color: var(--fg-faint);
  cursor: default;
}

.run-btn.ghost {
  background: transparent;
}

.sql-editor {
  flex: none;
  height: 112px;
  min-height: 72px;
  max-height: 240px;
  resize: vertical;
  padding: 8px 10px;
  border: 0;
  border-bottom: 1px solid var(--border);
  outline: none;
  background: var(--panel-2);
  color: var(--fg);
  font-family: var(--font-mono);
  font-size: 12px;
  line-height: 1.55;
  tab-size: 2;
}

.sql-editor:focus {
  box-shadow: inset 0 0 0 1px var(--accent-line);
}

.error-bar {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 10px;
  border-bottom: 1px solid var(--border);
  background: rgb(217 122 112 / 12%);
  color: var(--red);
  font-size: 11px;
}

.error-code {
  flex: none;
  font-family: var(--font-mono);
  font-size: 10px;
  opacity: 0.8;
}

.error-msg {
  flex: 1;
  min-width: 0;
  overflow-wrap: anywhere;
}

.error-dismiss {
  flex: none;
  color: var(--fg-dim);
  font-size: 10px;
}

.error-dismiss:hover {
  color: var(--fg);
}

.output {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.pane {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.pane-head {
  flex: none;
  display: flex;
  gap: 14px;
  padding: 5px 10px;
  border-bottom: 1px solid var(--border);
  color: var(--fg-dim);
  font-family: var(--font-mono);
  font-size: 10px;
}

.untrusted,
.truncated {
  flex: none;
  padding: 4px 10px;
  border-bottom: 1px solid var(--border);
  font-size: 10px;
}

.untrusted {
  background: var(--accent-soft);
  color: var(--accent);
}

.truncated {
  background: rgb(217 174 95 / 10%);
  color: var(--yellow);
}

.grid-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
}

.grid {
  width: max-content;
  min-width: 100%;
  border-collapse: separate;
  border-spacing: 0;
  font-family: var(--font-mono);
  font-size: 11px;
}

.grid th {
  position: sticky;
  top: 0;
  z-index: 1;
  padding: 4px 10px;
  border-right: 1px solid var(--border);
  border-bottom: 1px solid var(--border-strong);
  background: var(--panel-2);
  color: var(--fg-2);
  font-weight: 500;
  text-align: left;
  white-space: nowrap;
}

.col-name {
  margin-right: 6px;
}

.col-type {
  color: var(--fg-faint);
  font-size: 9px;
  font-weight: 400;
}

.grid td {
  max-width: 320px;
  padding: 3px 10px;
  overflow: hidden;
  border-right: 1px solid var(--border);
  border-bottom: 1px solid var(--border);
  color: var(--fg);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.grid tbody tr:hover td {
  background: var(--panel-2);
}

.null {
  color: var(--fg-faint);
  font-style: italic;
}

.plan {
  flex: 1;
  margin: 0;
  padding: 10px;
  overflow: auto;
  color: var(--fg-2);
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.5;
  white-space: pre-wrap;
}

.empty {
  display: flex;
  flex: 1;
  align-items: center;
  justify-content: center;
  min-height: 80px;
  padding: 12px;
  color: var(--fg-faint);
  font-size: 11px;
  text-align: center;
}
</style>
