<script setup lang="ts">
/* eslint-disable vue/no-v-html -- renderMarkdown 先转义全文，只插入固定标签 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import AppIcon from './AppIcon.vue'
import AgentSessions from './AgentSessions.vue'
import type { ApprovalDecision, ApprovalSource, SessionMode } from '@lc-proto'
import {
  beyondCompressThreshold,
  contextRatio,
  type ToolCard,
} from '../services/agentStore'
import {
  consumeAgentContext,
  drainAgentContexts,
  subscribeAgentContext,
  type AgentCodeContext,
} from '../services/agentContext'
import type { AgentSessionController, TurnStat } from '../services/agentSession'
import { renderMarkdown } from '../services/markdown'
import { subscribePrefs, uiPrefs } from '../services/themeStore'
import { cycleFindIndex, findMessageMatches } from '../services/agentFind'
import { copyText } from '../services/clipboard'
import { forkBeforeUserMessage } from '../services/forkSession'
import {
  buildFileIndex,
  searchFiles,
  type FileIndex,
} from '../services/fileIndex'
import {
  agentPickFiles,
  isTauri,
  providersLoad,
  type AgentAttachment,
  type ProviderView,
} from '../services/bridge'

const props = defineProps<{
  controller: AgentSessionController
  workspace?: string
  showRailToggle?: boolean
  railOpen?: boolean
  showPreviewToggle?: boolean
  previewOpen?: boolean
  /** 全宽 Agent 模式：Ctrl+F 打开会话内查找而不是编辑器查找。 */
  fullMode?: boolean
}>()
const emit = defineEmits<{
  'manage-providers': []
  'toggle-rail': []
  'toggle-preview': []
}>()
const { t, te } = useI18n()

const MODES: SessionMode[] = ['plan', 'chat', 'auto', 'yolo']
const COMPACTION_FLASH_MS = 600
const MAX_ATTACHMENTS = 10
const DEFAULT_THINKING = ['off', 'low', 'medium', 'high']

interface ModelChoice {
  id: string
  label: string
  provider: string
  contextK: number
  compressAtK: number
  thinking: string[]
}

const state = ref(props.controller.getState())
const unsubscribe = props.controller.subscribe(next => {
  state.value = next
})
const prefs = ref(uiPrefs())
const unsubscribePrefs = subscribePrefs(next => {
  prefs.value = next
})

const tauri = isTauri()
const agent = computed(() => state.value.agent)
const modelChoices = ref<ModelChoice[]>([])
const defaultModelId = ref<string | null>(null)
const defaultThinking = ref<string | null>(null)
const providerSummary = ref('')
const providerReady = computed(() => modelChoices.value.length > 0)

function hasProviderKey(provider: ProviderView): boolean {
  return provider.hasApiKey || provider.apiKeyRef.startsWith('env:')
}

function thinkingLevelsFor(modelId: string): string[] {
  const info = modelChoices.value.find(item => item.id === modelId)
  return info && info.thinking.length > 0 ? info.thinking : DEFAULT_THINKING
}

function chosenModelId(): string {
  const configured = state.value.model ?? defaultModelId.value
  if (configured && modelChoices.value.some(item => item.id === configured)) {
    return configured
  }
  return modelChoices.value[0]?.id ?? ''
}

/** 无会话时把模型/思考偏好同步到控制器；已有会话只影响下一次新会话。 */
function syncSelection(): void {
  if (state.value.agent && !state.value.archived) return
  const model = chosenModelId()
  const levels = thinkingLevelsFor(model)
  const preferred = state.value.thinking ?? defaultThinking.value
  const thinking = preferred && levels.includes(preferred) ? preferred : levels[0] ?? null
  const nextModel = model || null
  if (nextModel !== state.value.model || thinking !== state.value.thinking) {
    props.controller.setModel(nextModel, thinking)
  }
}

async function refreshProviderState(): Promise<void> {
  const res = await providersLoad()
  if (!res.ok) {
    modelChoices.value = []
    providerSummary.value = ''
    return
  }
  const enabled = new Map(
    res.data.providers
      .filter(provider => provider.enabled)
      .map(provider => [provider.id, provider]),
  )
  const choices: ModelChoice[] = []
  for (const model of res.data.models) {
    const provider = enabled.get(model.provider)
    if (!provider || !hasProviderKey(provider) || !model.id.trim()) continue
    choices.push({
      id: model.id,
      label: model.id,
      provider: provider.label || provider.id,
      contextK: model.contextK,
      compressAtK: model.compressAtK,
      thinking: model.thinking.length > 0 ? [...model.thinking] : DEFAULT_THINKING,
    })
  }
  modelChoices.value = choices
  defaultModelId.value = res.data.defaultModel
  defaultThinking.value = res.data.defaultThinking
  const first = choices[0]
  providerSummary.value = first
    ? `${first.provider} · ${res.data.defaultModel ?? first.id}`
    : ''
  syncSelection()
}

function onProvidersChanged(): void {
  void refreshProviderState()
}

/** 把编辑器选区作为围栏代码块追加到草稿，保留路径与行号。 */
function appendCodeContext(context: AgentCodeContext): void {
  const header = `${context.path}:${context.startLine}-${context.endLine}`
  const block = `\`\`\`${context.language}\n${context.text}\n\`\`\``
  const draft = state.value.draft.trimEnd()
  props.controller.setDraft(draft ? `${draft}\n\n${header}\n${block}` : `${header}\n${block}`)
}

let offAgentContext: (() => void) | null = null

onMounted(() => {
  void refreshProviderState()
  window.addEventListener('providers:changed', onProvidersChanged)
  window.addEventListener('click', closeMenus)
  window.addEventListener('keydown', onWindowKeydown)
  for (const context of drainAgentContexts()) {
    appendCodeContext(context)
    consumeAgentContext(context)
  }
  offAgentContext = subscribeAgentContext(context => {
    appendCodeContext(context)
    consumeAgentContext(context)
  })
})
const messages = computed(() => agent.value?.messages ?? [])
const tools = computed(() => agent.value?.tools ?? [])
const approvals = computed(() => agent.value?.approvals ?? [])
const compactions = computed(() => agent.value?.compactions ?? [])
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

const findOpen = ref(false)
const findQuery = ref('')
const findIndex = ref(0)
const findInput = ref<HTMLInputElement | null>(null)
const findMatches = computed(() => findMessageMatches(messages.value, findQuery.value))
const findCurrent = computed(() => findMatches.value[findIndex.value] ?? null)

function openFind(): void {
  findOpen.value = true
  void nextTick(() => {
    findInput.value?.focus()
    findInput.value?.select()
  })
}

function closeFind(): void {
  findOpen.value = false
  findQuery.value = ''
  findIndex.value = 0
}

function cycleFind(direction: 1 | -1): void {
  findIndex.value = cycleFindIndex(findIndex.value, findMatches.value.length, direction)
  scrollToFind()
}

function onFindInput(): void {
  findIndex.value = 0
  scrollToFind()
}

function scrollToFind(): void {
  void nextTick(() => {
    const index = findCurrent.value
    if (index === null) return
    streamEl.value
      ?.querySelector<HTMLElement>(`[data-message-index="${index}"]`)
      ?.scrollIntoView({ block: 'center', behavior: 'smooth' })
  })
}

const usedTokens = computed(() => {
  // input + output 是下一次请求要带上的上下文；cached 是 input 子集，不重复计。
  const usage = agent.value?.usage
  return usage ? usage.input + usage.output : 0
})
const ratio = computed(() => (agent.value ? contextRatio(agent.value) : 0))
const contextPct = computed(() => Math.round(ratio.value * 100))
const contextBeyond = computed(() =>
  agent.value ? beyondCompressThreshold(agent.value) : false,
)
const modelLocked = computed(
  () => (Boolean(agent.value) && !state.value.archived) || streaming.value,
)
const selectedModelInfo = computed(
  () => modelChoices.value.find(item => item.id === selectedModel.value) ?? null,
)
const thinkingLevels = computed(() => {
  const info = selectedModelInfo.value
  return info && info.thinking.length > 0 ? info.thinking : []
})

const selectedModel = computed({
  get: () => chosenModelId(),
  set: (value: string) => {
    if (modelLocked.value) return
    const levels = thinkingLevelsFor(value)
    const thinking = levels.includes(state.value.thinking ?? '')
      ? state.value.thinking
      : levels[0] ?? null
    props.controller.setModel(value || null, thinking)
  },
})

