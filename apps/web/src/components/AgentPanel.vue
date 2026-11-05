<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { ApprovalDecision, ApprovalSource, SessionMode } from '@lc-proto'
import {
  contextRatio,
  type ToolCard,
} from '../services/agentStore'
import {
  DEFAULT_CONTEXT_K,
  type AgentSessionController,
} from '../services/agentSession'
import { isTauri } from '../services/bridge'

const props = defineProps<{ controller: AgentSessionController }>()
const { t } = useI18n()

const MODES: SessionMode[] = ['plan', 'chat', 'auto', 'yolo']
const TOOL_PREVIEW_CHARS = 600
const TOOL_PREVIEW_LINES = 8

const state = ref(props.controller.getState())
const unsubscribe = props.controller.subscribe(next => {
  state.value = next
})
onBeforeUnmount(unsubscribe)

const tauri = isTauri()
const agent = computed(() => state.value.agent)
const messages = computed(() => agent.value?.messages ?? [])
const tools = computed(() => agent.value?.tools ?? [])
const approvals = computed(() => agent.value?.approvals ?? [])
const streaming = computed(() => state.value.streaming)
const approvalPending = computed(() => approvals.value.length > 0)
const switchDisabled = computed(
  () => streaming.value || approvalPending.value || switching.value,
)

const draft = computed({
  get: () => state.value.draft,
  set: (value: string) => props.controller.setDraft(value),
})

const pendingYolo = ref(false)
const switching = ref(false)
const expandedTools = ref<Set<string>>(new Set())
const streamEl = ref<HTMLElement | null>(null)

const usedTokens = computed(() => {
  const usage = agent.value?.usage
  return usage ? usage.input + usage.output : 0
})
const ratio = computed(() =>
  agent.value ? contextRatio(agent.value, DEFAULT_CONTEXT_K) : 0,
)
const contextPct = computed(() => Math.round(ratio.value * 100))
const contextWarn = computed(() => ratio.value >= 0.8)
const contextText = computed(() => {
  const used = usedTokens.value
  const usedLabel = used >= 1000 ? `${(used / 1000).toFixed(1)}K` : String(used)
  return `${usedLabel} / ${DEFAULT_CONTEXT_K}K`
})
const turnText = computed(() =>
  agent.value ? `${agent.value.turn}/${agent.value.maxTurns}` : '—',
)
const composerHint = computed(() => {
  if (!tauri) return t('agent.browserDemo')
  if (streaming.value) return t('agent.streaming')
  if (approvalPending.value) return t('agent.approval.pendingHint')
  return t('agent.sendHint')
})
const sendDisabled = computed(
  () =>
    !tauri ||
    streaming.value ||
    approvalPending.value ||
    switching.value ||
    draft.value.trim().length === 0,
)

const streamSignature = computed(() => {
  const current = agent.value
  const last = current?.messages[current.messages.length - 1]
  const lastTool = current?.tools[current.tools.length - 1]
  return [
    current?.messages.length ?? 0,
    last?.text.length ?? 0,
    last?.thinking?.length ?? 0,
    current?.tools.length ?? 0,
    lastTool?.text?.length ?? 0,
    current?.approvals.length ?? 0,
  ].join(':')
})
watch(
  streamSignature,
  () => {
    const el = streamEl.value
    if (el) el.scrollTop = el.scrollHeight
  },
  { flush: 'post' },
)

function toolStatusLabel(status: ToolCard['status']): string {
  return t(`agent.tool.${status}`)
}

function sourceLabel(source: ApprovalSource): string {
  return t(`agent.sources.${source}`)
}

function isResolving(approvalId: number): boolean {
  return state.value.resolvingApprovals.includes(approvalId)
}

function isToolExpanded(callId: string): boolean {
  return expandedTools.value.has(callId)
}

function isToolLong(tool: ToolCard): boolean {
  const text = tool.text ?? ''
  return text.length > TOOL_PREVIEW_CHARS || text.split('\n').length > TOOL_PREVIEW_LINES
}

function toolText(tool: ToolCard): string {
  const text = tool.text ?? ''
  if (!isToolLong(tool) || isToolExpanded(tool.callId)) return text
  const byLines = text.split('\n').slice(0, TOOL_PREVIEW_LINES).join('\n')
  const clipped =
    byLines.length > TOOL_PREVIEW_CHARS ? byLines.slice(0, TOOL_PREVIEW_CHARS) : byLines
  return `${clipped}…`
}

function toggleTool(callId: string): void {
  const next = new Set(expandedTools.value)
  if (next.has(callId)) next.delete(callId)
  else next.add(callId)
  expandedTools.value = next
}

function selectMode(mode: SessionMode): void {
  if (mode === state.value.mode || switchDisabled.value) return
  if (mode === 'yolo') {
    pendingYolo.value = true
    return
  }
  pendingYolo.value = false
  switching.value = true
  void props.controller.switchMode(mode).finally(() => {
    switching.value = false
  })
}

