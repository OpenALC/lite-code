<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { TaskDiffResult } from '../services/bridge'
import {
  MAX_RUNNING_TASKS,
  queuedReason,
  statusTone,
  taskStatusLabel,
  type TaskController,
  type Translate,
} from '../services/taskStore'
import type { TaskStepState } from '../services/taskTypes'

const props = defineProps<{ controller: TaskController }>()
const emit = defineEmits<{ collapse: [] }>()
const { t } = useI18n()

const state = ref(props.controller.getState())
const unsubscribe = props.controller.subscribe(next => {
  state.value = next
})
onBeforeUnmount(unsubscribe)

const translate: Translate = (key, params) => t(key, params ?? {})
const task = computed(
  () => state.value.tasks.find(item => item.task_id === state.value.activeTaskId) ?? null,
)
const taskId = computed(() => task.value?.task_id ?? null)
const status = computed(() => task.value?.status ?? null)
const statusText = computed(() =>
  task.value ? taskStatusLabel(task.value.status, translate) : '',
)
const tone = computed(() => (task.value ? statusTone(task.value.status) : 'muted'))
const queueText = computed(() => queuedReason(translate, MAX_RUNNING_TASKS))

const canStart = computed(() => status.value === 'planning' || status.value === 'queued')
const canCancel = computed(
  () => status.value !== null && status.value !== 'done' && status.value !== 'failed',
)
const canApply = computed(() => status.value === 'review')
const canDiscard = computed(() => status.value !== null && status.value !== 'done')

const pendingAction = ref<'apply' | 'discard' | null>(null)
const acting = ref(false)
const showDiff = ref(false)
const diff = ref<TaskDiffResult | null>(null)
const diffLoading = ref(false)
const diffError = ref<string | null>(null)

const isConflict = computed(() => {
  const message = state.value.error?.message.toLowerCase() ?? ''
  return message.includes('conflict') || message.includes('冲突')
})

watch(taskId, () => {
  pendingAction.value = null
  showDiff.value = false
  diff.value = null
  diffError.value = null
})

function shortSha(sha: string): string {
  return sha.slice(0, 8)
}

function stepIcon(stepState: TaskStepState): string {
  switch (stepState) {
    case 'pending':
      return '○'
    case 'running':
      return '◐'
    case 'done':
      return '●'
    case 'failed':
      return '×'
  }
}

async function act(kind: 'start' | 'cancel' | 'apply' | 'discard'): Promise<void> {
  const current = task.value
  if (!current || acting.value) return
  acting.value = true
  try {
    if (kind === 'start') await props.controller.start(current.task_id)
    else if (kind === 'cancel') await props.controller.cancel(current.task_id)
    else if (kind === 'apply') await props.controller.apply(current.task_id)
    else await props.controller.discard(current.task_id)
  } finally {
    acting.value = false
    pendingAction.value = null
  }
}

function confirmPending(): void {
  if (pendingAction.value) void act(pendingAction.value)
}

function requestAction(action: 'apply' | 'discard'): void {
  props.controller.clearError()
  pendingAction.value = action
}

async function toggleDiff(): Promise<void> {
  const current = task.value
  if (!current) return
  if (showDiff.value) {
    showDiff.value = false
    return
  }
  showDiff.value = true
  if (diff.value?.task_id === current.task_id) return

  diffLoading.value = true
  diffError.value = null
  const res = await props.controller.diff(current.task_id)
  diffLoading.value = false
  if (task.value?.task_id !== current.task_id) return
  if (!res.ok) {
    diffError.value = `${res.error.code}: ${res.error.message}`
    return
  }
  diff.value = res.data
}
</script>

