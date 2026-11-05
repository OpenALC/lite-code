<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { GitFileStatus } from '@lc-proto'

import {
  aheadBehindLabel,
  canCommit,
  groupFiles,
  statusLabel,
  statusTone,
  type GitController,
} from '../services/gitStore'

const props = defineProps<{ controller: GitController }>()
const emit = defineEmits<{ close: [] }>()
const { t } = useI18n()

const state = ref(props.controller.getState())
const unsubscribe = props.controller.subscribe(next => {
  state.value = next
})
onBeforeUnmount(unsubscribe)

const status = computed(() => state.value.status)
const groups = computed(() => groupFiles(state.value.status))
const branchText = computed(() => status.value?.branch ?? t('git.noBranch'))
const upstreamText = computed(() => status.value?.upstream ?? '')
const aheadBehind = computed(() => aheadBehindLabel(status.value))
const isClean = computed(() => status.value?.clean === true)
const commitEnabled = computed(() => canCommit(state.value))
const commitTitle = computed(() => {
  if (groups.value.staged.length === 0) return t('git.noStaged')
  if (state.value.commitMessage.trim().length === 0) return t('git.emptyMessage')
  return ''
})

const message = computed({
  get: () => state.value.commitMessage,
  set: (value: string) => props.controller.setCommitMessage(value),
})

const selected = reactive(new Set<string>())
const selectedPaths = computed(() =>
  groups.value.unstaged
    .map(file => file.path)
    .filter(path => selected.has(path)),
)
const allUnstagedSelected = computed(
  () =>
    groups.value.unstaged.length > 0 &&
    groups.value.unstaged.every(file => selected.has(file.path)),
)

function toggleSelected(path: string): void {
  if (selected.has(path)) selected.delete(path)
  else selected.add(path)
}

function toggleAllUnstaged(): void {
  if (allUnstagedSelected.value) {
    for (const file of groups.value.unstaged) selected.delete(file.path)
    return
  }
  for (const file of groups.value.unstaged) selected.add(file.path)
}

function selectFile(file: GitFileStatus): void {
  void props.controller.selectFile(file.path, file.staged)
}

function isActive(file: GitFileStatus): boolean {
  return (
    state.value.selectedPath === file.path &&
    state.value.diffStaged === file.staged
  )
}

async function stageOne(file: GitFileStatus): Promise<void> {
  const staged = await props.controller.stage([file.path])
  if (staged) selected.delete(file.path)
}

async function unstageOne(file: GitFileStatus): Promise<void> {
  await props.controller.unstage([file.path])
}

async function bulkStage(): Promise<void> {
  const paths =
    selectedPaths.value.length > 0
      ? [...selectedPaths.value]
      : groups.value.unstaged.map(file => file.path)
  if (paths.length === 0) return
  const staged = await props.controller.stage(paths)
  if (staged) selected.clear()
}

async function unstageAll(): Promise<void> {
  const paths = groups.value.staged.map(file => file.path)
  if (paths.length === 0) return
  await props.controller.unstage(paths)
}

async function refreshAll(): Promise<void> {
  await Promise.all([props.controller.refresh(), props.controller.loadLog()])
}

async function submitCommit(): Promise<void> {
  if (!commitEnabled.value) return
  await props.controller.commit(state.value.commitMessage)
}

function onCommitKey(event: KeyboardEvent): void {
  if (event.ctrlKey && event.key === 'Enter') {
    event.preventDefault()
    void submitCommit()
  }
}

const DIFF_COLLAPSE_LINES = 500
const diffExpanded = ref(false)
const diffLines = computed(() =>
  state.value.diff === null ? [] : state.value.diff.split('\n'),
)
const diffCollapsed = computed(
  () => !diffExpanded.value && diffLines.value.length > DIFF_COLLAPSE_LINES,
)
const visibleDiffLines = computed(() =>
  diffCollapsed.value
    ? diffLines.value.slice(0, DIFF_COLLAPSE_LINES)
    : diffLines.value,
)
const hasPatch = computed(() => (state.value.diff ?? '').trim().length > 0)

// 切换文件或暂存分组时重置折叠，避免把上一个 patch 的展开状态带过来。
watch(
  () => [state.value.selectedPath, state.value.diffStaged, state.value.diff],
  () => {
    diffExpanded.value = false
  },
)

type DiffLineKind = 'add' | 'del' | 'hunk' | 'meta' | 'context'