const selectedThinking = computed({
  get: () => {
    const levels = thinkingLevels.value
    const current = state.value.thinking
    if (current && levels.includes(current)) return current
    return levels[0] ?? ''
  },
  set: (value: string) => {
    if (modelLocked.value) return
    props.controller.setModel(selectedModel.value || null, value || null)
  },
})

const contextK = computed(
  () => agent.value?.contextK ?? selectedModelInfo.value?.contextK ?? 0,
)
const compressAtK = computed(
  () => agent.value?.compressAtK ?? selectedModelInfo.value?.compressAtK ?? 0,
)
const thresholdPct = computed(() => {
  if (!Number.isFinite(contextK.value) || contextK.value <= 0) return 0
  if (!Number.isFinite(compressAtK.value) || compressAtK.value <= 0) return 0
  return Math.min(100, (compressAtK.value / contextK.value) * 100)
})
const contextThresholdTitle = computed(() =>
  t('agent.contextThreshold', { threshold: compressAtK.value }),
)
const contextBudgetTitle = computed(() =>
  contextK.value > 0
    ? t('agent.contextHint', { k: contextK.value })
    : t('agent.contextHintUnknown'),
)
const contextText = computed(() =>
  contextK.value > 0
    ? `${formatTokens(usedTokens.value)} / ${contextK.value}k`
    : '—',
)
const turnText = computed(() =>
  agent.value ? `${agent.value.turn}/${agent.value.maxTurns}` : '—',
)
const turnStats = computed<TurnStat[]>(() => state.value.turnStats)
const totalTurnInput = computed(() =>
  turnStats.value.reduce((sum, stat) => sum + stat.input, 0),
)
const totalTurnOutput = computed(() =>
  turnStats.value.reduce((sum, stat) => sum + stat.output, 0),
)
const sessionLabel = computed(() => {
  const firstUser = agent.value?.messages.find(message => message.role === 'user')
  const line = firstUser?.text.split('\n')[0]?.trim() ?? ''
  return line ? line.slice(0, 32) : t('agent.noSession')
})
const selectedModelLabel = computed(
  () => selectedModelInfo.value?.label ?? selectedModel.value ?? t('agent.noModels'),
)
/** pill 只显示模型短名（去掉 vendor/ 前缀），完整 id 放在 title 与菜单里。 */
const selectedModelShort = computed(() => {
  const label = selectedModelLabel.value
  const slash = label.lastIndexOf('/')
  return slash >= 0 ? label.slice(slash + 1) : label
})

const statusLabel = computed(() => t(`agent.status.${state.value.status}`))
const statusActive = computed(
  () =>
    state.value.status === 'thinking' ||
    state.value.status === 'generating' ||
    state.value.status === 'working',
)

const historyOpen = ref(false)

function toggleHistory(): void {
  historyOpen.value = !historyOpen.value
}

function closeHistory(): void {
  historyOpen.value = false
}

const openMenu = ref<'mode' | 'model' | 'thinking' | null>(null)
const ctxOpen = ref(false)
const copiedId = ref<string | null>(null)
let copyTimer: ReturnType<typeof setTimeout> | null = null

function toggleMenu(menu: 'mode' | 'model' | 'thinking'): void {
  openMenu.value = openMenu.value === menu ? null : menu
}

function toggleCtx(): void {
  ctxOpen.value = !ctxOpen.value
  openMenu.value = null
}

function closeMenus(): void {
  openMenu.value = null
  ctxOpen.value = false
  closeMention()
}

async function copyMessage(id: string, text: string): Promise<void> {
  if (!copyText(text)) return
  copiedId.value = id
  if (copyTimer !== null) clearTimeout(copyTimer)
  copyTimer = setTimeout(() => {
    copiedId.value = null
    copyTimer = null
  }, 1200)
}

/** 代码块复制按钮在 v-html 内，用事件委托处理，避免为每块绑定监听。 */
function onMarkdownClick(event: MouseEvent): void {
  const target = event.target
  if (!(target instanceof Element)) return
  const button = target.closest<HTMLButtonElement>('[data-md-copy]')
  if (!button) return
  const code = button.closest('.md-code-block')?.querySelector('code')
  if (!code || !copyText(code.textContent ?? '')) return
  const original = button.textContent ?? ''
  button.textContent = t('agent.copied')
  window.setTimeout(() => {
    button.textContent = original
  }, 1200)
}

function forkFromUser(userIndex: number, sendNow: boolean): void {
  if (!agent.value || streaming.value || approvalPending.value || state.value.archived) {
    return
  }
  const fork = forkBeforeUserMessage(state.value, userIndex, props.workspace ?? '')
  if (!fork || !props.controller.loadHistory(fork.session)) return
  if (sendNow) {
    void props.controller.send(fork.prompt)
    return
  }
  props.controller.setDraft(fork.prompt)
  void nextTick(() => {
    const el = composerEl.value
    if (!el) return
    el.focus()
    el.setSelectionRange(fork.prompt.length, fork.prompt.length)
  })
}

function editFromUser(userIndex: number): void {
  forkFromUser(userIndex, false)
}

function retryFromAssistant(messageIndex: number): void {
  for (let index = messageIndex; index >= 0; index -= 1) {
    if (messages.value[index]?.role === 'user') {
      forkFromUser(index, true)
      return
    }
  }
}

function onWindowKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape') {
    closeMenus()
    if (findOpen.value) closeFind()
    return
  }
  const mod = event.ctrlKey || event.metaKey
  if (!mod || event.key.toLowerCase() !== 'f' || !props.fullMode) return
  event.preventDefault()
  openFind()
}

watch(
  () => agent.value?.sessionId,
  () => closeFind(),
)

watch(
  () => findMatches.value.length,
  length => {
    if (findIndex.value >= length) findIndex.value = 0
  },
)

function selectModeFromMenu(mode: SessionMode): void {
  closeMenus()
  selectMode(mode)
}

function chooseModel(id: string): void {
  closeMenus()
  selectedModel.value = id
}

function chooseThinking(level: string): void {
  closeMenus()
  selectedThinking.value = level
}
const composerHint = computed(() => {
  if (!tauri) return t('agent.browserDemo')
  if (streaming.value) return t('agent.queueHint')
  if (approvalPending.value) return t('agent.queueApprovalHint')
  return prefs.value.agentSendKey === 'enter'
    ? t('agent.sendHintEnter')
    : t('agent.sendHintCtrlEnter')
})
const sendDisabled = computed(
  () =>
    !tauri ||
    switching.value ||
    (draft.value.trim().length === 0 && attachments.value.length === 0),
)

const queue = computed(() => state.value.queue)

const attachments = ref<AgentAttachment[]>([])
const attachmentError = ref('')
const composerEl = ref<HTMLTextAreaElement | null>(null)

const mentionOpen = ref(false)
const mentionQuery = ref('')
const mentionItems = ref<string[]>([])
const mentionIndex = ref(0)
const mentionLoading = ref(false)
const mentionError = ref('')
const mentionTruncated = ref(false)
const MENTION_LIMIT = 8
let mentionStart = 0
let mentionEnd = 0
let mentionToken = 0
let mentionCache: FileIndex | null = null
let mentionCacheRoot = ''

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
    current?.compactions.length ?? 0,
    state.value.turnStats.length,
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

/** 载入历史后按当前供应商配置校验模型；已删除的模型回退到默认。 */
watch(
  () => state.value.archived,
  archived => {
    if (archived) syncSelection()
  },
)

watch(
  () => props.workspace,
  () => {
    mentionCache = null
    mentionCacheRoot = ''
    closeMention()
  },
)

/** 每次压缩让用量条闪一次；600ms 后移除 class（组件卸载时清理计时器）。 */
const compactionFlash = ref(false)
let flashTimer: ReturnType<typeof setTimeout> | null = null
watch(
  () => agent.value?.compactionPulse ?? 0,
  (pulse, previous) => {
    if (pulse <= previous) return
    compactionFlash.value = true
    if (flashTimer !== null) clearTimeout(flashTimer)
    flashTimer = setTimeout(() => {
      compactionFlash.value = false
      flashTimer = null
    }, COMPACTION_FLASH_MS)
  },
)
onBeforeUnmount(() => {
  unsubscribe()
  unsubscribePrefs()
  window.removeEventListener('providers:changed', onProvidersChanged)
  window.removeEventListener('click', closeMenus)
  window.removeEventListener('keydown', onWindowKeydown)
  offAgentContext?.()
  offAgentContext = null
  if (flashTimer !== null) clearTimeout(flashTimer)
  if (copyTimer !== null) clearTimeout(copyTimer)
})