<template>
  <section class="workbench">
    <header class="wb-head">
      <span class="wb-title">{{ t('task.workbenchTitle') }}</span>
      <span
        v-if="task"
        class="badge"
        :class="`tone-${tone}`"
      >{{ statusText }}</span>
      <button
        type="button"
        class="wb-collapse"
        :title="t('task.collapse')"
        @click="emit('collapse')"
      >
        ×
      </button>
    </header>

    <div
      v-if="!task"
      class="empty"
    >
      <div class="empty-title">
        {{ t('task.emptyTitle') }}
      </div>
      <div class="empty-desc">
        {{ t('task.emptyDesc') }}
      </div>
    </div>

    <template v-else>
      <div class="record">
        <div
          class="task-title"
          :title="task.title"
        >
          {{ task.title }}
        </div>

        <div
          v-if="state.lastMerged && state.lastMerged.taskId === task.task_id"
          class="banner ok"
        >
          {{ t('task.merged', { sha: shortSha(state.lastMerged.commitSha) }) }}
        </div>
        <div
          v-if="state.lastDiscarded === task.task_id"
          class="banner dim"
        >
          {{ t('task.discarded') }}
        </div>
        <div
          v-if="state.error"
          class="banner error"
        >
          <span class="banner-text">{{ state.error.message }}</span>
          <button
            type="button"
            class="banner-close"
            :title="t('task.dismissError')"
            @click="props.controller.clearError()"
          >
            ×
          </button>
        </div>
        <div
          v-if="state.error && isConflict"
          class="banner warn"
        >
          {{ t('task.conflict') }}
        </div>
        <div
          v-if="task.status === 'queued'"
          class="banner warn"
        >
          {{ queueText }}
        </div>

        <dl class="meta">
          <dt>{{ t('task.branch') }}</dt>
          <dd
            class="mono"
            :title="task.branch ?? ''"
          >
            {{ task.branch ?? t('task.none') }}
          </dd>
          <dt>{{ t('task.worktree') }}</dt>
          <dd
            class="mono"
            :title="task.worktree_path ?? ''"
          >
            {{ task.worktree_path ?? t('task.noWorktree') }}
          </dd>
          <dt>{{ t('task.base') }}</dt>
          <dd class="mono">
            {{ task.base_sha ? shortSha(task.base_sha) : t('task.none') }}
          </dd>
          <dt>{{ t('task.port') }}</dt>
          <dd class="mono">
            {{ task.port ?? t('task.none') }}
          </dd>
          <dt>{{ t('task.session') }}</dt>
          <dd
            class="mono"
            :title="task.session_id ?? ''"
          >
            {{ task.session_id ?? t('task.none') }}
          </dd>
        </dl>
      </div>

      <div class="wb-body">
        <section
          v-if="task.steps.length > 0"
          class="section"
        >
          <div class="section-title">
            {{ t('task.steps') }}
          </div>
          <ol class="steps">
            <li
              v-for="step in task.steps"
              :key="step.index"
              class="step"
              :class="`step-${step.state}`"
              :title="t(`task.stepStates.${step.state}`)"
            >
              <span class="step-icon">{{ stepIcon(step.state) }}</span>
              <span class="step-title">{{ step.title }}</span>
            </li>
          </ol>
        </section>

        <section class="section">
          <div class="section-title">
            {{ t('task.changes') }}
            <span class="count">{{ task.changes.length }}</span>
          </div>
          <div
            v-if="task.changes.length === 0"
            class="muted"
          >
            {{ t('task.noChanges') }}
          </div>
          <ul
            v-else
            class="changes"
          >
            <li
              v-for="change in task.changes"
              :key="change.path"
              class="change"
            >
              <span
                class="change-status"
                :class="`change-${change.status}`"
              >{{ t(`task.changeStatuses.${change.status}`) }}</span>
              <span
                class="change-path mono"
                :title="change.path"
              >{{ change.path }}</span>
              <span class="stat add">+{{ change.added }}</span>
              <span class="stat del">-{{ change.removed }}</span>
              <span
                v-if="change.applied"
                class="applied"
              >{{ t('task.applied') }}</span>
            </li>
          </ul>
        </section>

        <section class="section">
          <div class="section-title">
            {{ t('task.checks') }}
            <span class="count">{{ task.checks.length }}</span>
          </div>
          <div
            v-if="task.checks.length === 0"
            class="muted"
          >
            {{ t('task.noChecks') }}
          </div>
          <ul
            v-else
            class="checks"
          >
            <li
              v-for="check in task.checks"
              :key="check.name"
              class="check"
            >
              <div class="check-row">
                <span
                  class="check-ok"
                  :class="{ ok: check.ok, fail: !check.ok }"
                  :title="t(check.ok ? 'task.checkPassed' : 'task.checkFailed')"
                >{{ check.ok ? '✓' : '×' }}</span>
                <span class="check-name mono">{{ check.name }}</span>
                <span class="check-dur">{{ t('task.duration', { ms: check.duration_ms }) }}</span>
              </div>
              <pre
                v-if="!check.ok && check.output_tail"
                class="check-output"
              >{{ check.output_tail }}</pre>
            </li>
          </ul>
        </section>

        <section class="section">
          <div class="section-head">
            <button
              type="button"
              class="btn"
              :disabled="diffLoading"
              @click="toggleDiff"
            >
              {{ showDiff ? t('task.hideDiff') : t('task.diff') }}
            </button>
            <span
              v-if="diffLoading"
              class="muted"
            >{{ t('task.diffLoading') }}</span>
            <span
              v-else-if="diffError"
              class="error-text"
              :title="diffError"
            >{{ t('task.diffFailed') }}</span>
          </div>
          <pre
            v-if="showDiff && diff"
            class="diff mono"
          >{{ diff.patch || t('task.noChanges') }}</pre>
        </section>
      </div>

      <div
        v-if="pendingAction"
        class="confirm"
      >
        <span class="confirm-text">
          {{ pendingAction === 'apply' ? t('task.confirmApply') : t('task.confirmDiscard') }}
        </span>
        <button
          type="button"
          class="btn accent"
          :disabled="acting"
          @click="confirmPending"
        >
          {{ t('task.confirm') }}
        </button>
        <button
          type="button"
          class="btn"
          :disabled="acting"
          @click="pendingAction = null"
        >
          {{ t('task.dismiss') }}
        </button>
      </div>

      <footer class="actions">
        <button
          v-if="canStart"
          type="button"
          class="btn accent"
          :disabled="acting"
          @click="act('start')"
        >
          {{ t('task.start') }}
        </button>
        <button
          v-if="canCancel"
          type="button"
          class="btn"
          :disabled="acting"
          @click="act('cancel')"
        >
          {{ t('task.cancel') }}
        </button>
        <span class="spacer" />
        <button
          v-if="canDiscard"
          type="button"
          class="btn danger"
          :disabled="acting"
          @click="requestAction('discard')"
        >
          {{ t('task.discard') }}
        </button>
        <button
          v-if="canApply"
          type="button"
          class="btn accent"
          :disabled="acting"
          @click="requestAction('apply')"
        >
          {{ t('task.apply') }}
        </button>
      </footer>
    </template>
  </section>