function diffLineKind(line: string): DiffLineKind {
  if (line.startsWith('@@')) return 'hunk'
  if (
    line.startsWith('+++') ||
    line.startsWith('---') ||
    line.startsWith('diff ') ||
    line.startsWith('index ') ||
    line.startsWith('new file ') ||
    line.startsWith('deleted file ') ||
    line.startsWith('similarity ') ||
    line.startsWith('rename ')
  ) {
    return 'meta'
  }
  if (line.startsWith('+')) return 'add'
  if (line.startsWith('-')) return 'del'
  return 'context'
}

function shortSha(sha: string, shortSha: string): string {
  return shortSha.length > 0 ? shortSha : sha.slice(0, 7)
}
</script>

<template>
  <section class="git-panel">
    <header class="git-head">
      <span class="git-title">{{ t('git.title') }}</span>
      <span
        v-if="status"
        class="branch"
        :title="status.branch ?? ''"
      >
        <span class="meta-label">{{ t('git.branch') }}</span>
        {{ branchText }}
      </span>
      <span
        v-if="upstreamText"
        class="upstream"
        :title="upstreamText"
      >
        <span class="meta-label">{{ t('git.upstream') }}</span>
        {{ upstreamText }}
      </span>
      <span
        v-if="aheadBehind"
        class="ahead-behind"
        :title="t('git.aheadBehind')"
      >{{ aheadBehind }}</span>
      <span
        v-if="isClean"
        class="chip clean"
      >{{ t('git.clean') }}</span>
      <span
        v-if="status?.merge_in_progress"
        class="chip warn"
      >{{ t('git.merging') }}</span>
      <span
        v-if="status?.rebase_in_progress"
        class="chip warn"
      >{{ t('git.rebasing') }}</span>
      <span
        v-if="state.loading"
        class="busy"
      >{{ t('git.loading') }}</span>
      <button
        type="button"
        class="head-btn"
        :disabled="state.loading"
        @click="refreshAll"
      >
        {{ t('git.refresh') }}
      </button>
      <button
        type="button"
        class="head-btn close"
        :title="t('git.close')"
        @click="emit('close')"
      >
        ×
      </button>
    </header>

    <div
      v-if="state.error"
      class="error-banner"
    >
      <span class="error-text">{{ t('git.error') }}: {{ state.error.message }}</span>
      <button
        type="button"
        class="error-dismiss"
        @click="props.controller.clearError()"
      >
        {{ t('git.dismiss') }}
      </button>
    </div>

    <div class="git-body">
      <aside class="files-pane">
        <div class="files-scroll">
          <section class="file-group">
            <header class="group-head">
              <span class="group-title">
                {{ t('git.staged') }}
                <span class="group-count">{{ groups.staged.length }}</span>
              </span>
              <button
                type="button"
                class="group-action"
                :disabled="state.loading || groups.staged.length === 0"
                @click="unstageAll"
              >
                {{ t('git.unstageAll') }}
              </button>
            </header>
            <div
              v-if="groups.staged.length === 0"
              class="group-empty"
            >
              {{ t('git.noStagedFiles') }}
            </div>
            <div
              v-for="file in groups.staged"
              :key="`s:${file.path}`"
              class="file-row"
              :class="{ active: isActive(file) }"
              role="button"
              tabindex="0"
              @click="selectFile(file)"
              @keydown.enter="selectFile(file)"
            >
              <span
                class="status-letter"
                :class="`tone-${statusTone(file.status)}`"
              >{{ statusLabel(file.status) }}</span>
              <span
                class="file-path"
                :title="file.path"
              >{{ file.path }}</span>
              <span class="stats">
                <span class="added">+{{ file.added }}</span>
                <span class="removed">-{{ file.removed }}</span>
              </span>
              <button
                type="button"
                class="row-action"
                :disabled="state.loading"
                :title="t('git.unstageFile', { path: file.path })"
                @click.stop="unstageOne(file)"
              >
                {{ t('git.unstage') }}
              </button>
            </div>
          </section>

          <section class="file-group">
            <header class="group-head">
              <label class="select-all">
                <input
                  type="checkbox"
                  :checked="allUnstagedSelected"
                  :disabled="state.loading || groups.unstaged.length === 0"
                  @change="toggleAllUnstaged"
                >
                <span>{{ t('git.selectAll') }}</span>
              </label>
              <span class="group-title">
                {{ t('git.unstaged') }}
                <span class="group-count">{{ groups.unstaged.length }}</span>
              </span>
              <button
                type="button"
                class="group-action"
                :disabled="state.loading || groups.unstaged.length === 0"
                @click="bulkStage"
              >
                {{
                  selectedPaths.length > 0
                    ? t('git.stageSelected', { count: selectedPaths.length })
                    : t('git.stageAll')
                }}
              </button>
            </header>
            <div
              v-if="groups.unstaged.length === 0"
              class="group-empty"
            >
              {{ t('git.noUnstagedFiles') }}
            </div>
            <div
              v-for="file in groups.unstaged"
              :key="`u:${file.path}`"
              class="file-row"
              :class="{ active: isActive(file), checked: selected.has(file.path) }"
              role="button"
              tabindex="0"
              @click="selectFile(file)"
              @keydown.enter="selectFile(file)"
            >
              <input
                type="checkbox"
                :checked="selected.has(file.path)"
                :disabled="state.loading"
                @click.stop
                @change="toggleSelected(file.path)"
              >
              <span
                class="status-letter"
                :class="`tone-${statusTone(file.status)}`"
              >{{ statusLabel(file.status) }}</span>
              <span
                class="file-path"
                :title="file.path"
              >{{ file.path }}</span>
              <span class="stats">
                <span class="added">+{{ file.added }}</span>
                <span class="removed">-{{ file.removed }}</span>
              </span>
              <button
                type="button"
                class="row-action"
                :disabled="state.loading"
                :title="t('git.stageFile', { path: file.path })"
                @click.stop="stageOne(file)"
              >
                {{ t('git.stage') }}
              </button>
            </div>
          </section>
        </div>

        <div class="commit-box">
          <label
            class="commit-label"
            for="git-commit-message"
          >{{ t('git.commitTitle') }}</label>
          <textarea
            id="git-commit-message"
            v-model="message"
            class="commit-input"
            :placeholder="t('git.commitPlaceholder')"
            :disabled="state.loading"
            rows="3"
            spellcheck="false"
            @keydown="onCommitKey"
          />
          <button
            type="button"
            class="commit-btn"
            :disabled="!commitEnabled"
            :title="commitTitle"
            @click="submitCommit"
          >
            {{ state.loading ? t('git.committing') : t('git.commit') }}
          </button>
        </div>

        <section class="log-pane">
          <header class="log-head">
            {{ t('git.recent') }}
          </header>
          <div
            v-if="state.log.length === 0"
            class="group-empty log-empty"
          >
            {{ t('git.noCommits') }}
          </div>
          <ul
            v-else
            class="commit-list"
          >
            <li
              v-for="entry in state.log"
              :key="entry.sha"
              class="commit-item"
            >
              <span class="commit-sha">{{ shortSha(entry.sha, entry.short_sha) }}</span>
              <span
                class="commit-author"
                :title="entry.author"
              >{{ entry.author }}</span>
              <span
                class="commit-summary"
                :title="entry.message"
              >{{ entry.message }}</span>
            </li>
          </ul>
        </section>
      </aside>

      <section class="diff-pane">
        <header class="diff-head">
          <span
            v-if="state.selectedPath"
            class="diff-path"
            :title="state.selectedPath"
          >{{ state.selectedPath }}</span>
          <span
            v-else
            class="diff-title"
          >{{ t('git.diffTitle') }}</span>
          <span
            v-if="state.selectedPath"
            class="diff-badge"
          >{{ state.diffStaged ? t('git.staged') : t('git.unstaged') }}</span>
          <span class="spacer" />
          <span
            v-if="state.selectedPath && hasPatch"
            class="diff-count"
          >{{ t('git.diffLines', { count: diffLines.length }) }}</span>
        </header>

        <div class="diff-scroll">
          <div
            v-if="!state.selectedPath"
            class="diff-empty"
          >
            {{ t('git.diffEmpty') }}
          </div>
          <div
            v-else-if="state.loading && state.diff === null"
            class="diff-empty"
          >
            {{ t('git.diffLoading') }}
          </div>
          <div
            v-else-if="state.diff === null"
            class="diff-empty"
          >
            {{ t('git.diffFailed') }}
          </div>
          <div
            v-else-if="!hasPatch"
            class="diff-empty"
          >
            {{ t('git.patchEmpty') }}
          </div>
          <pre
            v-else
            class="diff"
          ><span
            v-for="(line, index) in visibleDiffLines"
            :key="index"
            class="diff-line"
            :class="`line-${diffLineKind(line)}`"
          >{{ line === '' ? ' ' : line }}</span></pre>
          <div
            v-if="diffCollapsed"
            class="diff-more"
          >
            <span>{{ t('git.diffTruncated', { shown: visibleDiffLines.length, total: diffLines.length }) }}</span>
            <button
              type="button"
              class="link-btn"
              @click="diffExpanded = true"
            >
              {{ t('git.diffShowAll') }}
            </button>
          </div>
          <div
            v-else-if="diffLines.length > DIFF_COLLAPSE_LINES"
            class="diff-more"
          >
            <button
              type="button"
              class="link-btn"
              @click="diffExpanded = false"
            >
              {{ t('git.diffCollapse') }}
            </button>
          </div>
        </div>
      </section>
    </div>
  </section>