async function confirmYolo(): Promise<void> {
  if (switching.value) return
  switching.value = true
  const switched = await props.controller.switchMode('yolo', { confirmed: true })
  switching.value = false
  if (switched) pendingYolo.value = false
}

async function submit(): Promise<void> {
  if (sendDisabled.value) return
  await props.controller.send()
}

function cancel(): void {
  void props.controller.cancel()
}

function decide(approvalId: number, decision: ApprovalDecision): void {
  void props.controller.resolve(approvalId, decision)
}
</script>

<template>
  <div class="agent-body">
    <div class="agent-bar">
      <div
        class="mode-group"
        role="radiogroup"
        :aria-label="t('agent.modeLabel')"
      >
        <button
          v-for="mode in MODES"
          :key="mode"
          type="button"
          role="radio"
          class="mode-btn"
          :class="{ active: state.mode === mode, yolo: mode === 'yolo' }"
          :aria-checked="state.mode === mode"
          :disabled="switchDisabled"
          @click="selectMode(mode)"
        >
          {{ t(`agent.modes.${mode}`) }}
        </button>
      </div>
      <div class="bar-meta">
        <span class="turn">{{ t('agent.turn') }} {{ turnText }}</span>
        <button
          v-if="streaming"
          type="button"
          class="stop"
          @click="cancel"
        >
          {{ t('agent.stop') }}
        </button>
      </div>
    </div>

    <div
      class="context-row"
      :title="t('agent.contextHint', { k: DEFAULT_CONTEXT_K })"
    >
      <span class="context-label">{{ t('agent.context') }}</span>
      <span class="context-track">
        <span
          class="context-fill"
          :class="{ warn: contextWarn }"
          :style="{ width: `${contextPct}%` }"
        />
      </span>
      <span class="context-text">{{ contextText }}</span>
      <span class="context-note">{{ t('agent.contextAssumed', { k: DEFAULT_CONTEXT_K }) }}</span>
    </div>

    <div
      ref="streamEl"
      class="stream"
    >
      <div
        v-if="!agent"
        class="empty"
      >
        <h3>{{ t('agent.emptyTitle') }}</h3>
        <p>{{ t('agent.emptyDesc') }}</p>
        <p
          v-if="!tauri"
          class="demo"
        >
          {{ t('agent.browserDemo') }}
        </p>
      </div>

      <template
        v-for="message in messages"
        :key="message.id"
      >
        <div
          class="msg"
          :class="message.role"
        >
          <details
            v-if="message.thinking"
            class="thinking"
          >
            <summary>{{ t('agent.thinking') }}</summary>
            <pre>{{ message.thinking }}</pre>
          </details>
          <div
            v-if="message.text"
            class="bubble"
          >
            {{ message.text }}
          </div>
        </div>
      </template>

      <div
        v-for="tool in tools"
        :key="tool.callId"
        class="tool-card"
        :class="[`tool-${tool.status}`, { expanded: isToolExpanded(tool.callId) }]"
      >
        <div class="tool-head">
          <span class="tool-name">{{ tool.name }}</span>
          <span class="tool-status">{{ toolStatusLabel(tool.status) }}</span>
        </div>
        <pre
          v-if="tool.text"
          class="tool-result"
        >{{ toolText(tool) }}</pre>
        <button
          v-if="isToolLong(tool)"
          type="button"
          class="link"
          @click="toggleTool(tool.callId)"
        >
          {{ isToolExpanded(tool.callId) ? t('agent.tool.collapse') : t('agent.tool.expand') }}
        </button>
      </div>

      <div
        v-for="approval in approvals"
        :key="approval.approvalId"
        class="approval-card"
      >
        <div class="approval-head">
          <span class="approval-title">{{ t('agent.approval.title') }}</span>
          <span class="approval-tool">{{ approval.tool }}</span>
        </div>
        <div
          v-if="state.mode === 'yolo'"
          class="approval-note"
        >
          {{ t('agent.approval.yoloNotice') }}
        </div>
        <pre
          v-if="approval.command"
          class="approval-command"
        >{{ approval.command }}</pre>
        <dl class="approval-meta">
          <dt>{{ t('agent.approval.source') }}</dt>
          <dd>{{ sourceLabel(approval.source) }}</dd>
          <dt>{{ t('agent.approval.reason') }}</dt>
          <dd>{{ approval.reason }}</dd>
        </dl>
        <div class="approval-actions">
          <button
            type="button"
            :disabled="isResolving(approval.approvalId)"
            @click="decide(approval.approvalId, 'allow_once')"
          >
            {{ t('agent.approval.allowOnce') }}
          </button>
          <button
            type="button"
            :disabled="isResolving(approval.approvalId)"
            @click="decide(approval.approvalId, 'allow_session')"
          >
            {{ t('agent.approval.allowSession') }}
          </button>
          <button
            type="button"
            class="deny"
            :disabled="isResolving(approval.approvalId)"
            @click="decide(approval.approvalId, 'deny')"
          >
            {{ t('agent.approval.deny') }}
          </button>
          <span
            v-if="isResolving(approval.approvalId)"
            class="resolving"
          >{{ t('agent.approval.resolving') }}</span>
        </div>
      </div>
    </div>

    <div
      v-if="pendingYolo"
      class="yolo-confirm"
      role="alert"
    >
      <div class="yolo-text">
        <strong>{{ t('agent.yolo.title') }}</strong>
        <span>{{ t('agent.yolo.desc') }}</span>
      </div>
      <div class="yolo-actions">
        <button
          type="button"
          class="danger"
          :disabled="switching"
          @click="confirmYolo"
        >
          {{ t('agent.yolo.confirm') }}
        </button>
        <button
          type="button"
          :disabled="switching"
          @click="pendingYolo = false"
        >
          {{ t('agent.yolo.cancel') }}
        </button>
      </div>
    </div>

    <div
      v-if="state.error"
      class="error-banner"
      role="alert"
    >
      <span class="error-code">{{ state.error.code }}</span>
      <span class="error-message">{{ state.error.message }}</span>
    </div>

    <div class="composer">
      <textarea
        v-model="draft"
        rows="2"
        spellcheck="false"
        :placeholder="t('agent.composerPlaceholder')"
        @keydown.ctrl.enter.prevent="submit"
        @keydown.meta.enter.prevent="submit"
      />
      <div class="composer-foot">
        <span class="hint">{{ composerHint }}</span>
        <button
          type="button"
          class="send"
          :disabled="sendDisabled"
          @click="submit"
        >
          {{ t('agent.send') }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.agent-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--bg);
}