</template>

<style scoped>
.workbench {
  flex: 1;
  min-width: 0;
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--panel);
}

.wb-head {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  height: 28px;
  padding: 0 8px;
  border-bottom: 1px solid var(--border);
}

.wb-title {
  font-size: 11px;
  color: var(--fg-dim);
  letter-spacing: 0.04em;
}

.wb-collapse {
  margin-left: auto;
  width: 20px;
  height: 20px;
  color: var(--fg-faint);
  font-size: 14px;
  line-height: 1;
}

.wb-collapse:hover {
  color: var(--fg);
}

.badge {
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

.empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 18px 14px;
}

.empty-title {
  color: var(--fg-2);
  font-size: 12px;
}

.empty-desc {
  color: var(--fg-faint);
  font-size: 11px;
  line-height: 1.5;
}

.record {
  flex: none;
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 8px;
  border-bottom: 1px solid var(--border);
}

.task-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--fg);
  font-size: 12px;
  font-weight: 600;
}

.banner {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  padding: 4px 6px;
  border-radius: var(--r-sm);
  font-size: 10px;
  line-height: 1.4;
  background: var(--panel-3);
  color: var(--fg-2);
}

.banner.ok {
  color: var(--green);
}

.banner.warn {
  color: var(--yellow);
}

.banner.error {
  color: var(--red);
}