</template>

<style scoped>
.git-panel {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--bg);
}

.git-head {
  flex: none;
  display: flex;
  align-items: center;
  gap: 10px;
  height: 32px;
  padding: 0 10px;
  background: var(--panel);
  border-bottom: 1px solid var(--border);
  font-size: 11px;
  white-space: nowrap;
  overflow: hidden;
}

.git-title {
  color: var(--fg-2);
  font-weight: 600;
  letter-spacing: 0.04em;
}

.branch,
.upstream {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  color: var(--fg-2);
  font-family: var(--font-mono);
}

.meta-label {
  margin-right: 4px;
  color: var(--fg-faint);
  font-family: var(--font-ui);
}

.ahead-behind {
  color: var(--accent);
  font-family: var(--font-mono);
}

.chip {
  padding: 0 6px;
  border-radius: 8px;
  font-size: 10px;
  line-height: 16px;
}

.chip.clean {
  color: var(--green);
  background: rgb(121 185 138 / 12%);
}

.chip.warn {
  color: var(--yellow);
  background: rgb(217 174 95 / 12%);
}

.busy {
  color: var(--fg-faint);
}

.head-btn {
  height: 20px;
  padding: 0 7px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  color: var(--fg-dim);
  font-size: 11px;
}

.head-btn:hover:not(:disabled) {
  color: var(--fg);
  border-color: var(--accent);
}