function formatTokens(tokens: number): string {
  if (!Number.isFinite(tokens)) return '—'
  return tokens >= 1000 ? `${(tokens / 1000).toFixed(1)}k` : String(Math.round(tokens))
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B'
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`
  return `${bytes} B`
}

function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return '—'
  if (ms < 1000) return `${Math.round(ms)}ms`
  return `${(ms / 1000).toFixed(1)}s`
}

function thinkingLabel(level: string): string {
  const key = `agent.thinkingLevels.${level}`
  return te(key) ? t(key) : level
}

const TOOL_ICONS: Record<string, string> = {
  read_file: 'file',
  write_file: 'pencil',
  edit_file: 'pencil',
  glob: 'search',
  grep: 'search',
  shell: 'terminal',
}

function toolIcon(name: string): string {
  if (name.startsWith('db_')) return 'db'
  return TOOL_ICONS[name] ?? 'smart'
}

/** 工具头部的单行摘要；参数/输出只取第一行，由 CSS 截断。 */
function toolArg(tool: ToolCard): string {
  const line = (tool.text ?? '').split('\n').find(item => item.trim())
  return line?.trim().slice(0, 120) ?? ''
}

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
  const files = [...attachments.value]
  await props.controller.send(undefined, files)
  if (!props.controller.getState().error) {
    attachments.value = []
    attachmentError.value = ''
  }
}

function removeQueued(id: number): void {
  props.controller.removeQueued(id)
}

function editQueued(id: number): void {
  const item = state.value.queue.find(entry => entry.id === id)
  if (!item) return
  props.controller.removeQueued(id)
  props.controller.setDraft(item.text)
  attachments.value = [...item.attachments]
  attachmentError.value = ''
}

function clearQueue(): void {
  props.controller.clearQueue()
}

function closeMention(): void {
  mentionToken++
  mentionOpen.value = false
  mentionQuery.value = ''
  mentionItems.value = []
  mentionError.value = ''
  mentionIndex.value = 0
}

async function refreshMention(query: string): Promise<void> {
  if (!props.workspace) {
    mentionItems.value = []
    mentionError.value = t('agent.mentionNoWorkspace')
    mentionLoading.value = false
    return
  }
  const token = ++mentionToken
  if (!mentionCache || mentionCacheRoot !== props.workspace) {
    mentionLoading.value = true
    const cache = await buildFileIndex(props.workspace)
    if (token !== mentionToken) return
    mentionCache = cache
    mentionCacheRoot = props.workspace
    mentionLoading.value = false
  }
  if (token !== mentionToken) return
  mentionError.value = ''
  mentionTruncated.value = mentionCache?.truncated ?? false
  mentionItems.value = mentionCache
    ? searchFiles(mentionCache, query, MENTION_LIMIT)
    : []
  mentionIndex.value = 0
}

function onDraftInput(event: Event): void {
  const el = event.target as HTMLTextAreaElement
  const cursor = el.selectionStart ?? el.value.length
  const before = el.value.slice(0, cursor)
  const at = before.lastIndexOf('@')
  if (at < 0) {
    closeMention()
    return
  }
  const previous = at > 0 ? before[at - 1]! : ''
  if (previous && !/[\s([{"']/.test(previous)) {
    closeMention()
    return
  }
  const query = before.slice(at + 1)
  if (/\s/.test(query)) {
    closeMention()
    return
  }
  mentionStart = at
  mentionEnd = cursor
  mentionQuery.value = query
  mentionOpen.value = true
  mentionIndex.value = 0
  void refreshMention(query)
}

function chooseMention(file: string): void {
  if (!mentionOpen.value) return
  const current = composerEl.value?.value ?? state.value.draft
  const insert = `@${file} `
  props.controller.setDraft(
    current.slice(0, mentionStart) + insert + current.slice(mentionEnd),
  )
  const position = mentionStart + insert.length
  closeMention()
  void nextTick(() => {
    const el = composerEl.value
    if (!el) return
    el.focus()
    el.setSelectionRange(position, position)
  })
}

function onComposerKeydown(event: KeyboardEvent): void {
  if (!mentionOpen.value) {
    if (event.key === 'Enter' && prefs.value.agentSendKey === 'enter') {
      event.preventDefault()
      void submit()
    }
    return
  }
  if (event.key === 'ArrowDown') {
    event.preventDefault()
    if (mentionItems.value.length > 0) {
      mentionIndex.value = (mentionIndex.value + 1) % mentionItems.value.length
    }
  } else if (event.key === 'ArrowUp') {
    event.preventDefault()
    if (mentionItems.value.length > 0) {
      mentionIndex.value =
        (mentionIndex.value - 1 + mentionItems.value.length) %
        mentionItems.value.length
    }
  } else if (event.key === 'Enter' && !event.ctrlKey && !event.metaKey) {
    if (mentionItems.value.length === 0) return
    event.preventDefault()
    const file = mentionItems.value[mentionIndex.value]
    if (file) chooseMention(file)
  } else if (event.key === 'Escape') {
    event.preventDefault()
    closeMention()
  }
}

async function pickFiles(): Promise<void> {
  if (!tauri) return
  attachmentError.value = ''
  const res = await agentPickFiles()
  if (!res.ok) {
    attachmentError.value = res.error.message
    return
  }
  if (res.data.length === 0) return
  const byPath = new Map(attachments.value.map(file => [file.path, file]))
  for (const file of res.data) {
    if (byPath.size >= MAX_ATTACHMENTS && !byPath.has(file.path)) {
      attachmentError.value = t('agent.attachmentLimit', { max: MAX_ATTACHMENTS })
      break
    }
    byPath.set(file.path, file)
  }
  attachments.value = [...byPath.values()]
}

function removeAttachment(path: string): void {
  attachments.value = attachments.value.filter(file => file.path !== path)
  if (attachments.value.length === 0) attachmentError.value = ''
}

function newSession(): void {
  if (streaming.value || approvalPending.value) return
  if (props.controller.resetSession()) {
    attachments.value = []
    attachmentError.value = ''
  }
}

function cancel(): void {
  void props.controller.cancel()
}

function decide(approvalId: number, decision: ApprovalDecision): void {
  void props.controller.resolve(approvalId, decision)
}
</script>

<template>
  <div
    class="agent-body"
    :class="`density-${prefs.agentDensity}`"
    :style="{ '--agent-font': `${prefs.agentFontSize}px` }"
  >
    <header class="agent-head">
      <span class="agent-title">Agent</span>
      <span
        class="sess-dot"
        :class="{ running: streaming }"
      />
      <span
        class="sess-name"
        :title="sessionLabel"
      >{{ sessionLabel }}</span>
      <span class="spacer" />
      <span
        v-if="agent"
        class="status-text"
        :class="[state.status, { active: statusActive }]"
      >{{ statusLabel }}</span>
      <span
        v-if="agent"
        class="turn"
        :title="t('agent.turn')"
      >{{ turnText }}</span>
      <button
        v-if="props.showRailToggle"
        type="button"
        class="icon-btn"
        :class="{ active: props.railOpen }"
        :title="t('activity.git')"
        :aria-label="t('activity.git')"
        @click="emit('toggle-rail')"
      >
        <AppIcon
          name="git"
          :size="15"
        />
      </button>
      <button
        v-if="props.showPreviewToggle"
        type="button"
        class="icon-btn"
        :class="{ active: props.previewOpen }"
        :title="t('agent.previewTitle')"
        :aria-label="t('agent.previewTitle')"
        @click="emit('toggle-preview')"
      >
        <AppIcon
          name="preview"
          :size="15"
        />
      </button>
      <button
        type="button"
        class="icon-btn"
        :class="{ active: historyOpen }"
        :title="t('agent.history')"
        :aria-label="t('agent.history')"
        @click="toggleHistory"
      >
        <AppIcon
          name="history"
          :size="15"
        />
      </button>
      <button
        v-if="streaming"
        type="button"
        class="icon-btn"
        :title="t('agent.stop')"
        :aria-label="t('agent.stop')"
        @click="cancel"
      >
        <AppIcon
          name="square"
          :size="13"
        />
      </button>
      <button
        type="button"
        class="icon-btn"
        :title="t('agent.newSession')"
        :aria-label="t('agent.newSession')"
        :disabled="!agent || streaming || approvalPending"
        @click="newSession"
      >
        <AppIcon
          name="plus"
          :size="15"
        />
      </button>
    </header>

    <div
      v-if="historyOpen"
      class="history-panel"
    >
      <AgentSessions
        :controller="props.controller"
        :workspace="props.workspace"
        closable
        @select="closeHistory"
        @new-session="closeHistory"
        @close="closeHistory"
      />
    </div>

    <div
      v-if="findOpen"
      class="agent-find"
    >
      <input
        ref="findInput"
        v-model="findQuery"
        type="text"
        :placeholder="t('agent.findPlaceholder')"
        @input="onFindInput"
        @keydown.enter.exact.prevent="cycleFind(1)"
        @keydown.shift.enter.prevent="cycleFind(-1)"
        @keydown.esc.prevent="closeFind"
      >
      <span class="agent-find-count">
        {{ findMatches.length ? `${findIndex + 1}/${findMatches.length}` : t('agent.findNone') }}
      </span>
      <button
        type="button"
        class="icon-btn"
        :title="t('agent.findPrev')"
        :aria-label="t('agent.findPrev')"
        :disabled="findMatches.length === 0"
        @click="cycleFind(-1)"
      >
        <AppIcon
          name="chevron-down"
          :size="13"
          class="up"
        />
      </button>
      <button
        type="button"
        class="icon-btn"
        :title="t('agent.findNext')"
        :aria-label="t('agent.findNext')"
        :disabled="findMatches.length === 0"
        @click="cycleFind(1)"
      >
        <AppIcon
          name="chevron-down"
          :size="13"
        />
      </button>
      <button
        type="button"
        class="icon-btn"
        :title="t('agent.findClose')"
        :aria-label="t('agent.findClose')"
        @click="closeFind"
      >
        <AppIcon
          name="x"
          :size="13"
        />
      </button>
    </div>

    <div
      ref="streamEl"
      class="stream"
    >
      <div
        v-if="!agent || (!streaming && messages.length === 0 && tools.length === 0 && approvals.length === 0)"
        class="empty"
      >
        <h3>{{ providerReady ? t('agent.readyTitle') : t('agent.emptyTitle') }}</h3>
        <p>{{ providerReady ? t('agent.readyDesc', { summary: providerSummary }) : t('agent.emptyDesc') }}</p>
        <button
          type="button"
          class="manage-providers"
          @click="emit('manage-providers')"
        >
          {{ t('agent.manageProviders') }}
        </button>
        <p
          v-if="!tauri"
          class="demo"
        >
          {{ t('agent.browserDemo') }}
        </p>
      </div>

      <div
        v-for="(message, index) in messages"
        :key="message.id"
        class="msg"
        :class="[message.role, { 'find-hit': findCurrent === index }]"
        :data-message-index="index"
      >
        <details
          v-if="message.thinking"
          class="thinking"
        >
          <summary>{{ t('agent.thinking') }}</summary>
          <pre>{{ message.thinking }}</pre>
        </details>
        <div
          v-if="message.text && message.role !== 'assistant'"
          class="msg-text"
        >
          {{ message.text }}
        </div>
        <!-- eslint-disable-next-line vue/no-v-html -- renderMarkdown 已先转义全文 -->
        <div
          v-else-if="message.text"
          class="msg-text markdown"
          @click="onMarkdownClick"
          v-html="renderMarkdown(message.text, { copyLabel: t('agent.copy') })"
        />
        <div
          v-if="message.role === 'assistant' && message.text"
          class="msg-actions"
        >
          <button
            type="button"
            @click.stop="copyMessage(message.id, message.text)"
          >
            {{ copiedId === message.id ? t('agent.copied') : t('agent.copy') }}
          </button>
          <button
            v-if="!state.archived && !streaming && !approvalPending"
            type="button"
            :title="t('agent.forkHint')"
            @click.stop="retryFromAssistant(index)"
          >
            {{ t('agent.forkRetry') }}
          </button>
        </div>
        <div
          v-if="message.role === 'user' && message.text && !state.archived && !streaming && !approvalPending"
          class="msg-actions user-actions"
        >
          <button
            type="button"
            :title="t('agent.forkHint')"
            @click.stop="editFromUser(index)"
          >
            {{ t('agent.forkEdit') }}
          </button>
        </div>
      </div>

      <details
        v-for="record in compactions"
        :key="record.recordId"
        class="compact"
      >
        <summary class="compact-row">
          <AppIcon
            name="layers"
            :size="13"
          />
          <span>{{ t('agent.compactedCount', { count: record.removedMessages }) }}</span>
          <span class="r">{{ formatTokens(record.beforeTokens) }} → {{ formatTokens(record.afterTokens) }}</span>
          <AppIcon
            name="chevron-down"
            :size="12"
            class="chev"
          />
        </summary>
        <div class="compact-body">
          {{ record.summary }}
        </div>
      </details>

      <div
        v-for="tool in tools"
        :key="tool.callId"
        class="tool"
        :class="[`tool-${tool.status}`, { open: isToolExpanded(tool.callId) }]"
      >
        <button
          type="button"
          class="tool-head"
          :aria-expanded="isToolExpanded(tool.callId)"
          @click="toggleTool(tool.callId)"
        >
          <AppIcon
            :name="toolIcon(tool.name)"
            :size="13"
            class="tool-ico"
          />
          <span class="tool-name">{{ tool.name }}</span>
          <span class="tool-arg">{{ toolArg(tool) }}</span>
          <span class="tool-dur">{{ toolStatusLabel(tool.status) }}</span>
          <AppIcon
            name="chevron-down"
            :size="12"
            class="chev"
          />
        </button>
        <pre
          v-if="tool.text"
          class="tool-body"
        >{{ tool.text }}</pre>
      </div>

      <div
        v-for="approval in approvals"
        :key="approval.approvalId"
        class="perm"
      >
        <div class="perm-title">
          <AppIcon
            name="shield"
            :size="14"
          />
          <span>{{ t('agent.approval.title') }}</span>
          <span class="perm-kind">{{ approval.tool }}</span>
        </div>
        <pre
          v-if="approval.command"
          class="perm-cmd"
        >{{ approval.command }}</pre>
        <div class="perm-meta">
          {{ sourceLabel(approval.source) }} · {{ approval.reason }}
          <template v-if="state.mode === 'yolo'">
            · {{ t('agent.approval.yoloNotice') }}
          </template>
        </div>
        <div class="perm-actions">
          <button
            type="button"
            class="btn"
            :disabled="isResolving(approval.approvalId)"
            @click="decide(approval.approvalId, 'deny')"
          >
            {{ t('agent.approval.deny') }}
          </button>
          <button
            type="button"
            class="btn"
            :disabled="isResolving(approval.approvalId)"
            @click="decide(approval.approvalId, 'allow_session')"
          >
            {{ t('agent.approval.allowSession') }}
          </button>
          <button
            type="button"
            class="btn primary"
            :disabled="isResolving(approval.approvalId)"
            @click="decide(approval.approvalId, 'allow_once')"
          >
            {{ t('agent.approval.allowOnce') }}
          </button>
        </div>
        <div
          v-if="isResolving(approval.approvalId)"
          class="resolving"
        >
          {{ t('agent.approval.resolving') }}
        </div>
      </div>

      <div
        v-if="turnStats.length > 0"
        class="turn-stats"
      >
        <div class="turn-stats-head">
          {{ t('agent.turnUsage') }}
        </div>
        <div
          v-for="stat in turnStats"
          :key="stat.turn"
          class="turn-stat"
        >
          <span class="stat-turn">#{{ stat.turn }}</span>
          <span
            class="stat-in"
            :title="t('agent.inputTokens')"
          >{{ t('agent.statIn') }} {{ formatTokens(stat.input) }}</span>
          <span
            class="stat-out"
            :title="t('agent.outputTokens')"
          >{{ t('agent.statOut') }} {{ formatTokens(stat.output) }}</span>
          <span
            v-if="stat.cached > 0"
            class="stat-cache"
            :title="t('agent.cachedTokens')"
          >⚡ {{ formatTokens(stat.cached) }}</span>
          <span
            class="stat-time"
            :title="t('agent.duration')"
          >{{ formatDuration(stat.durationMs) }}</span>
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
      <div class="perm-actions">
        <button
          type="button"
          class="btn"
          :disabled="switching"
          @click="pendingYolo = false"
        >
          {{ t('agent.yolo.cancel') }}
        </button>
        <button
          type="button"
          class="btn primary"
          :disabled="switching"
          @click="confirmYolo"
        >
          {{ t('agent.yolo.confirm') }}
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
      <span
        v-if="/40[13]/.test(state.error.message)"
        class="error-hint"
      >{{ t('agent.authHint') }}</span>
    </div>

    <div
      v-if="state.archived"
      class="archive-banner"
    >
      <AppIcon
        name="history"
        :size="13"
      />
      <span>{{ t('agent.archivedHint') }}</span>
    </div>

    <div
      v-if="queue.length > 0"
      class="queue"
    >
      <div class="queue-head">
        <span>{{ t('agent.queueTitle', { count: queue.length }) }}</span>
        <button
          type="button"
          @click="clearQueue"
        >
          {{ t('agent.queueClear') }}
        </button>
      </div>
      <div
        v-for="item in queue"
        :key="item.id"
        class="queue-item"
      >
        <button
          type="button"
          class="queue-text"
          :title="t('agent.queueEdit')"
          @click="editQueued(item.id)"
        >
          {{ item.display }}
        </button>
        <button
          type="button"
          class="queue-remove"
          :title="t('agent.queueRemove')"
          :aria-label="t('agent.queueRemove')"
          @click="removeQueued(item.id)"
        >
          <AppIcon
            name="x"
            :size="11"
          />
        </button>
      </div>
    </div>

    <div class="composer">
      <div
        v-if="mentionOpen"
        class="mention-pop"
        @click.stop
      >
        <div class="mention-head">
          {{ t('agent.mentionTitle') }}
          <span v-if="mentionQuery">@{{ mentionQuery }}</span>
        </div>
        <p
          v-if="mentionLoading"
          class="mention-note"
        >
          {{ t('agent.mentionLoading') }}
        </p>
        <p
          v-else-if="mentionError"
          class="mention-note error"
        >
          {{ mentionError }}
        </p>
        <p
          v-else-if="mentionItems.length === 0"
          class="mention-note"
        >
          {{ t('agent.mentionEmpty') }}
        </p>
        <button
          v-for="(file, index) in mentionItems"
          :key="file"
          type="button"
          class="mention-item"
          :class="{ active: index === mentionIndex }"
          @mousedown.prevent.stop="chooseMention(file)"
          @mouseenter="mentionIndex = index"
        >
          <AppIcon
            name="file"
            :size="12"
          />
          <span class="mention-path">{{ file }}</span>
        </button>
        <p
          v-if="mentionTruncated"
          class="mention-note"
        >
          {{ t('agent.mentionTruncated') }}
        </p>
      </div>
      <div class="comp-box">
        <div
          v-if="attachments.length > 0 || attachmentError"
          class="attach-row"
        >
          <span
            v-for="file in attachments"
            :key="file.path"
            class="attachment-chip"
          >
            <span
              class="chip-name"
              :title="file.path"
            >{{ file.name }}</span>
            <span class="chip-size">{{ formatBytes(file.bytes) }}</span>
            <button
              type="button"
              class="chip-remove"
              :aria-label="t('agent.removeAttachment')"
              @click="removeAttachment(file.path)"
            >
              <AppIcon
                name="x"
                :size="11"
              />
            </button>
          </span>
          <p
            v-if="attachmentError"
            class="attachment-error"
          >
            {{ attachmentError }}
          </p>
        </div>
        <textarea
          ref="composerEl"
          v-model="draft"
          rows="1"
          spellcheck="false"
          :placeholder="t('agent.composerPlaceholder')"
          @input="onDraftInput"
          @keydown.exact="onComposerKeydown"
          @keydown.ctrl.enter.prevent="submit"
          @keydown.meta.enter.prevent="submit"
        />
        <div class="comp-ctrl">
          <span class="pill-wrap">
            <button
              type="button"
              class="pill"
              :class="`mode-${state.mode}`"
              :disabled="switchDisabled"
              :title="t('agent.modeLabel')"
              @click.stop="toggleMenu('mode')"
            >
              <AppIcon
                name="smart"
                :size="13"
              />
              <span class="pill-label">{{ t(`agent.modes.${state.mode}`) }}</span>
              <AppIcon
                name="chevron-down"
                :size="11"
                class="chev"
              />
            </button>
            <div
              v-if="openMenu === 'mode'"
              class="pill-menu"
            >
              <button
                v-for="mode in MODES"
                :key="mode"
                type="button"
                class="pill-item"
                :class="{ active: state.mode === mode, yolo: mode === 'yolo' }"
                @click="selectModeFromMenu(mode)"
              >
                {{ t(`agent.modes.${mode}`) }}
              </button>
            </div>
          </span>

          <span class="pill-wrap">
            <button
              type="button"
              class="pill"
              :disabled="modelLocked || modelChoices.length === 0"
              :title="modelLocked ? t('agent.modelLocked') : selectedModelLabel"
              @click.stop="toggleMenu('model')"
            >
              <span class="pill-label">{{ selectedModelShort }}</span>
              <AppIcon
                name="chevron-down"
                :size="11"
                class="chev"
              />
            </button>
            <div
              v-if="openMenu === 'model'"
              class="pill-menu model-menu"
            >
              <button
                v-for="choice in modelChoices"
                :key="choice.id"
                type="button"
                class="pill-item"
                :class="{ active: choice.id === selectedModel }"
                @click="chooseModel(choice.id)"
              >
                <span class="item-main">{{ choice.label }}</span>
                <span class="item-sub">{{ choice.provider }} · {{ choice.contextK }}k</span>
              </button>
              <p
                v-if="modelChoices.length === 0"
                class="pill-empty"
              >
                {{ t('agent.noModels') }}
              </p>
            </div>
          </span>

          <span class="pill-wrap">
            <button
              type="button"
              class="pill"
              :disabled="modelLocked || thinkingLevels.length === 0"
              :title="modelLocked ? t('agent.modelLocked') : t('agent.reasoningEffort')"
              @click.stop="toggleMenu('thinking')"
            >
              <span class="pill-label">{{ t('agent.thinkingShort') }} {{ thinkingLabel(selectedThinking) }}</span>
              <AppIcon
                name="chevron-down"
                :size="11"
                class="chev"
              />
            </button>
            <div
              v-if="openMenu === 'thinking'"
              class="pill-menu"
            >
              <button
                v-for="level in thinkingLevels"
                :key="level"
                type="button"
                class="pill-item"
                :class="{ active: level === selectedThinking }"
                @click="chooseThinking(level)"
              >
                {{ thinkingLabel(level) }}
              </button>
            </div>
          </span>

          <span class="spacer" />

          <button
            type="button"
            class="icon-btn attach-btn"
            :disabled="!tauri"
            :title="t('agent.addAttachment')"
            :aria-label="t('agent.addAttachment')"
            @click="pickFiles"
          >
            <AppIcon
              name="paperclip"
              :size="15"
            />
            <span
              v-if="attachments.length > 0"
              class="attach-count"
            >{{ attachments.length }}</span>
          </button>
          <button
            type="button"
            class="send-btn"
            :disabled="sendDisabled"
            :title="composerHint"
            :aria-label="t('agent.send')"
            @click="submit"
          >
            <AppIcon
              name="arrow-up"
              :size="15"
            />
          </button>
        </div>
      </div>

      <div class="ctx">
        <div
          class="ctx-top"
          role="button"
          tabindex="0"
          :aria-expanded="ctxOpen"
          :title="t('agent.contextDetails')"
          @click.stop="toggleCtx"
          @keydown.enter.prevent="toggleCtx"
        >
          <span class="ctx-label">{{ t('agent.contextLabel') }}</span>
          <div
            class="ctx-bar"
            role="progressbar"
            :aria-label="t('agent.contextLabel')"
            aria-valuemin="0"
            :aria-valuemax="contextK"
            :aria-valuenow="usedTokens"
            :title="contextBudgetTitle"
          >
            <div
              class="ctx-fill"
              :class="{ warn: contextBeyond, flash: compactionFlash }"
              :style="{ width: `${contextPct}%` }"
            />
            <div
              v-if="thresholdPct > 0"
              class="ctx-tick"
              :style="{ left: `${thresholdPct}%` }"
              :title="contextThresholdTitle"
            />
          </div>
        </div>
        <div class="ctx-meta">
          <span class="ctx-val">{{ contextText }}</span>
          <span v-if="compressAtK > 0">{{ contextThresholdTitle }}</span>
        </div>

        <div
          v-if="ctxOpen"
          class="ctx-pop"
          @click.stop
        >
          <div class="ctx-pop-title">
            {{ t('agent.contextDetails') }}
          </div>
          <div class="ctx-pop-row">
            <span>{{ t('agent.contextUsed') }}</span>
            <strong>{{ formatTokens(usedTokens) }} / {{ contextK }}k</strong>
          </div>
          <div class="ctx-pop-row">
            <span>{{ t('agent.compressThreshold') }}</span>
            <strong>{{ compressAtK }}k</strong>
          </div>
          <div class="ctx-pop-row">
            <span>{{ t('agent.lastRequest') }}</span>
            <strong>↑ {{ formatTokens(agent?.usage.input ?? 0) }} · ↓ {{ formatTokens(agent?.usage.output ?? 0) }}</strong>
          </div>
          <div class="ctx-pop-row">
            <span>{{ t('agent.cachedTokens') }}</span>
            <strong>{{ formatTokens(agent?.usage.cached ?? 0) }}</strong>
          </div>
          <div class="ctx-pop-row">
            <span>{{ t('agent.totalTurnTokens') }}</span>
            <strong>↑ {{ formatTokens(totalTurnInput) }} · ↓ {{ formatTokens(totalTurnOutput) }}</strong>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.agent-body {
  position: relative;
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--panel);
}

/* 会话头 */
.agent-head {
  flex: none;
  height: 36px;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 8px 0 12px;
  border-bottom: 1px solid var(--border);
}

.agent-title {
  color: var(--fg-dim);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.07em;
  text-transform: uppercase;
}

.sess-dot {
  flex: none;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--fg-faint);
}

.sess-dot.running {
  background: var(--accent);
  box-shadow: 0 0 0 3px var(--accent-soft);
  animation: dot-pulse 1.6s infinite;
}

@keyframes dot-pulse {
  50% {
    opacity: 0.45;
  }
}

.sess-name {
  min-width: 0;
  overflow: hidden;
  color: var(--fg-2);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.spacer {
  flex: 1;
}

.turn {
  flex: none;
  color: var(--fg-faint);
  font: 10.5px var(--font-mono);
  white-space: nowrap;
}

.status-text {
  flex: none;
  color: var(--fg-faint);
  font-size: 10.5px;
  white-space: nowrap;
}

.status-text.thinking,
.status-text.generating,
.status-text.working {
  color: var(--accent);
}

.status-text.thinking.active,
.status-text.generating.active,
.status-text.working.active {
  animation: status-pulse 1.4s ease-in-out infinite;
}

.status-text.waiting {
  color: var(--yellow);
}

.status-text.done {
  color: var(--green);
}

.status-text.error {
  color: var(--red);
}

.status-text.archived {
  color: var(--fg-faint);
}

@keyframes status-pulse {
  50% {
    opacity: 0.45;
  }
}

.history-panel {
  position: absolute;
  top: 36px;
  left: 0;
  right: 0;
  bottom: 0;
  z-index: 30;
  border-top: 1px solid var(--border);
  background: var(--panel);
}

.icon-btn.active {
  background: var(--panel-3);
  color: var(--accent);
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

.icon-btn:hover:not(:disabled) {
  background: var(--panel-3);
  color: var(--fg);
}

.icon-btn:disabled {
  opacity: 0.4;
  cursor: default;
}

/* 消息流 */
.agent-find {
  flex: none;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 12px;
  border-bottom: 1px solid var(--border);
  background: var(--panel-2);
}

.agent-find input {
  flex: 1;
  min-width: 0;
  height: 26px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--bg);
  color: var(--fg);
  font-size: 12px;
}

.agent-find input:focus {
  border-color: var(--accent-line);
}

.agent-find-count {
  min-width: 42px;
  text-align: center;
  color: var(--fg-dim);
  font-size: 11px;
}

.agent-find .icon-btn {
  width: 24px;
  height: 24px;
}

.agent-find .up {
  transform: rotate(180deg);
}

.msg.find-hit {
  outline: 1px solid var(--accent-line);
  outline-offset: 4px;
  border-radius: var(--r-sm);
}

.stream {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 12px;
}

/* 消息/工具/审批卡不参与 flex 收缩，否则条目一多会被压成细条而不是滚动。 */
.stream > * {
  flex: none;
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
  font-size: 11.5px;
  line-height: 1.6;
}

.empty .demo {
  margin-top: 8px;
  color: var(--yellow);
}

.manage-providers {
  margin-top: 10px;
  height: 28px;
  padding: 0 12px;
  border: 1px solid var(--accent);
  border-radius: var(--r-sm);
  background: var(--accent-soft);
  color: var(--accent);
  font-size: 11.5px;
}

.manage-providers:hover {
  background: var(--accent);
  color: var(--accent-fg);
}

.msg {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.msg.user {
  align-items: flex-end;
}

.msg-text {
  color: var(--fg);
  font-size: var(--agent-font, 12.5px);
  line-height: 1.65;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.msg.user .msg-text {
  max-width: 86%;
  padding: 8px 10px;
  border: 1px solid var(--accent-line);
  border-radius: var(--r);
  background: color-mix(in srgb, var(--accent) 10%, var(--panel-2));
}

.msg.assistant .msg-text {
  font-size: var(--agent-font, 13px);
}

.msg-text.markdown {
  white-space: normal;
}

.msg-text.markdown p {
  margin: 0 0 8px;
}

.msg-text.markdown p:last-child {
  margin-bottom: 0;
}

.msg-text.markdown ul {
  margin: 0 0 8px;
  padding-left: 17px;
}

.msg-text.markdown li {
  margin: 2px 0;
}

.msg-text.markdown code {
  padding: 0 4px;
  border: 1px solid var(--border);
  border-radius: 3px;
  background: var(--panel-2);
  color: var(--fg-2);
  font: 11.5px var(--font-mono);
}

.msg-text.markdown .md-h {
  color: var(--fg);
  font-weight: 600;
}

.msg-text.markdown .md-code-block {
  margin: 8px 0;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--bg);
  overflow: hidden;
}

.msg-text.markdown .md-code-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 3px 8px;
  border-bottom: 1px solid var(--border);
  color: var(--fg-dim);
  font: 11px/1.6 var(--font-mono);
  user-select: none;
}

.msg-text.markdown .md-code-copy {
  padding: 0 2px;
  border: 0;
  background: transparent;
  color: var(--fg-dim);
  font: inherit;
  cursor: pointer;
}

.msg-text.markdown .md-code-copy:hover {
  color: var(--fg);
}

.msg-text.markdown pre.md-code {
  margin: 0;
  padding: 8px 10px;
  border: 0;
  border-radius: 0;
  background: transparent;
  overflow: auto;
}

.msg-text.markdown pre.md-code code {
  padding: 0;
  border: 0;
  background: transparent;
  color: var(--fg-dim);
  font: 11.5px/1.6 var(--font-mono);
  white-space: pre;
}

.msg.system .msg-text {
  color: var(--fg-dim);
  font-size: 11.5px;
}

.msg-actions {
  display: flex;
  gap: 4px;
  opacity: 0;
  transition: opacity var(--dur) var(--ease);
}

.msg:hover .msg-actions {
  opacity: 1;
}

.msg-actions button {
  padding: 2px 6px;
  border-radius: var(--r-xs, 4px);
  color: var(--fg-faint);
  font-size: 10.5px;
}

.msg-actions button:hover {
  background: var(--panel-2);
  color: var(--fg-dim);
}

.thinking {
  padding-left: 9px;
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
  font: 11px/1.55 var(--font-mono);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

/* 工具卡 */
.tool {
  border: 1px solid var(--border);
  border-radius: var(--r);
  background: var(--panel-2);
  overflow: hidden;
  transition: border-color var(--dur) var(--ease);
}

.tool:hover {
  border-color: var(--border-strong);
}

.tool-head {
  display: flex;
  align-items: center;
  gap: 7px;
  width: 100%;
  height: 31px;
  padding: 0 9px;
  color: var(--fg-2);
  text-align: left;
}

.tool-head:hover {
  background: var(--panel-3);
}

.tool-ico {
  flex: none;
  color: var(--fg-faint);
}

.tool-name {
  flex: none;
  color: var(--accent);
  font: 11.5px var(--font-mono);
}

.tool-arg {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  color: var(--fg-dim);
  font: 11.5px var(--font-mono);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tool-dur {
  flex: none;
  color: var(--fg-faint);
  font: 10.5px var(--font-mono);
}

.tool .chev {
  flex: none;
  color: var(--fg-faint);
  transition: transform 120ms var(--ease);
}

.tool.open .chev {
  transform: rotate(180deg);
}

.tool-running .tool-name {
  color: var(--accent);
}

.tool-approval .tool-name {
  color: var(--yellow);
}

.tool-error .tool-name {
  color: var(--red);
}

.tool-body {
  margin: 0;
  padding: 0 10px 8px;
  color: var(--fg-dim);
  font: 11.5px/1.6 var(--font-mono);
  white-space: pre;
  overflow: auto;
  max-height: calc(5 * 1.6em + 4px);
  mask-image: linear-gradient(#000 72%, transparent);
}

.tool.open .tool-body {
  max-height: none;
  mask-image: none;
}

/* 压缩记录 */
.compact {
  border: 1px solid var(--border);
  border-radius: var(--r);
  background: transparent;
  overflow: hidden;
}

.compact-row {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  height: 30px;
  padding: 0 9px;
  color: var(--fg-dim);
  font-size: 11.5px;
  text-align: left;
  list-style: none;
  cursor: pointer;
}

.compact-row::-webkit-details-marker {
  display: none;
}

.compact-row:hover {
  background: var(--panel-2);
}

.compact-row .r {
  margin-left: auto;
  color: var(--fg-faint);
  font: 10.5px var(--font-mono);
}

.compact-row .chev {
  color: var(--fg-faint);
  transition: transform 120ms var(--ease);
}

.compact[open] .compact-row .chev {
  transform: rotate(180deg);
}

.compact-body {
  padding: 0 10px 8px 30px;
  color: var(--fg-faint);
  font-size: 11px;
  line-height: 1.6;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

/* 审批卡 */
.perm {
  padding: 10px;
  border: 1px solid var(--accent-line);
  border-radius: var(--r);
  background: var(--accent-soft);
  animation: fade-in 140ms var(--ease);
}

.perm-title {
  display: flex;
  align-items: center;
  gap: 7px;
  margin-bottom: 8px;
  color: var(--accent);
  font-size: 12px;
  font-weight: 600;
}

.perm-kind {
  margin-left: auto;
  color: var(--fg-dim);
  font: 10px var(--font-mono);
  font-weight: 400;
  letter-spacing: 0.04em;
}

.perm-cmd {
  margin: 0;
  padding: 7px 9px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--bg);
  color: var(--fg);
  font: 11.5px/1.6 var(--font-mono);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.perm-meta {
  margin: 7px 0 9px;
  color: var(--fg-dim);
  font-size: 10.5px;
  line-height: 1.5;
  overflow-wrap: anywhere;
}

.perm-actions {
  display: flex;
  justify-content: flex-end;
  flex-wrap: wrap;
  gap: 6px;
}

.btn {
  height: 25px;
  padding: 0 10px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-2);
  color: var(--fg-2);
  font-size: 11.5px;
}

.btn:hover:not(:disabled) {
  color: var(--fg);
  border-color: var(--border-strong);
}

.btn.primary {
  border-color: var(--accent);
  background: var(--accent);
  color: var(--accent-fg);
}

.btn.primary:hover:not(:disabled) {
  border-color: var(--accent-hov);
  background: var(--accent-hov);
}

.btn:disabled {
  opacity: 0.5;
  cursor: default;
}

.resolving {
  margin-top: 6px;
  color: var(--fg-faint);
  font-size: 10.5px;
  text-align: right;
}

/* 每轮 token 用量 */
.turn-stats {
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 8px 2px 2px;
  border-top: 1px dashed var(--border);
}

.turn-stats-head {
  color: var(--fg-faint);
  font-size: 10px;
  letter-spacing: 0.04em;
}

.turn-stat {
  display: flex;
  align-items: center;
  gap: 10px;
  color: var(--fg-faint);
  font: 10.5px var(--font-mono);
}

.stat-turn {
  width: 24px;
  color: var(--fg-dim);
}

.stat-in {
  color: var(--accent);
}

.stat-out {
  color: var(--green);
}

.stat-cache {
  color: var(--fg-faint);
}

.stat-time {
  margin-left: auto;
  color: var(--fg-faint);
}

.archive-banner {
  flex: none;
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0 10px 8px;
  padding: 6px 9px;
  border: 1px solid var(--accent-line);
  border-radius: var(--r-sm);
  background: var(--accent-soft);
  color: var(--fg-dim);
  font-size: 10.5px;
}

.queue {
  flex: none;
  display: flex;
  flex-direction: column;
  gap: 4px;
  max-height: 112px;
  overflow-y: auto;
  margin: 0 10px 8px;
  padding: 6px 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-2);
}

.queue-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  color: var(--fg-faint);
  font-size: 10px;
}

.queue-head button {
  color: var(--accent);
  font-size: 10px;
}

.queue-item {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.queue-text {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  border: 0;
  background: transparent;
  color: var(--fg-dim);
  font-size: 11px;
  text-align: left;
  text-overflow: ellipsis;
  white-space: nowrap;
  cursor: pointer;
}

.queue-text:hover {
  color: var(--fg);
}

.queue-remove {
  flex: none;
  width: 16px;
  height: 16px;
  display: grid;
  place-items: center;
  border-radius: 50%;
  color: var(--fg-faint);
}

.queue-remove:hover {
  background: var(--panel-4);
  color: var(--red);
}

/* yolo 确认 */
.yolo-confirm {
  flex: none;
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin: 0 10px 8px;
  padding: 10px;
  border: 1px solid var(--accent-line);
  border-radius: var(--r);
  background: var(--accent-soft);
}

.yolo-text {
  display: flex;
  flex-direction: column;
  gap: 3px;
  color: var(--fg-dim);
  font-size: 11.5px;
  line-height: 1.5;
}

.yolo-text strong {
  color: var(--accent);
  font-size: 12px;
}

/* 错误 */
.error-banner {
  flex: none;
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin: 0 10px 8px;
  padding: 7px 9px;
  border: 1px solid var(--red);
  border-radius: var(--r-sm);
  background: var(--panel-2);
  font-size: 11px;
  overflow-wrap: anywhere;
}

.error-code {
  flex: none;
  color: var(--red);
  font-family: var(--font-mono);
}

.error-message {
  color: var(--fg-dim);
}

.error-hint {
  flex-basis: 100%;
  color: var(--fg-dim);
  font-size: 10px;
  line-height: 1.5;
}

/* 输入区 */
.composer {
  position: relative;
  flex: none;
  padding: 9px 10px 8px;
  border-top: 1px solid var(--border);
  background: var(--panel);
}

.mention-pop {
  position: absolute;
  bottom: calc(100% + 6px);
  left: 50%;
  z-index: 30;
  width: min(860px, calc(100% - 20px));
  max-height: 280px;
  padding: 5px;
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  background: var(--panel-2);
  box-shadow: var(--shadow);
  overflow-y: auto;
  transform: translateX(-50%);
}

.mention-head {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 3px 6px 6px;
  color: var(--fg-faint);
  font-size: 10px;
  letter-spacing: 0.04em;
  text-transform: uppercase;
}

.mention-head span {
  color: var(--accent);
  font-family: var(--font-mono);
  text-transform: none;
}

.mention-item {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 6px 8px;
  border-radius: var(--r-xs, 4px);
  color: var(--fg-dim);
  font: 11.5px var(--font-mono);
  text-align: left;
}

.mention-item:hover,
.mention-item.active {
  background: var(--panel-3);
  color: var(--fg);
}

.mention-path {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.mention-note {
  margin: 0;
  padding: 6px 8px;
  color: var(--fg-faint);
  font-size: 10.5px;
}

.mention-note.error {
  color: var(--red);
}

:global(.shell.agent-mode .mention-pop) {
  width: min(860px, calc(100% - 48px));
}

.comp-box {
  border: 1px solid var(--border);
  border-radius: var(--r);
  background: var(--panel-2);
  transition: border-color var(--dur) var(--ease);
}

.comp-box:focus-within {
  border-color: var(--accent-line);
}

.composer textarea {
  display: block;
  width: 100%;
  min-height: 38px;
  max-height: 132px;
  padding: 9px 10px 3px;
  border: 0;
  background: transparent;
  color: var(--fg);
  font: var(--agent-font, 12.5px)/1.6 var(--font-ui);
  resize: none;
  outline: none;
  overflow-y: auto;
}

.composer textarea::placeholder {
  color: var(--fg-faint);
}

.attach-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 4px;
  padding: 8px 10px 0;
}

.attachment-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  max-width: 100%;
  height: 22px;
  padding: 0 4px 0 7px;
  border: 1px solid var(--border);
  border-radius: 999px;
  background: var(--panel-3);
  color: var(--fg-dim);
  font-size: 10.5px;
}

.chip-name {
  max-width: 150px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.chip-size {
  color: var(--fg-faint);
  white-space: nowrap;
}

.chip-remove {
  display: grid;
  place-items: center;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  color: var(--fg-faint);
}

.chip-remove:hover {
  background: var(--panel-4);
  color: var(--red);
}

.attachment-error {
  flex-basis: 100%;
  margin: 0;
  color: var(--red);
  font-size: 10.5px;
}

.comp-ctrl {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 3px 6px 6px;
  flex-wrap: wrap;
}

.pill-wrap {
  position: relative;
  display: inline-flex;
}

.pill {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 23px;
  max-width: 170px;
  padding: 0 7px;
  border: 1px solid transparent;
  border-radius: var(--r-sm);
  color: var(--fg-dim);
  font-size: 11.5px;
  white-space: nowrap;
  transition: background var(--dur) var(--ease), color var(--dur) var(--ease),
    border-color var(--dur) var(--ease);
}

.pill:hover:not(:disabled) {
  background: var(--panel-3);
  color: var(--fg-2);
}

.pill:disabled {
  opacity: 0.5;
  cursor: default;
}

.pill .chev {
  flex: none;
  color: var(--fg-faint);
}

.pill-label {
  overflow: hidden;
  text-overflow: ellipsis;
}

.pill.mode-plan {
  border-style: dashed;
  border-color: var(--border-strong);
  color: var(--fg-2);
}

.pill.mode-auto {
  color: var(--fg-2);
}

.pill.mode-yolo {
  border-color: var(--accent);
  color: var(--accent);
}

.pill-menu {
  position: absolute;
  bottom: calc(100% + 5px);
  left: 0;
  z-index: 20;
  min-width: 150px;
  padding: 4px;
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  background: var(--panel-2);
  box-shadow: var(--shadow);
  animation: fade-in 100ms var(--ease);
}

.pill-menu.model-menu {
  min-width: 230px;
}

.pill-item {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 1px;
  width: 100%;
  padding: 6px 8px;
  border-radius: 3px;
  color: var(--fg-2);
  font-size: 11.5px;
  text-align: left;
}

.pill-item:hover {
  background: var(--panel-3);
  color: var(--fg);
}

.pill-item.active {
  background: var(--accent-soft);
  color: var(--accent);
}

.pill-item.yolo {
  color: var(--yellow);
}

.item-main {
  font-family: var(--font-mono);
  font-size: 11px;
}

.item-sub {
  color: var(--fg-faint);
  font-size: 10px;
}

.pill-empty {
  margin: 0;
  padding: 8px;
  color: var(--fg-faint);
  font-size: 10.5px;
}

.attach-btn {
  position: relative;
  width: 24px;
  height: 24px;
}

.attach-count {
  position: absolute;
  top: -2px;
  right: -2px;
  min-width: 12px;
  height: 12px;
  padding: 0 3px;
  border-radius: 999px;
  background: var(--accent);
  color: var(--accent-fg);
  font: 8.5px/12px var(--font-mono);
  text-align: center;
}

.send-btn {
  flex: none;
  display: grid;
  place-items: center;
  width: 24px;
  height: 24px;
  border-radius: var(--r-sm);
  background: var(--accent);
  color: var(--accent-fg);
  transition: background var(--dur) var(--ease), opacity var(--dur) var(--ease);
}

.send-btn:hover:not(:disabled) {
  background: var(--accent-hov);
}

.send-btn:disabled {
  background: var(--border-strong);
  color: var(--fg-faint);
  cursor: default;
}

/* 上下文条 */
.ctx {
  position: relative;
  margin-top: 8px;
  padding: 0 1px;
}

.ctx-top {
  cursor: pointer;
}

.ctx-pop {
  position: absolute;
  right: 0;
  bottom: calc(100% + 8px);
  z-index: 25;
  width: 270px;
  padding: 10px;
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  background: var(--panel-2);
  box-shadow: var(--shadow);
}

.ctx-pop-title {
  margin-bottom: 6px;
  color: var(--fg-dim);
  font-size: 10.5px;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-transform: uppercase;
}

.ctx-pop-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 3px 0;
  color: var(--fg-faint);
  font-size: 10.5px;
}

.ctx-pop-row strong {
  color: var(--fg-dim);
  font: 10.5px var(--font-mono);
  font-weight: 400;
}

.ctx-top {
  display: flex;
  align-items: center;
  gap: 8px;
}

.ctx-label {
  flex: none;
  color: var(--fg-faint);
  font-size: 10.5px;
  letter-spacing: 0.02em;
}

.ctx-bar {
  position: relative;
  flex: 1;
  height: 3px;
  border-radius: 2px;
  background: var(--border);
}

.ctx-fill {
  position: absolute;
  left: 0;
  top: 0;
  bottom: 0;
  border-radius: 2px;
  background: var(--accent);
  transition: width 160ms var(--ease), background-color 160ms var(--ease);
}

.ctx-fill.warn {
  background: var(--yellow);
}

.ctx-fill.flash {
  animation: ctx-flash 600ms var(--ease);
}

@keyframes ctx-flash {
  0%,
  100% {
    opacity: 1;
  }

  40% {
    opacity: 0.25;
  }
}

.ctx-tick {
  position: absolute;
  top: -2px;
  bottom: -2px;
  width: 1px;
  background: var(--fg-faint);
  transform: translateX(-0.5px);
}

.ctx-meta {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-top: 4px;
  color: var(--fg-faint);
  font: 10.5px var(--font-mono);
}

.ctx-val {
  color: var(--fg-dim);
}

@keyframes fade-in {
  from {
    opacity: 0;
    transform: translateY(2px);
  }

  to {
    opacity: 1;
    transform: none;
  }
}

@media (prefers-reduced-motion: reduce) {
  .sess-dot.running,
  .ctx-fill.flash,
  .perm,
  .pill-menu {
    animation: none;
  }

  .ctx-fill,
  .tool .chev,
  .compact-row .chev,
  .pill {
    transition: none;
  }
}

/* Agent 模式：Kimi 式 48px 顶栏、居中 860px 对话列与更大的正文 */
:global(.shell.agent-mode .agent-head){
  height: 48px;
  padding: 0 16px;
}

:global(.shell.agent-mode .agent-title){
  display: none;
}

:global(.shell.agent-mode .sess-name){
  color: var(--fg);
  font-size: 13.5px;
  font-weight: 600;
}

:global(.shell.agent-mode .stream){
  gap: 14px;
  padding: 18px 24px;
}

:global(.shell.agent-mode .stream > *){
  width: 100%;
  max-width: 860px;
  margin-right: auto;
  margin-left: auto;
}

:global(.shell.agent-mode .msg-text){
  font-size: var(--agent-font, 14px);
}

:global(.shell.agent-mode .msg.user .msg-text){
  padding: 10px 12px;
  border-radius: 12px;
  font-size: var(--agent-font, 14px);
}

:global(.shell.agent-mode .tool-name){
  font-size: 12px;
}

:global(.shell.agent-mode .tool-arg),
:global(.shell.agent-mode .tool-body){
  font-size: 12.5px;
}

:global(.shell.agent-mode .composer){
  padding: 12px 24px 10px;
}

:global(.shell.agent-mode .composer .comp-box),
:global(.shell.agent-mode .composer .ctx){
  width: 100%;
  max-width: 860px;
  margin-right: auto;
  margin-left: auto;
}

:global(.shell.agent-mode .composer textarea){
  min-height: 44px;
  font-size: var(--agent-font, 14px);
}

:global(.shell.agent-mode .empty) {
  max-width: 420px;
}

:global(.shell.agent-mode .archive-banner),
:global(.shell.agent-mode .error-banner) {
  width: calc(100% - 48px);
  max-width: 860px;
  margin-right: auto;
  margin-left: auto;
}

:global(.shell.agent-mode .archive-banner) {
  margin-bottom: 8px;
}

:global(.shell.agent-mode .queue) {
  width: calc(100% - 48px);
  max-width: 860px;
  margin-right: auto;
  margin-left: auto;
}

/* Agent 偏好：紧凑密度 */
.agent-body.density-compact .stream {
  gap: 8px;
  padding: 8px 10px;
}

.agent-body.density-compact .msg-text {
  line-height: 1.5;
}

.agent-body.density-compact .tool-head {
  height: 28px;
}

.agent-body.density-compact .tool-body,
.agent-body.density-compact .tool-arg {
  font-size: calc(var(--agent-font, 12.5px) - 1.5px);
}

.agent-body.density-compact .composer {
  padding: 6px 8px 6px;
}

.agent-body.density-compact .composer textarea {
  min-height: 34px;
}

.agent-body.density-compact .agent-head {
  height: 36px;
}
</style>