.banner.dim {
  color: var(--fg-dim);
}

.banner-text {
  flex: 1;
  min-width: 0;
  overflow-wrap: anywhere;
}

.banner-close {
  flex: none;
  color: inherit;
  font-size: 12px;
  line-height: 1;
}

.meta {
  display: grid;
  grid-template-columns: max-content minmax(0, 1fr);
  gap: 2px 8px;
  margin: 0;
  font-size: 10px;
}

.meta dt {
  color: var(--fg-faint);
}

.meta dd {
  margin: 0;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--fg-dim);
}

.wb-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 8px;
}

.section {
  margin-bottom: 12px;
}

.section-title {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 4px;
  color: var(--fg-dim);
  font-size: 10px;
  letter-spacing: 0.04em;
  text-transform: uppercase;
}

.count {
  color: var(--fg-faint);
  font-family: var(--font-mono);
}

.muted {
  color: var(--fg-faint);
  font-size: 11px;
}

.section-head {
  display: flex;
  align-items: center;
  gap: 8px;
}

.steps {
  list-style: none;
  margin: 0;
  padding: 0;
}

.step {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 22px;
  font-size: 11px;
  color: var(--fg-dim);
}

.step-icon {
  width: 10px;
  flex: none;
  font-family: var(--font-mono);
  font-size: 10px;
}

.step-done .step-icon {
  color: var(--green);
}

.step-running .step-icon {
  color: var(--accent);
}

.step-failed .step-icon {
  color: var(--red);
}

.step-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.changes,
.checks {
  list-style: none;
  margin: 0;
  padding: 0;
}

.change {
  display: flex;
  align-items: center;
  gap: 6px;
  min-height: 22px;
  font-size: 11px;
  color: var(--fg-2);
}

.change-status {
  flex: none;
  width: 14px;
  text-align: center;
  font-family: var(--font-mono);
  font-size: 10px;
}

.change-modified {
  color: var(--yellow);
}

.change-added {
  color: var(--green);
}

.change-deleted {
  color: var(--red);
}

.change-renamed {
  color: var(--accent);
}

.change-path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.stat {
  flex: none;
  font-family: var(--font-mono);
  font-size: 10px;
}

.stat.add {
  color: var(--green);
}

.stat.del {
  color: var(--red);
}

.applied {
  flex: none;
  color: var(--fg-faint);
  font-size: 9px;
}

.check {
  padding: 3px 0;
}

.check-row {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
}

.check-ok {
  width: 12px;
  flex: none;
  text-align: center;
  font-family: var(--font-mono);
}

.check-ok.ok {
  color: var(--green);
}

.check-ok.fail {
  color: var(--red);
}

.check-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--fg-2);
}

.check-dur {
  flex: none;
  color: var(--fg-faint);
  font-family: var(--font-mono);
  font-size: 10px;
}

.check-output,
.diff {
  margin: 4px 0 0;
  padding: 6px;
  max-height: 220px;
  overflow: auto;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  color: var(--fg-dim);
  font-size: 10px;
  line-height: 1.5;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.btn {
  flex: none;
  height: 22px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-3);
  color: var(--fg-2);
  font-size: 11px;
}

.btn:hover:not(:disabled) {
  color: var(--fg);
  border-color: var(--accent);
}

.btn:disabled {
  color: var(--fg-faint);
  cursor: default;
}

.btn.accent {
  color: var(--accent);
  border-color: var(--accent-line);
}

.btn.danger {
  color: var(--red);
  border-color: rgb(217 122 112 / 45%);
}

.confirm {
  flex: none;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  padding: 6px 8px;
  border-top: 1px solid var(--border);
  background: var(--panel-2);
}

.confirm-text {
  flex: 1 1 100%;
  color: var(--fg-2);
  font-size: 10px;
  line-height: 1.4;
}

.actions {
  flex: none;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 8px;
  border-top: 1px solid var(--border);
}

.spacer {
  flex: 1;
}

.error-text {
  color: var(--red);
  font-size: 10px;
}

.mono {
  font-family: var(--font-mono);
}
</style>
