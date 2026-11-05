<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  queuedReason,
  statusTone,
  taskStatusLabel,
  type TaskController,
  type Translate,
} from '../services/taskStore'

const props = defineProps<{ controller: TaskController }>()
const emit = defineEmits<{ created: [] }>()
const { t } = useI18n()

const state = ref(props.controller.getState())
const unsubscribe = props.controller.subscribe(next => {
  state.value = next
})
onBeforeUnmount(unsubscribe)

const translate: Translate = (key, params) => t(key, params ?? {})
const tasks = computed(() => state.value.tasks)
const draft = ref('')
const creating = ref(false)
const canCreate = computed(() => draft.value.trim().length > 0 && !creating.value)

async function submit(): Promise<void> {
  if (!canCreate.value) return
  creating.value = true
  try {
    const created = await props.controller.create(draft.value)
    if (created) {
      draft.value = ''
      emit('created')
    }
  } finally {
    creating.value = false
  }
}

function hasInput(taskId: string): boolean {
  return state.value.needsInputTaskIds.includes(taskId)
}
</script>

<template>
  <section class="task-list">
    <header class="tl-head">
      <span class="tl-title">{{ t('task.title') }}</span>
      <span class="tl-count">{{ t('task.count', { count: tasks.length }) }}</span>
    </header>

    <form
      class="create"
      @submit.prevent="submit"
    >
      <input
        v-model="draft"
        class="create-input"
        type="text"
        :placeholder="t('task.createPlaceholder')"
        :disabled="creating"
        spellcheck="false"
      >
      <button
        type="submit"
        class="create-btn"
        :disabled="!canCreate"
      >
        {{ creating ? t('task.creating') : t('task.create') }}
      </button>
    </form>

    <div
      v-if="state.error"
      class="tl-error"
      :title="`${state.error.code}: ${state.error.message}`"
    >
      {{ state.error.message }}
    </div>

    <div class="rows">
      <button
        v-for="task in tasks"
        :key="task.task_id"
        type="button"
        class="row"
        :class="{ active: task.task_id === state.activeTaskId }"
        :title="task.title"
        @click="props.controller.setActive(task.task_id)"
      >
        <span class="row-top">
          <span
            class="badge"
            :class="`tone-${statusTone(task.status)}`"
          >{{ taskStatusLabel(task.status, translate) }}</span>
          <span class="row-title">{{ task.title }}</span>
          <span
            v-if="hasInput(task.task_id)"
            class="input-dot"
            :title="t('task.needsInputBadge')"
          >●</span>
        </span>
        <span
          v-if="task.branch || task.status === 'queued'"
          class="row-sub"
        >
          <span
            v-if="task.branch"
            class="branch"
          >{{ task.branch }}</span>
          <span
            v-if="task.status === 'queued'"
            class="queue"
          >{{ queuedReason(translate) }}</span>
        </span>
      </button>

      <div
        v-if="tasks.length === 0"
        class="empty"
      >
        {{ t('task.empty') }}
      </div>
    </div>
  </section>
</template>

<style scoped>
.task-list {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--panel);
}

.tl-head {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 28px;
  padding: 0 8px;
  border-bottom: 1px solid var(--border);
}

.tl-title {
  font-size: 11px;
  color: var(--fg-dim);
  letter-spacing: 0.04em;
}

.tl-count {
  margin-left: auto;
  color: var(--fg-faint);
  font-size: 10px;
}

.create {
  display: flex;
  gap: 4px;
  padding: 6px 8px;
  border-bottom: 1px solid var(--border);
}

.create-input {
  flex: 1;
  min-width: 0;
  height: 24px;
  padding: 0 6px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--bg);
  color: var(--fg);
  font-size: 11px;
}

.create-input:focus {
  border-color: var(--accent);
  outline: none;
}

.create-btn {
  flex: none;
  height: 24px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-3);
  color: var(--fg-2);
  font-size: 11px;
}

.create-btn:hover:not(:disabled) {
  color: var(--fg);
  border-color: var(--accent);
}

.create-btn:disabled {
  color: var(--fg-faint);
  cursor: default;
}

.tl-error {
  flex: none;
  padding: 4px 8px;
  border-bottom: 1px solid var(--border);
  color: var(--red);
  font-size: 10px;
  line-height: 1.4;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.rows {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.row {
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 5px 8px;
  text-align: left;
  border-left: 2px solid transparent;
}

.row:hover {
  background: var(--panel-2);
}

.row.active {
  background: var(--accent-soft);
  border-left-color: var(--accent);
}

.row-top {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.row-title {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
  color: var(--fg-2);
}

.row.active .row-title {
  color: var(--fg);
}

.badge {
  flex: none;
  padding: 0 5px;
  height: 16px;
  display: inline-flex;
  align-items: center;
  border: 1px solid var(--border);
  border-radius: 8px;
  font-size: 9px;
  line-height: 1;
  white-space: nowrap;
}

.tone-muted {
  color: var(--fg-faint);
  border-color: var(--border);
}

.tone-active {
  color: var(--accent);
  border-color: var(--accent-line);
}

.tone-warn {
  color: var(--yellow);
  border-color: rgb(217 174 95 / 45%);
}

.tone-accent {
  color: var(--accent-hov);
  border-color: var(--accent-line);
}

.tone-success {
  color: var(--green);
  border-color: rgb(121 185 138 / 45%);
}

.tone-danger {
  color: var(--red);
  border-color: rgb(217 122 112 / 45%);
}

.input-dot {
  flex: none;
  color: var(--yellow);
  font-size: 8px;
}

.row-sub {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  padding-left: 1px;
}

.branch,
.queue {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 10px;
  color: var(--fg-faint);
  font-family: var(--font-mono);
}

.queue {
  color: var(--yellow);
}

.empty {
  padding: 12px 10px;
  color: var(--fg-faint);
  font-size: 11px;
  line-height: 1.5;
}
</style>