.head-btn:disabled {
  color: var(--fg-faint);
}

.head-btn.close {
  margin-left: auto;
  padding: 0 5px;
  border-color: transparent;
  font-size: 14px;
  line-height: 1;
}

.error-banner {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 10px;
  background: rgb(217 122 112 / 10%);
  border-bottom: 1px solid rgb(217 122 112 / 30%);
  color: var(--red);
  font-size: 11px;
}

.error-text {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}

.error-dismiss {
  color: var(--red);
  text-decoration: underline;
  font-size: 11px;
}

.git-body {
  flex: 1;
  min-height: 0;
  display: flex;
}

.files-pane {
  flex: 0 0 340px;
  min-width: 260px;
  max-width: 45%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--panel);
  border-right: 1px solid var(--border);
}

.files-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
}

.file-group {
  padding-bottom: 4px;
  border-bottom: 1px solid var(--border);
}

.group-head {
  position: sticky;
  top: 0;
  z-index: 1;
  display: flex;
  align-items: center;
  gap: 6px;
  height: 25px;
  padding: 0 8px;
  background: var(--panel);
  color: var(--fg-dim);
}

.group-title {
  font-size: 10px;
  letter-spacing: 0.04em;
}

.group-count {
  margin-left: 4px;
  color: var(--fg-faint);
  font-family: var(--font-mono);
}

.select-all {
  display: flex;
  align-items: center;
  gap: 3px;
  color: var(--fg-faint);
  font-size: 10px;
  cursor: pointer;
}

.select-all input,
.file-row input {
  width: 11px;
  height: 11px;
  margin: 0;
  accent-color: var(--accent);
}

.group-action {
  margin-left: auto;
  padding: 0 4px;
  color: var(--accent);
  font-size: 10px;
}

.group-action:hover:not(:disabled) {
  text-decoration: underline;
}

.group-action:disabled {
  color: var(--fg-faint);
  text-decoration: none;
}

.group-empty {
  padding: 5px 10px;
  color: var(--fg-faint);
  font-size: 11px;
}

.file-row {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 24px;
  padding: 0 8px;
  border-right: 2px solid transparent;
  color: var(--fg-2);
  font-family: var(--font-mono);
  font-size: 11px;
  cursor: pointer;
}

.file-row:hover {
  background: var(--panel-2);
}

.file-row:focus-visible {
  outline: 1px solid var(--accent-line);
  outline-offset: -1px;
}

.file-row.active {
  background: var(--accent-soft);
  border-right-color: var(--accent);
  color: var(--fg);
}

.file-row.checked {
  background: var(--panel-2);
}

.status-letter {
  flex: none;
  width: 13px;
  font-weight: 600;
  text-align: center;
}

.tone-warning {
  color: var(--yellow);
}