.agent-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  padding: 6px 8px;
  border-bottom: 1px solid var(--border);
}

.mode-group {
  display: flex;
  gap: 2px;
  padding: 2px;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
}

.mode-btn {
  padding: 2px 7px;
  border-radius: 3px;
  color: var(--fg-dim);
  font-size: 11px;
}

.mode-btn:hover:not(:disabled) {
  color: var(--fg);
}

.mode-btn.active {
  color: var(--accent-fg);
  background: var(--accent);
}

.mode-btn.yolo {
  color: var(--yellow);
}

.mode-btn.yolo.active {
  color: var(--accent);
  background: var(--accent-soft);
  outline: 1px solid var(--accent-line);
}

.mode-btn:disabled {
  opacity: 0.5;
  cursor: default;
}

.bar-meta {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--fg-faint);
  font-size: 11px;
}

.stop {
  padding: 2px 8px;
  border: 1px solid rgb(217 122 112 / 40%);
  border-radius: var(--r-sm);
  color: var(--red);
  font-size: 11px;
}

.stop:hover {
  color: var(--fg);
  border-color: var(--red);
}

.context-row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 8px 6px;
  color: var(--fg-faint);
  font-size: 10px;
}

.context-track {
  flex: 0 1 96px;
  height: 3px;
  background: var(--panel-3);
  border-radius: 2px;
  overflow: hidden;
}

.context-fill {
  display: block;
  height: 100%;
  background: var(--accent);
}

.context-fill.warn {
  background: var(--yellow);
}

.context-text {
  color: var(--fg-dim);
  font-family: var(--font-mono);
}

.context-note {
  margin-left: auto;
}

.stream {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 8px;
}

.empty {
  max-width: 320px;
  margin: auto;
  padding: 16px;
  color: var(--fg-dim);
  text-align: center;
}

.empty h3 {
  margin: 0 0 6px;
  color: var(--fg-2);
  font-size: 13px;
}

.empty p {
  margin: 0;
  font-size: 11px;
  line-height: 1.6;
}

.empty .demo {
  margin-top: 8px;
  color: var(--yellow);
}

.msg {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.msg.user {
  align-items: flex-end;
}

.bubble {
  padding: 6px 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-2);
  font-size: 12px;
  line-height: 1.6;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.msg.user .bubble {
  max-width: 85%;
  background: var(--panel-3);
}

.msg.assistant .bubble {
  width: 100%;
}

.msg.system .bubble {
  background: var(--panel);
  color: var(--fg-dim);
}

.thinking {
  padding: 2px 0 2px 8px;
  border-left: 2px solid var(--border-strong);
  color: var(--fg-dim);
  font-size: 11px;
}

.thinking summary {
  cursor: pointer;
  color: var(--fg-faint);
  user-select: none;
}

.thinking pre {
  margin: 4px 0 0;
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.5;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.tool-card {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 6px 8px;
  border: 1px solid var(--border);
  border-left: 2px solid var(--fg-faint);
  border-radius: var(--r-sm);
  background: var(--panel-2);
}

.tool-running {
  border-left-color: var(--accent);
}

.tool-approval {
  border-left-color: var(--yellow);
}

.tool-ok {
  border-left-color: var(--green);
}

.tool-error {
  border-left-color: var(--red);
}

.tool-head {
  display: flex;
  align-items: center;
  gap: 8px;
}

.tool-name {
  color: var(--fg-2);
  font-family: var(--font-mono);
  font-size: 11px;
}

.tool-status {
  margin-left: auto;
  color: var(--fg-faint);
  font-size: 10px;
}

.tool-running .tool-status {
  color: var(--accent);
}

.tool-approval .tool-status {
  color: var(--yellow);
}

.tool-ok .tool-status {
  color: var(--green);
}

.tool-error .tool-status {
  color: var(--red);
}

.tool-result {
  margin: 0;
  color: var(--fg-dim);
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.5;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.link {
  align-self: flex-start;
  padding: 0;
  color: var(--accent);
  font-size: 10px;
}

.link:hover {
  text-decoration: underline;
}

.approval-card {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 8px;
  border: 1px solid var(--yellow);
  border-radius: var(--r-sm);
  background: var(--panel-2);
}

.approval-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.approval-title {
  color: var(--yellow);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.04em;
}

.approval-tool {
  margin-left: auto;
  color: var(--fg-2);
  font-family: var(--font-mono);
  font-size: 11px;
}

.approval-note {
  color: var(--yellow);
  font-size: 10px;
}

.approval-command {
  margin: 0;
  padding: 6px 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--bg);
  color: var(--fg-2);
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.5;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.approval-meta {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 2px 8px;
  margin: 0;
  font-size: 11px;
}

.approval-meta dt {
  color: var(--fg-faint);
}

.approval-meta dd {
  margin: 0;
  color: var(--fg-dim);
  overflow-wrap: anywhere;
}

.approval-actions {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
}

.approval-actions button {
  padding: 3px 10px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-3);
  color: var(--fg-2);
  font-size: 11px;
}

.approval-actions button:hover:not(:disabled) {
  color: var(--fg);
  border-color: var(--accent);
}

.approval-actions button:disabled {
  opacity: 0.5;
  cursor: default;
}

.approval-actions .deny {
  color: var(--red);
  border-color: rgb(217 122 112 / 40%);
}

.resolving {
  color: var(--fg-faint);
  font-size: 10px;
}

.yolo-confirm {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin: 0 8px 8px;
  padding: 8px;
  border: 1px solid var(--yellow);
  border-radius: var(--r-sm);
  background: var(--panel-2);
}

.yolo-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  color: var(--fg-dim);
  font-size: 11px;
  line-height: 1.5;
}

.yolo-text strong {
  color: var(--yellow);
  font-size: 12px;
}

.yolo-actions {
  display: flex;
  justify-content: flex-end;
  gap: 6px;
}

.yolo-actions button {
  padding: 3px 10px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-3);
  color: var(--fg-2);
  font-size: 11px;
}

.yolo-actions button:hover:not(:disabled) {
  color: var(--fg);
  border-color: var(--accent);
}

.yolo-actions button.danger {
  color: var(--yellow);
  border-color: var(--yellow);
}

.yolo-actions button:disabled {
  opacity: 0.5;
  cursor: default;
}

.error-banner {
  display: flex;
  gap: 6px;
  margin: 0 8px 8px;
  padding: 6px 8px;
  border: 1px solid var(--red);
  border-radius: var(--r-sm);
  background: var(--panel-2);
  font-size: 11px;
  overflow-wrap: anywhere;
}

.error-code {
  flex: 0 0 auto;
  color: var(--red);
  font-family: var(--font-mono);
}

.error-message {
  color: var(--fg-dim);
}

.composer {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 8px;
  border-top: 1px solid var(--border);
  background: var(--panel);
}

.composer textarea {
  width: 100%;
  min-height: 52px;
  max-height: 160px;
  padding: 6px 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--bg);
  color: var(--fg);
  font: inherit;
  line-height: 1.5;
  resize: none;
}

.composer textarea:focus {
  border-color: var(--accent);
  outline: none;
}

.composer textarea::placeholder {
  color: var(--fg-faint);
}

.composer-foot {
  display: flex;
  align-items: center;
  gap: 6px;
}

.hint {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  color: var(--fg-faint);
  font-size: 10px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.send {
  padding: 4px 12px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-3);
  color: var(--fg-2);
  font-size: 11px;
}

.send:hover:not(:disabled) {
  color: var(--fg);
  border-color: var(--accent);
}

.send:disabled {
  opacity: 0.45;
  cursor: default;
}
</style>