.tone-success {
  color: var(--green);
}

.tone-danger {
  color: var(--red);
}

.tone-accent {
  color: var(--accent);
}

.file-path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.stats {
  flex: none;
  display: flex;
  gap: 4px;
  font-size: 10px;
}

.added {
  color: var(--green);
}

.removed {
  color: var(--red);
}

.row-action {
  flex: none;
  padding: 0 4px;
  color: var(--fg-faint);
  font-family: var(--font-ui);
  font-size: 10px;
}

.row-action:hover:not(:disabled) {
  color: var(--accent);
}

.row-action:disabled {
  opacity: 0.5;
}

.commit-box {
  flex: none;
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 8px;
  border-top: 1px solid var(--border);
  background: var(--panel);
}

.commit-label {
  color: var(--fg-dim);
  font-size: 10px;
  letter-spacing: 0.04em;
}

.commit-input {
  width: 100%;
  min-height: 52px;
  padding: 5px 6px;
  resize: vertical;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--bg);
  color: var(--fg);
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.4;
}

.commit-input:focus {
  border-color: var(--accent);
  outline: none;
}

.commit-input:disabled {
  opacity: 0.6;
}

.commit-btn {
  height: 24px;
  border: 1px solid var(--accent-line);
  border-radius: var(--r-sm);
  background: var(--accent-soft);
  color: var(--accent);
  font-size: 11px;
}

.commit-btn:hover:not(:disabled) {
  background: var(--accent);
  color: var(--accent-fg);
}

.commit-btn:disabled {
  border-color: var(--border);
  background: var(--panel-2);
  color: var(--fg-faint);
}

.log-pane {
  flex: 0 1 168px;
  min-height: 64px;
  display: flex;
  flex-direction: column;
  border-top: 1px solid var(--border);
}

.log-head {
  flex: none;
  height: 23px;
  padding: 0 8px;
  line-height: 23px;
  color: var(--fg-dim);
  font-size: 10px;
  letter-spacing: 0.04em;
}

.log-empty {
  padding-top: 0;
}

.commit-list {
  flex: 1;
  min-height: 0;
  margin: 0;
  padding: 0 0 4px;
  overflow: auto;
  list-style: none;
}

.commit-item {
  display: grid;
  grid-template-columns: 52px minmax(48px, 88px) minmax(0, 1fr);
  gap: 6px;
  padding: 2px 8px;
  font-size: 10px;
  line-height: 1.5;
}

.commit-sha {
  color: var(--accent);
  font-family: var(--font-mono);
}

.commit-author {
  overflow: hidden;
  color: var(--fg-dim);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.commit-summary {
  overflow: hidden;
  color: var(--fg-2);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.diff-pane {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--bg);
}

.diff-head {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  height: 28px;
  padding: 0 10px;
  border-bottom: 1px solid var(--border);
  background: var(--panel);
  font-size: 11px;
}

.diff-path {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--fg);
  font-family: var(--font-mono);
}

.diff-title {
  color: var(--fg-dim);
}

.diff-badge {
  flex: none;
  padding: 0 6px;
  border-radius: 8px;
  background: var(--panel-3);
  color: var(--fg-dim);
  font-size: 10px;
  line-height: 16px;
}

.diff-count {
  flex: none;
  color: var(--fg-faint);
  font-family: var(--font-mono);
  font-size: 10px;
}

.spacer {
  flex: 1;
}

.diff-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
}

.diff-empty {
  padding: 16px 12px;
  color: var(--fg-faint);
  font-size: 12px;
}

.diff {
  min-width: 100%;
  width: max-content;
  margin: 0;
  padding: 4px 0;
  color: var(--fg-2);
  font-family: var(--font-mono);
  font-size: 11.5px;
  line-height: 1.5;
  white-space: pre;
  tab-size: 4;
}

.diff-line {
  display: block;
  min-height: 1.5em;
  padding: 0 12px;
}

.line-add {
  background: rgb(121 185 138 / 12%);
  color: var(--green);
}

.line-del {
  background: rgb(217 122 112 / 12%);
  color: var(--red);
}

.line-hunk {
  background: var(--accent-soft);
  color: var(--accent);
}

.line-meta {
  color: var(--fg-faint);
}

.line-context {
  color: var(--fg-2);
}

.diff-more {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 12px;
  border-top: 1px solid var(--border);
  color: var(--fg-faint);
  font-size: 11px;
}

.link-btn {
  color: var(--accent);
  font-size: 11px;
}

.link-btn:hover {
  text-decoration: underline;
}
</style>
