<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import AgentPanel from './components/AgentPanel.vue'
import AgentPreview from './components/AgentPreview.vue'
import AgentSessions from './components/AgentSessions.vue'
import AppIcon from './components/AppIcon.vue'
import Breadcrumbs from './components/EditorBreadcrumbs.vue'
import CodeEditor from './components/CodeEditor.vue'
import DbPanel from './components/DbPanel.vue'
import ExtensionsPanel from './components/ExtensionsPanel.vue'
import FileTree from './components/FileTree.vue'
import GitPanel from './components/GitPanel.vue'
import ProvidersPanel from './components/ProvidersPanel.vue'
import SearchView from './components/SearchView.vue'
import SettingsPanel from './components/SettingsPanel.vue'
import TabBar from './components/TabBar.vue'
import TaskList from './components/TaskList.vue'
import TaskStatusBar from './components/TaskStatusBar.vue'
import TaskWorkbench from './components/TaskWorkbench.vue'
import { createAgentSession } from './services/agentSession'
import { createHistoryRecorder } from './services/agentHistory'
import {
  loadWorkspaces,
  removeWorkspace,
  samePath,
  saveWorkspaces,
  upsertWorkspace,
  type WorkspaceEntry,
} from './services/workspaceStore'
import {
  agentCancel,
  agentSend,
  dbConnect,
  dbDeleteConnection,
  dbDisconnect,
  dbExecute,
  dbExplain,
  dbList,
  dbQuery,
  dbSaveConnection,
  dbSchema,
  gitCommit,
  gitDiff,
  gitLog,
  gitStage,
  gitStatus,
  gitUnstage,
  historyMigrateWorkspace,
  isTauri,
  onCoreEvent,
  lspStartPlugins,
  onLspDiagnostics,
  openWorkspace,
  pickWorkspace,
  resolveApproval,
  sessionCreate,
  taskApply,
  taskBindSession,
  taskCancel,
  taskCreate,
  taskDiff,
  taskDiscard,
  taskList,
  taskStart,
  workspaceRoot,
} from './services/bridge'
import { createDbController } from './services/dbStore'
import { createGitController } from './services/gitStore'
import { LAYOUT_LIMITS, clamp } from './services/layoutProfile'
import { loadLayoutPrefs, saveLayoutPrefs } from './services/layoutPrefs'
import { createTaskController } from './services/taskStore'
import { extensionsExecuteCommand, extensionsList } from './services/extensions'
import { subscribeAgentContext } from './services/agentContext'
import { setFileDiagnostics } from './services/lspDiagnostics'
import { activateTab, closeTab, openTab, renameTab, tabsStore } from './services/tabs'
import {
  initUiPrefs,
  setLocale,
  setZoom,
  subscribePrefs,
  toggleTheme,
  uiPrefs,
  zoomBy,
} from './services/themeStore'

const { t, locale } = useI18n()

const prefs = ref(uiPrefs())
const unsubLsp = onLspDiagnostics(event => {
  setFileDiagnostics(event.path, event.serverId, event.diagnostics)
})

const unsubAgentContext = subscribeAgentContext(() => {
  rightTab.value = 'agent'
  rightVisible.value = true
})

const unsubPrefs = subscribePrefs(next => {
  prefs.value = next
})

// 右侧功能栏：Agent / 任务工作台 / Git / 数据库，四者共用一栏，编辑器始终保留。
type RightTab = 'agent' | 'workbench' | 'git' | 'db'
type SidebarView = 'explorer' | 'search' | 'tasks'
type AppMode = 'editor' | 'agent'
const APP_MODE_KEY = 'lc.appMode'
const rightTab = ref<RightTab>('agent')
const sidebarView = ref<SidebarView>('explorer')
const sidebarVisible = ref(true)
const rightVisible = ref(true)
const appMode = ref<AppMode>(
  localStorage.getItem(APP_MODE_KEY) === 'agent' ? 'agent' : 'editor',
)
const agentRail = ref<'git' | 'preview' | null>(null)

function toggleAgentRail(rail: 'git' | 'preview'): void {
  agentRail.value = agentRail.value === rail ? null : rail
  if (agentRail.value === 'git') void refreshGit()
}

function setAppMode(mode: AppMode): void {
  appMode.value = mode
  try {
    localStorage.setItem(APP_MODE_KEY, mode)
  } catch {
    // 隐私模式下 localStorage 不可用时只影响持久化。
  }
}

const agentSession = createAgentSession({
  // 控制器只关心 mode/model/thinking；task 绑定由任务工作台单独处理。
  sessionCreate: (mode, model, thinking) => sessionCreate(mode, null, model, thinking),
  agentSend,
  agentCancel,
  resolveApproval,
  onCoreEvent,
})

const historyRecorder = createHistoryRecorder(agentSession, () => projectRoot.value)

const taskSession = createTaskController({
  taskList,
  taskCreate,
  taskStart,
  taskCancel,
  taskApply,
  taskDiscard,
  taskDiff,
  taskBindSession,
  onCoreEvent,
})

const dbSession = createDbController({
  dbList,
  dbConnect,
  dbDisconnect,
  dbSchema,
  dbQuery,
  dbExecute,
  dbExplain,
  dbSaveConnection,
  dbDeleteConnection,
  onCoreEvent,
})

const gitSession = createGitController({
  gitStatus,
  gitDiff,
  gitStage,
  gitUnstage,
  gitCommit,
  gitLog,
})

// 布局尺寸仍复用原偏好；去掉 editor/agent 模式，改为 VS Code 式固定区域。
const layout = ref(loadLayoutPrefs())
watch(layout, value => saveLayoutPrefs(value), { deep: true })
const sidebarWidth = computed(() => layout.value.sidebarWidth)
const rightWidth = computed(() => layout.value.agentWidth)
const bottomHeight = computed(() => layout.value.bottomHeight)
const bottomVisible = ref(!layout.value.bottomCollapsed)

type ResizeTarget = 'sidebar' | 'right' | 'bottom'
function startResize(target: ResizeTarget, event: PointerEvent): void {
  event.preventDefault()
  const startX = event.clientX
  const startY = event.clientY
  const start = { ...layout.value }
  const move = (e: PointerEvent): void => {
    if (target === 'sidebar') {
      layout.value = {
        ...layout.value,
        sidebarWidth: clamp(start.sidebarWidth + e.clientX - startX, 200, 420),
      }
    } else if (target === 'right') {
      layout.value = {
        ...layout.value,
        agentWidth: clamp(
          start.agentWidth - (e.clientX - startX),
          LAYOUT_LIMITS.agent[0],
          LAYOUT_LIMITS.agent[1],
        ),
      }
    } else {
      layout.value = {
        ...layout.value,
        bottomHeight: clamp(
          start.bottomHeight - (e.clientY - startY),
          LAYOUT_LIMITS.bottom[0],
          LAYOUT_LIMITS.bottom[1],
        ),
      }
    }
  }
  const up = (): void => {
    window.removeEventListener('pointermove', move)
    window.removeEventListener('pointerup', up)
  }
  window.addEventListener('pointermove', move)
  window.addEventListener('pointerup', up)
}

// 工作区与项目名
const projectRoot = ref(new URLSearchParams(window.location.search).get('root') ?? '')
const treeRefresh = ref(0)
const projectName = computed(() => {
  if (!projectRoot.value) return t('explorer.project')
  // 浏览器演示用占位根 'workspace'，不应把它当成项目名展示。
  if (!isTauri() && projectRoot.value === 'workspace') return t('explorer.project')
  const parts = projectRoot.value.replace(/\\/g, '/').split('/').filter(Boolean)
  return parts.length > 0 ? parts[parts.length - 1] : projectRoot.value
})
const workspaceError = ref<string | null>(null)
const workspaces = ref<WorkspaceEntry[]>(loadWorkspaces())
const workspaceSwitching = ref(false)

function rememberWorkspace(root: string): void {
  if (!root) return
  const next = upsertWorkspace(workspaces.value, root)
  workspaces.value = next
  saveWorkspaces(next)
}

function agentBusy(): boolean {
  const state = agentSession.getState()
  if (state.streaming) return true
  return Boolean(
    state.agent && !state.archived && state.agent.approvals.length > 0,
  )
}

async function activateWorkspace(root: string): Promise<boolean> {
  if (samePath(root, projectRoot.value)) return true
  if (workspaceSwitching.value) return false
  if (agentBusy()) {
    workspaceError.value = t('agent.workspaceBusy')
    return false
  }
  workspaceSwitching.value = true
  workspaceError.value = null
  const res = await openWorkspace(root)
  workspaceSwitching.value = false
  if (!res.ok) {
    workspaceError.value = res.error.message
    return false
  }
  const next = res.data.root ?? root
  projectRoot.value = next
  rememberWorkspace(next)
  agentSession.resetSession()
  agentSession.setDraft('')
  treeRefresh.value += 1
  startLsp()
  void refreshGit()
  void dbSession.refresh()
  void taskSession.refresh()
  return true
}

async function newSessionInWorkspace(root: string): Promise<void> {
  if (!(await activateWorkspace(root))) return
  agentSession.resetSession()
  agentSession.setDraft('')
  setAppMode('agent')
}

function removeWorkspaceEntry(root: string): void {
  if (samePath(root, projectRoot.value) && agentBusy()) {
    workspaceError.value = t('agent.workspaceBusy')
    return
  }
  const next = removeWorkspace(workspaces.value, root)
  workspaces.value = next
  saveWorkspaces(next)
  if (samePath(root, projectRoot.value)) {
    const fallback = next[0]
    if (fallback) void activateWorkspace(fallback.root)
  }
}

async function openWorkspaceDialog(): Promise<void> {
  workspaceError.value = null
  if (agentBusy()) {
    workspaceError.value = t('agent.workspaceBusy')
    return
  }
  const res = await pickWorkspace()
  if (!res.ok) {
    workspaceError.value = res.error.message
    return
  }
  if (res.data.root) {
    const next = res.data.root
    if (!samePath(next, projectRoot.value)) {
      agentSession.resetSession()
      agentSession.setDraft('')
    }
    projectRoot.value = next
    rememberWorkspace(next)
    treeRefresh.value += 1
    startLsp()
    void refreshGit()
    void dbSession.refresh()
    void taskSession.refresh()
  }
}

const tabsState = ref(tabsStore.get())
const unsubTabs = tabsStore.subscribe(state => {
  tabsState.value = state
})
const openTabs = computed(() => tabsState.value.tabs)
const activeTabId = computed(() => tabsState.value.activeId)

function activateEditor(id: string): void {
  activateTab(id)
  sidebarView.value = 'explorer'
}

// 顶部快速打开 / 命令面板
const quickQuery = ref('')
const searchSeed = ref({ query: '', seq: 0 })
const quickInput = ref<HTMLInputElement | null>(null)
const quickOpen = ref(false)
const quickIndex = ref(0)

interface QuickItem {
  id: string
  label: string
  detail: string
  action: () => void | Promise<void>
}

interface PluginCommandItem {
  command: string
  title: string
  pluginId: string
  pluginName: string
}

const pluginCommands = ref<PluginCommandItem[]>([])

function onExtensionsChanged(): void {
  void refreshPluginCommands()
  if (projectRoot.value) startLsp()
}

function startLsp(): void {
  void lspStartPlugins().then(res => {
    if (res.ok) window.dispatchEvent(new Event('lsp:started'))
  })
}

async function refreshPluginCommands(): Promise<void> {
  const res = await extensionsList()
  if (!res.ok) return
  pluginCommands.value = res.data.plugins
    .filter(plugin => plugin.enabled)
    .flatMap(plugin =>
      plugin.commands.map(command => ({
        command: command.command,
        title: command.title,
        pluginId: plugin.id,
        pluginName: plugin.name,
      })),
    )
}

const commands = computed<QuickItem[]>(() => [
  { id: 'cmd:workspace', label: t('menu.openWorkspace'), detail: 'Ctrl+O', action: openWorkspaceDialog },
  { id: 'cmd:settings', label: t('activity.settings'), detail: 'Ctrl+,', action: openSettings },
  { id: 'cmd:providers', label: t('activity.providers'), detail: '', action: openProviders },
  { id: 'cmd:extensions', label: t('activity.extensions'), detail: '', action: openExtensions },
  { id: 'cmd:git', label: t('activity.git'), detail: '', action: () => { rightTab.value = 'git'; rightVisible.value = true; void refreshGit() } },
  { id: 'cmd:db', label: t('activity.db'), detail: '', action: () => { rightTab.value = 'db'; rightVisible.value = true; void dbSession.refresh() } },
  { id: 'cmd:agent', label: t('activity.agent'), detail: '', action: () => { rightTab.value = 'agent'; rightVisible.value = true } },
  { id: 'cmd:theme', label: t('menu.toggleTheme'), detail: 'Ctrl+Alt+T', action: toggleTheme },
  { id: 'cmd:bottom', label: t('menu.toggleBottom'), detail: 'Ctrl+J', action: () => { bottomVisible.value = !bottomVisible.value } },
  ...pluginCommands.value.map(item => ({
    id: `ext:${item.command}`,
    label: item.title,
    detail: item.pluginName,
    action: () => { void extensionsExecuteCommand(item.command) },
  })),
])

const quickResults = computed<QuickItem[]>(() => {
  const query = quickQuery.value.trim().toLowerCase()
  if (!query) return []
  const files: QuickItem[] = openTabs.value
    .filter(tab => `${tab.title} ${tab.path ?? ''}`.toLowerCase().includes(query))
    .map(tab => ({
      id: `tab:${tab.id}`,
      label: tab.title,
      detail: tab.path ?? '',
      action: () => activateEditor(tab.id),
    }))
  const cmds = commands.value.filter(cmd => cmd.label.toLowerCase().includes(query))
  const queryText = quickQuery.value.trim()
  const searchInFiles: QuickItem = {
    id: 'cmd:searchFiles',
    label: t('search.searchInFiles', { query: queryText }),
    detail: '',
    action: () => {
      searchSeed.value = { query: queryText, seq: searchSeed.value.seq + 1 }
      sidebarVisible.value = true
      sidebarView.value = 'search'
    },
  }
  return [...files, ...cmds, searchInFiles].slice(0, 8)
})

watch(quickResults, list => {
  quickIndex.value = 0
  if (list.length === 0) quickOpen.value = quickQuery.value.trim().length > 0
})

function openQuick(): void {
  void refreshPluginCommands()
  quickOpen.value = true
  void nextTick(() => quickInput.value?.focus())
}

function runQuick(item: QuickItem | undefined): void {
  if (!item) return
  quickOpen.value = false
  quickQuery.value = ''
  quickInput.value?.blur()
  void item.action()
}

/** 点击快速打开以外的区域时收起面板（含清空查询），与 VS Code 行为一致。 */
function onDocumentPointerDown(event: PointerEvent): void {
  const target = event.target
  if (!(target instanceof Element)) return
  if (!target.closest('.quick-wrap')) {
    quickOpen.value = false
    quickQuery.value = ''
  }
}

/** Tab/Shift+Tab 离开快速打开区域时收起；结果项之间移动焦点不关闭。 */
function onQuickFocusOut(event: FocusEvent): void {
  const wrap = event.currentTarget
  const next = event.relatedTarget
  if (!(wrap instanceof Node)) return
  if (next instanceof Node && wrap.contains(next)) return
  quickOpen.value = false
  quickQuery.value = ''
}

function onQuickKey(event: KeyboardEvent): void {
  if (event.key === 'Escape') {
    quickOpen.value = false
    quickQuery.value = ''
    quickInput.value?.blur()
    return
  }
  if (event.key === 'ArrowDown') {
    event.preventDefault()
    quickIndex.value = Math.min(quickIndex.value + 1, quickResults.value.length - 1)
  } else if (event.key === 'ArrowUp') {
    event.preventDefault()
    quickIndex.value = Math.max(quickIndex.value - 1, 0)
  } else if (event.key === 'Enter') {
    event.preventDefault()
    runQuick(quickResults.value[quickIndex.value])
  }
}

// 自定义菜单栏
const openMenu = ref<'file' | 'edit' | 'view' | 'help' | null>(null)
const menuItems = computed(() => ({
  file: [
    { label: t('menu.openWorkspace'), action: openWorkspaceDialog },
    { label: t('menu.save'), action: () => { window.dispatchEvent(new KeyboardEvent('keydown', { key: 's', ctrlKey: true })) } },
    { label: t('menu.closeTab'), action: () => { if (activeTabId.value) closeTab(activeTabId.value) } },
  ],
  edit: [
    { label: t('menu.undo'), action: () => document.execCommand('undo') },
    { label: t('menu.redo'), action: () => document.execCommand('redo') },
    { label: t('menu.find'), action: openQuick },
  ],
  view: [
    { label: t('menu.toggleTheme'), action: toggleTheme },
    { label: t('menu.zoomIn'), action: () => zoomBy(0.1) },
    { label: t('menu.zoomOut'), action: () => zoomBy(-0.1) },
    { label: t('menu.resetZoom'), action: () => setZoom(1) },
    { label: t('menu.toggleBottom'), action: () => { bottomVisible.value = !bottomVisible.value } },
  ],
  help: [
    { label: t('menu.about'), action: openSettings },
  ],
}))

function toggleMenu(name: 'file' | 'edit' | 'view' | 'help'): void {
  openMenu.value = openMenu.value === name ? null : name
}

function runMenu(action: () => void | Promise<void>): void {
  openMenu.value = null
  void action()
}

// 设置作为编辑器区的独立页签（VS Code 行为），不在窄侧栏里硬塞表单。
const SETTINGS_TAB_ID = '__settings__'
function openSettings(): void {
  setAppMode('editor')
  openTab({
    id: SETTINGS_TAB_ID,
    title: t('activity.settings'),
    path: null,
    content: '',
  })
}

const PROVIDERS_TAB_ID = '__providers__'
function openProviders(): void {
  setAppMode('editor')
  openTab({
    id: PROVIDERS_TAB_ID,
    title: t('providers.title'),
    path: null,
    content: '',
  })
}

const EXTENSIONS_TAB_ID = '__extensions__'
const extensionsMounted = ref(false)
function openExtensions(): void {
  setAppMode('editor')
  extensionsMounted.value = true
  openTab({
    id: EXTENSIONS_TAB_ID,
    title: t('activity.extensions'),
    path: null,
    content: '',
  })
}

watch(locale, () => {
  renameTab(SETTINGS_TAB_ID, t('activity.settings'))
  renameTab(PROVIDERS_TAB_ID, t('providers.title'))
  renameTab(EXTENSIONS_TAB_ID, t('activity.extensions'))
})

// 主题 / 缩放 / 语言
function changeLocale(): void {
  const next = prefs.value.locale === 'zh-CN' ? 'en' : 'zh-CN'
  setLocale(next)
  locale.value = next
  document.documentElement.lang = next
}

function refreshGit(): Promise<void> {
  return Promise.all([gitSession.refresh(), gitSession.loadLog()]).then(() => undefined)
}

function selectRight(tab: RightTab): void {
  setAppMode('editor')
  rightTab.value = tab
  rightVisible.value = true
  if (tab === 'git') void refreshGit()
  if (tab === 'db') void dbSession.refresh()
}

function selectSidebar(view: SidebarView): void {
  setAppMode('editor')
  sidebarView.value = view
  sidebarVisible.value = true
}

// 自定义窗口控件（Tauri 无边框窗口）
function windowAction(action: 'minimize' | 'toggleMaximize' | 'close'): void {
  if (!isTauri()) return
  const internals = (window as unknown as { __TAURI_INTERNALS__?: { invoke: (cmd: string, args?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__
  const command = action === 'minimize'
    ? 'plugin:window|minimize'
    : action === 'toggleMaximize'
      ? 'plugin:window|toggle_maximize'
      : 'plugin:window|close'
  void internals?.invoke(command, { label: 'main' })
}

// 快捷键
function onKey(event: KeyboardEvent): void {
  const mod = event.ctrlKey || event.metaKey
  if (mod && event.key.toLowerCase() === 'p') {
    event.preventDefault()
    openQuick()
  } else if (mod && event.key === '=') {
    event.preventDefault()
    zoomBy(0.1)
  } else if (mod && event.key === '-') {
    event.preventDefault()
    zoomBy(-0.1)
  } else if (mod && event.key === '0') {
    event.preventDefault()
    setZoom(1)
  } else if (mod && event.altKey && event.key.toLowerCase() === 't') {
    event.preventDefault()
    toggleTheme()
  } else if (mod && event.key.toLowerCase() === 'b') {
    event.preventDefault()
    sidebarVisible.value = !sidebarVisible.value
  } else if (mod && event.key.toLowerCase() === 'j') {
    event.preventDefault()
    bottomVisible.value = !bottomVisible.value
  } else if (mod && event.key === ',') {
    event.preventDefault()
    openSettings()
  }
}

onMounted(() => {
  initUiPrefs()
  document.documentElement.dataset.theme = prefs.value.theme
  locale.value = prefs.value.locale
  document.documentElement.lang = prefs.value.locale
  renameTab(SETTINGS_TAB_ID, t('activity.settings'))
  renameTab(PROVIDERS_TAB_ID, t('providers.title'))
  renameTab(EXTENSIONS_TAB_ID, t('activity.extensions'))
  window.addEventListener('keydown', onKey)
  window.addEventListener('click', () => { openMenu.value = null })
  document.addEventListener('pointerdown', onDocumentPointerDown, true)
  void taskSession.refresh()
  void refreshPluginCommands()
  void workspaceRoot().then(res => {
    if (res.ok && res.data.root) {
      projectRoot.value = res.data.root
      rememberWorkspace(res.data.root)
      void historyMigrateWorkspace(res.data.root).then(moved => {
        if (moved.ok && moved.data > 0) {
          window.dispatchEvent(new Event('history:changed'))
        }
      })
    } else if (!isTauri() && projectRoot.value === '') {
      projectRoot.value = 'workspace'
    }
  })
})

onBeforeUnmount(() => {
  unsubPrefs()
  unsubLsp()
  unsubAgentContext()
  unsubTabs()
  historyRecorder.dispose()
  agentSession.dispose()
  taskSession.dispose()
  dbSession.dispose()
  gitSession.dispose()
  window.removeEventListener('keydown', onKey)
  document.removeEventListener('pointerdown', onDocumentPointerDown, true)
})
</script>

<template>
  <div
    class="shell"
    :class="{
      'sidebar-off': !sidebarVisible,
      'right-off': !rightVisible,
      'agent-mode': appMode === 'agent',
    }"
  >
    <header
      class="titlebar"
      data-tauri-drag-region
    >
      <div
        class="brand"
        data-tauri-drag-region
      >
        <span class="brand-mark" />
        <span class="brand-text">Lite Code</span>
      </div>
      <div
        class="titlebar-mode"
        role="group"
        :aria-label="t('activity.mode')"
      >
        <button
          type="button"
          class="mode-chip"
          :class="{ active: appMode === 'editor' }"
          :title="t('activity.modeEditor')"
          :aria-pressed="appMode === 'editor'"
          @click="setAppMode('editor')"
        >
          <AppIcon
            name="code"
            :size="14"
          />
        </button>
        <button
          type="button"
          class="mode-chip"
          :class="{ active: appMode === 'agent' }"
          :title="t('activity.modeAgent')"
          :aria-pressed="appMode === 'agent'"
          @click="setAppMode('agent')"
        >
          <AppIcon
            name="agent"
            :size="14"
          />
        </button>
      </div>
      <nav class="menus">
        <button
          type="button"
          @click.stop="toggleMenu('file')"
        >
          {{ t('menu.file') }}
        </button>
        <button
          type="button"
          @click.stop="toggleMenu('edit')"
        >
          {{ t('menu.edit') }}
        </button>
        <button
          type="button"
          @click.stop="toggleMenu('view')"
        >
          {{ t('menu.view') }}
        </button>
        <button
          type="button"
          @click.stop="toggleMenu('help')"
        >
          {{ t('menu.help') }}
        </button>
        <div
          v-if="openMenu"
          class="menu-pop"
          @click.stop
        >
          <button
            v-for="item in menuItems[openMenu]"
            :key="item.label"
            type="button"
            @click="runMenu(item.action)"
          >
            {{ item.label }}
          </button>
        </div>
      </nav>

      <div
        class="quick-wrap"
        data-tauri-drag-region="false"
        @focusout="onQuickFocusOut"
      >
        <input
          ref="quickInput"
          v-model="quickQuery"
          class="quick-input"
          :placeholder="t('titlebar.searchPlaceholder')"
          spellcheck="false"
          @focus="quickOpen = true"
          @keydown="onQuickKey"
          @input="quickOpen = true"
        >
        <div
          v-if="quickOpen && quickQuery && quickResults.length"
          class="quick-pop"
        >
          <button
            v-for="(item, index) in quickResults"
            :key="item.id"
            type="button"
            class="quick-item"
            :class="{ active: index === quickIndex }"
            @mousedown.prevent="runQuick(item)"
          >
            <span class="quick-label">{{ item.label }}</span>
            <span class="quick-detail">{{ item.detail }}</span>
          </button>
        </div>
      </div>

      <div class="title-actions">
        <button
          type="button"
          class="icon-btn"
          :title="t('settings.language')"
          @click="changeLocale"
        >
          {{ prefs.locale === 'zh-CN' ? '中' : 'EN' }}
        </button>
        <button
          type="button"
          class="icon-btn"
          :title="t('menu.toggleTheme')"
          @click="toggleTheme"
        >
          <AppIcon
            :name="prefs.theme === 'dark' ? 'sun' : 'moon'"
            :size="16"
          />
        </button>
        <template v-if="isTauri()">
          <button
            type="button"
            class="win-btn"
            :title="t('titlebar.minimize')"
            @click="windowAction('minimize')"
          >
            —
          </button>
          <button
            type="button"
            class="win-btn"
            :title="t('titlebar.maximize')"
            @click="windowAction('toggleMaximize')"
          >
            □
          </button>
          <button
            type="button"
            class="win-btn close"
            :title="t('titlebar.close')"
            @click="windowAction('close')"
          >
            ×
          </button>
        </template>
      </div>
    </header>

    <div class="workspace">
      <nav
        v-if="appMode === 'editor'"
        class="activity"
        :aria-label="t('activity.explorer')"
      >
        <button
          type="button"
          class="act-btn"
          :class="{ active: sidebarView === 'explorer' && sidebarVisible }"
          :title="t('activity.explorer')"
          @click="selectSidebar('explorer')"
        >
          <AppIcon name="explorer" />
        </button>
        <button
          type="button"
          class="act-btn"
          :class="{ active: sidebarView === 'search' && sidebarVisible }"
          :title="t('activity.search')"
          @click="selectSidebar('search')"
        >
          <AppIcon name="search" />
        </button>
        <button
          type="button"
          class="act-btn"
          :class="{ active: rightTab === 'git' && rightVisible }"
          :title="t('activity.git')"
          @click="selectRight('git')"
        >
          <AppIcon name="git" />
        </button>
        <button
          type="button"
          class="act-btn"
          :class="{ active: rightTab === 'db' && rightVisible }"
          :title="t('activity.db')"
          @click="selectRight('db')"
        >
          <AppIcon name="db" />
        </button>
        <button
          type="button"
          class="act-btn"
          :class="{ active: sidebarView === 'tasks' && sidebarVisible }"
          :title="t('activity.tasks')"
          @click="selectSidebar('tasks')"
        >
          <AppIcon name="tasks" />
        </button>
        <button
          type="button"
          class="act-btn"
          :class="{ active: activeTabId === '__extensions__' }"
          :title="t('activity.extensions')"
          @click="openExtensions"
        >
          <AppIcon name="extensions" />
        </button>
        <button
          type="button"
          class="act-btn"
          :class="{ active: activeTabId === '__providers__' }"
          :title="t('activity.providers')"
          @click="openProviders"
        >
          <AppIcon name="providers" />
        </button>
        <div class="act-space" />
        <button
          type="button"
          class="act-btn"
          :class="{ active: activeTabId === '__settings__' }"
          :title="t('activity.settings')"
          @click="openSettings"
        >
          <AppIcon name="settings" />
        </button>
      </nav>

      <aside
        v-show="sidebarVisible && appMode === 'editor'"
        class="sidebar"
        :style="{ width: `${sidebarWidth}px` }"
      >
        <div class="sidebar-head">
          <div class="project">
            <AppIcon
              name="folder"
              :size="14"
            />
            <span
              class="project-name"
              :title="projectRoot"
            >{{ projectName }}</span>
          </div>
          <div class="sidebar-actions">
            <button
              type="button"
              class="icon-btn"
              :title="t('explorer.openWorkspace')"
              @click="openWorkspaceDialog"
            >
              <AppIcon
                name="folder"
                :size="14"
              />
            </button>
            <button
              type="button"
              class="icon-btn"
              :title="t('explorer.refresh')"
              @click="treeRefresh += 1"
            >
              <AppIcon
                name="refresh"
                :size="14"
              />
            </button>
          </div>
        </div>

        <div
          v-if="workspaceError"
          class="side-error"
        >
          {{ workspaceError }}
        </div>

        <div
          v-if="sidebarView === 'explorer'"
          class="view explorer"
        >
          <div class="section-title">
            <span>{{ t('explorer.openEditors') }}</span>
            <span class="count">{{ openTabs.length }}</span>
          </div>
          <div class="open-editors">
            <div
              v-for="tab in openTabs"
              :key="tab.id"
              class="editor-row"
              :class="{ active: tab.id === activeTabId }"
              @click="activateEditor(tab.id)"
            >
              <span
                class="dirty"
                :class="{ on: tab.dirty }"
              />
              <span class="editor-name">{{ tab.title }}</span>
              <button
                type="button"
                class="editor-close"
                @click.stop="closeTab(tab.id)"
              >
                ×
              </button>
            </div>
            <div
              v-if="openTabs.length === 0"
              class="empty-line"
            >
              {{ t('explorer.noEditors') }}
            </div>
          </div>
          <div class="tree-body">
            <FileTree
              :root="projectRoot"
              :refresh-token="treeRefresh"
            />
          </div>
        </div>

        <SearchView
          v-show="sidebarView === 'search'"
          :root="projectRoot"
          :seed="searchSeed"
        />

        <div
          v-if="sidebarView === 'tasks'"
          class="view tasks-view"
        >
          <TaskList
            :controller="taskSession"
            @created="selectRight('workbench')"
          />
        </div>
      </aside>

      <div
        v-if="sidebarVisible"
        class="resizer"
        role="separator"
        aria-orientation="vertical"
        @pointerdown="startResize('sidebar', $event)"
      />

      <main
        v-show="appMode === 'editor'"
        class="editor-col"
      >
        <TabBar />
        <Breadcrumbs />
        <div class="editor-card">
          <ExtensionsPanel
            v-if="extensionsMounted"
            v-show="activeTabId === '__extensions__'"
            @changed="onExtensionsChanged"
          />
          <ProvidersPanel v-show="activeTabId === '__providers__'" />
          <template v-if="activeTabId !== '__providers__' && activeTabId !== '__extensions__'">
            <SettingsPanel
              v-if="activeTabId === '__settings__'"
              @manage-providers="openProviders"
            />
            <CodeEditor v-else-if="openTabs.length > 0" />
            <div
              v-else
              class="welcome"
            >
              <div class="welcome-mark" />
              <h1>{{ t('welcome.title') }}</h1>
              <p>{{ t('welcome.subtitle') }}</p>
              <button
                type="button"
                class="primary"
                @click="openWorkspaceDialog"
              >
                {{ t('welcome.openWorkspace') }}
              </button>
              <div class="shortcuts">
                {{ t('welcome.shortcuts') }}
              </div>
            </div>
          </template>
        </div>

        <div
          v-if="bottomVisible"
          class="resizer h"
          role="separator"
          aria-orientation="horizontal"
          @pointerdown="startResize('bottom', $event)"
        />
        <section
          v-if="bottomVisible"
          class="bottom"
          :style="{ height: `${bottomHeight}px` }"
        >
          <div class="bottom-tabs">
            <span class="bottom-tab active">{{ t('bottom.terminal') }}</span>
            <span class="bottom-tab">{{ t('bottom.output') }}</span>
            <span class="bottom-tab">{{ t('bottom.problems') }}</span>
            <button
              type="button"
              class="icon-btn bottom-close"
              @click="bottomVisible = false"
            >
              ×
            </button>
          </div>
          <div class="bottom-body">
            —
          </div>
        </section>
      </main>

      <div
        v-if="appMode === 'editor' && rightVisible"
        class="resizer"
        role="separator"
        aria-orientation="vertical"
        @pointerdown="startResize('right', $event)"
      />
      <aside
        v-if="appMode === 'editor' && rightVisible"
        class="right"
        :style="{ width: `${rightWidth}px` }"
      >
        <div class="right-tabs">
          <button
            type="button"
            :class="{ active: rightTab === 'agent' }"
            @click="selectRight('agent')"
          >
            {{ t('activity.agent') }}
          </button>
          <button
            type="button"
            :class="{ active: rightTab === 'workbench' }"
            @click="selectRight('workbench')"
          >
            {{ t('task.workbenchTitle') }}
          </button>
          <button
            type="button"
            :class="{ active: rightTab === 'git' }"
            @click="selectRight('git')"
          >
            {{ t('activity.git') }}
          </button>
          <button
            type="button"
            :class="{ active: rightTab === 'db' }"
            @click="selectRight('db')"
          >
            {{ t('activity.db') }}
          </button>
          <button
            type="button"
            class="right-close"
            @click="rightVisible = false"
          >
            ×
          </button>
        </div>
        <div class="right-body">
          <AgentPanel
            v-if="rightTab === 'agent'"
            :controller="agentSession"
            :workspace="projectRoot"
            @manage-providers="openProviders"
          />
          <TaskWorkbench
            v-else-if="rightTab === 'workbench'"
            :controller="taskSession"
            @collapse="rightVisible = false"
          />
          <GitPanel
            v-else-if="rightTab === 'git'"
            :controller="gitSession"
            @close="rightTab = 'agent'"
          />
          <DbPanel
            v-else
            :controller="dbSession"
            @close="rightTab = 'agent'"
          />
        </div>
      </aside>

      <template v-if="appMode === 'agent'">
        <aside class="agent-sessions">
          <AgentSessions
            :controller="agentSession"
            :workspace="projectRoot"
            :workspaces="workspaces"
            :active-workspace="projectRoot"
            show-workspaces
            @select-workspace="activateWorkspace"
            @remove-workspace="removeWorkspaceEntry"
            @add-workspace="openWorkspaceDialog"
            @new-session-in="newSessionInWorkspace"
          />
          <div class="agent-sidebar-footer">
            <button
              type="button"
              @click="openProviders"
            >
              <AppIcon
                name="providers"
                :size="14"
              />
              {{ t('activity.providers') }}
            </button>
            <button
              type="button"
              @click="openSettings"
            >
              <AppIcon
                name="settings"
                :size="14"
              />
              {{ t('activity.settings') }}
            </button>
          </div>
        </aside>
        <main class="agent-main">
          <p
            v-if="workspaceError"
            class="agent-error"
          >
            {{ workspaceError }}
          </p>
          <div class="agent-chat">
            <AgentPanel
              :controller="agentSession"
              :workspace="projectRoot"
              show-rail-toggle
              show-preview-toggle
              :rail-open="agentRail === 'git'"
              :preview-open="agentRail === 'preview'"
              @manage-providers="openProviders"
              @toggle-rail="toggleAgentRail('git')"
              @toggle-preview="toggleAgentRail('preview')"
            />
            <aside
              v-if="agentRail"
              class="agent-rail"
            >
              <GitPanel
                v-if="agentRail === 'git'"
                :controller="gitSession"
                @close="agentRail = null"
              />
              <AgentPreview
                v-else
                @close="agentRail = null"
              />
            </aside>
          </div>
        </main>
      </template>
    </div>

    <footer
      v-if="appMode === 'editor'"
      class="statusbar"
    >
      <span class="status-item">⎇ {{ t('status.branch') }}</span>
      <span class="status-item dim">Node —</span>
      <span class="status-item dim">MCP 0</span>
      <TaskStatusBar
        :controller="taskSession"
        @reveal="selectRight('workbench')"
      />
      <span class="status-space" />
      <div class="zoom-control">
        <button
          type="button"
          @click="zoomBy(-0.1)"
        >
          −
        </button>
        <button
          type="button"
          @click="setZoom(1)"
        >
          {{ Math.round(prefs.zoom * 100) }}%
        </button>
        <button
          type="button"
          @click="zoomBy(0.1)"
        >
          +
        </button>
      </div>
      <button
        type="button"
        class="status-item"
        @click="openSettings"
      >
        {{ t('activity.settings') }}
      </button>
      <button
        type="button"
        class="status-item"
        @click="bottomVisible = !bottomVisible"
      >
        {{ t('bottom.terminal') }}
      </button>
    </footer>
  </div>
</template>

<style scoped>
.shell {
  height: 100%;
  display: flex;
  flex-direction: column;
  background: var(--bg);
}

/* ---- 自定义标题栏 ---- */
.titlebar {
  position: relative;
  z-index: 30;
  flex: none;
  height: 38px;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 0 10px 0 14px;
  background: var(--titlebar);
  border-bottom: 1px solid var(--border);
  user-select: none;
}

.brand {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 132px;
}

.brand-mark {
  width: 14px;
  height: 14px;
  border-radius: 5px;
  background: linear-gradient(135deg, var(--accent), #9d7bff);
  box-shadow: 0 0 0 3px var(--accent-soft);
}

.brand-text {
  font-size: 12px;
  font-weight: 650;
  letter-spacing: 0.02em;
  color: var(--fg);
}

.titlebar-mode {
  flex: none;
  display: flex;
  align-items: center;
  gap: 2px;
  padding: 2px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-2);
}

.mode-chip {
  width: 24px;
  height: 20px;
  display: grid;
  place-items: center;
  border-radius: var(--r-xs);
  color: var(--fg-faint);
}

.mode-chip:hover {
  color: var(--fg-2);
}

.mode-chip.active {
  background: var(--panel-4);
  color: var(--accent);
}

/* Agent 模式：Kimi Code Desktop 式中性色板与无边框面板 */
.shell.agent-mode {
  --bg: #141414;
  --titlebar: #181818;
  --panel: #181818;
  --panel-2: #1b1b1b;
  --panel-3: #212121;
  --panel-4: #282828;
  --border: rgb(255 255 255 / 7%);
  --border-strong: rgb(255 255 255 / 12%);
  --fg: #efefef;
  --fg-2: #c1c1c1;
  --fg-dim: #929292;
  --fg-faint: #6b6b6b;
  --accent: #dedede;
  --accent-hov: #f5f5f5;
  --accent-fg: #121212;
  --accent-soft: rgb(255 255 255 / 7%);
  --accent-line: rgb(255 255 255 / 18%);
  --r: 12px;
  --r-sm: 8px;
  --r-xs: 6px;
}

:root[data-theme='light'] .shell.agent-mode {
  --bg: #f6f6f6;
  --titlebar: #f0f0f0;
  --panel: #f0f0f0;
  --panel-2: #ffffff;
  --panel-3: #e9e9e9;
  --panel-4: #dfdfdf;
  --border: #dddddd;
  --border-strong: #c6c6c6;
  --fg: #1c1c1c;
  --fg-2: #4d4d4d;
  --fg-dim: #6c6c6c;
  --fg-faint: #9a9a9a;
  --accent: #262626;
  --accent-hov: #111111;
  --accent-fg: #ffffff;
  --accent-soft: rgb(0 0 0 / 6%);
  --accent-line: rgb(0 0 0 / 18%);
  --r: 12px;
  --r-sm: 8px;
  --r-xs: 6px;
}

.shell.agent-mode .workspace {
  padding: 0;
  gap: 0;
}

.shell.agent-mode .agent-sessions {
  width: 272px;
  border: 0;
  border-right: 1px solid var(--border);
  border-radius: 0;
}

.shell.agent-mode .agent-main {
  border: 0;
  border-radius: 0;
  background: var(--bg);
}

.shell.agent-mode .brand-mark {
  background: linear-gradient(135deg, #dedede, #8a8a8a);
  box-shadow: none;
}

:root[data-theme='light'] .shell.agent-mode .brand-mark {
  background: linear-gradient(135deg, #262626, #8a8a8a);
}

.shell.agent-mode .activity {
  display: none;
}

.menus {
  position: relative;
  display: flex;
  gap: 2px;
}

.menus > button {
  height: 26px;
  padding: 0 8px;
  border-radius: var(--r-xs);
  color: var(--fg-dim);
  font-size: 12px;
}

.menus > button:hover {
  background: var(--panel-3);
  color: var(--fg);
}

.menu-pop {
  position: absolute;
  top: 30px;
  left: 0;
  z-index: 60;
  min-width: 190px;
  padding: 5px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--r);
  box-shadow: var(--shadow);
}

.menu-pop button {
  display: block;
  width: 100%;
  padding: 7px 10px;
  text-align: left;
  border-radius: var(--r-xs);
  font-size: 12px;
  color: var(--fg-2);
}

.menu-pop button:hover {
  background: var(--accent-soft);
  color: var(--fg);
}

.quick-wrap {
  position: relative;
  flex: 1;
  max-width: 520px;
  margin: 0 auto;
}

.quick-input {
  width: 100%;
  height: 26px;
  padding: 0 10px;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: 999px;
  color: var(--fg);
  font-size: 12px;
}

.quick-input:focus {
  border-color: var(--accent-line);
  outline: none;
  box-shadow: 0 0 0 3px var(--accent-soft);
}

.quick-pop {
  position: absolute;
  top: 32px;
  left: 0;
  right: 0;
  z-index: 60;
  padding: 5px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--r);
  box-shadow: var(--shadow);
}

.quick-item {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  padding: 7px 9px;
  border-radius: var(--r-xs);
  text-align: left;
}

.quick-item.active,
.quick-item:hover {
  background: var(--accent-soft);
}

.quick-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
}

.quick-detail {
  color: var(--fg-faint);
  font-size: 10px;
  font-family: var(--font-mono);
}

.title-actions {
  display: flex;
  align-items: center;
  gap: 2px;
  margin-left: auto;
}

.icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border-radius: var(--r-xs);
  color: var(--fg-dim);
  font-size: 11px;
}

.icon-btn:hover {
  background: var(--panel-3);
  color: var(--fg);
}

.win-btn {
  width: 30px;
  height: 26px;
  border-radius: var(--r-xs);
  color: var(--fg-dim);
  font-size: 12px;
}

.win-btn:hover {
  background: var(--panel-3);
  color: var(--fg);
}

.win-btn.close:hover {
  background: #d9534f;
  color: #fff;
}

/* ---- 主区域 ---- */
.workspace {
  flex: 1;
  min-height: 0;
  display: flex;
  padding: 6px;
  gap: 6px;
}

.activity {
  flex: none;
  width: 48px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
  padding: 6px 0;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--r);
}

.act-btn {
  width: 40px;
  height: 40px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--r-sm);
  color: var(--fg-dim);
}

.act-btn :deep(svg) {
  width: 20px;
  height: 20px;
}

.act-btn:hover {
  color: var(--fg);
  background: var(--panel-3);
}

.act-btn.active {
  color: var(--accent);
  background: var(--accent-soft);
  box-shadow: inset 2px 0 0 var(--accent);
}

.act-space {
  flex: 1;
}

.mode-switch {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 2px;
  width: 100%;
  padding-bottom: 5px;
  margin-bottom: 2px;
  border-bottom: 1px solid var(--border);
}

.act-divider {
  width: 22px;
  height: 1px;
  margin: 2px 0 4px;
  background: var(--border);
}

.agent-sessions {
  flex: none;
  width: 268px;
  min-width: 0;
  display: flex;
  flex-direction: column;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--r);
  overflow: hidden;
}

.agent-main {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--r);
  overflow: hidden;
}

.agent-chat {
  flex: 1;
  min-height: 0;
  display: flex;
}

.agent-rail {
  flex: none;
  width: clamp(320px, 34vw, 420px);
  min-width: 0;
  display: flex;
  border-left: 1px solid var(--border);
  background: var(--panel);
  overflow: hidden;
}

.agent-rail :deep(.git-panel) {
  flex: 1;
  min-width: 0;
}

.agent-sidebar-footer {
  flex: none;
  display: flex;
  gap: 4px;
  padding: 6px 8px;
  border-top: 1px solid var(--border);
}

.agent-sidebar-footer button {
  flex: 1;
  height: 28px;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  border-radius: var(--r-sm);
  color: var(--fg-dim);
  font-size: 11px;
}

.agent-sidebar-footer button:hover {
  background: var(--panel-3);
  color: var(--fg);
}

.agent-error {
  flex: none;
  margin: 0;
  padding: 7px 12px;
  border-bottom: 1px solid var(--red);
  background: var(--panel-2);
  color: var(--red);
  font-size: 11px;
}

.sidebar {
  flex: none;
  min-width: 0;
  display: flex;
  flex-direction: column;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--r);
  overflow: hidden;
}

.sidebar-head {
  flex: none;
  display: flex;
  align-items: center;
  gap: 6px;
  height: 36px;
  padding: 0 8px 0 10px;
  border-bottom: 1px solid var(--border);
}

.project {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  color: var(--fg-2);
}

.project-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
  font-weight: 650;
  letter-spacing: 0.01em;
}

.sidebar-actions {
  margin-left: auto;
  display: flex;
  gap: 2px;
}

.side-error {
  padding: 6px 10px;
  color: var(--red);
  font-size: 11px;
  border-bottom: 1px solid var(--border);
}

.view {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.section-title {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 28px;
  padding: 0 10px;
  color: var(--fg-dim);
  font-size: 10px;
  font-weight: 650;
  letter-spacing: 0.06em;
  text-transform: uppercase;
}

.section-title .count {
  color: var(--fg-faint);
}

.open-editors {
  max-height: 120px;
  overflow: auto;
  padding: 0 6px 6px;
}

.editor-row {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 24px;
  padding: 0 6px;
  border-radius: var(--r-xs);
  color: var(--fg-2);
  font-size: 12px;
  cursor: pointer;
}

.editor-row:hover {
  background: var(--panel-3);
}

.editor-row.active {
  background: var(--accent-soft);
  color: var(--fg);
}

.dirty {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: transparent;
}

.dirty.on {
  background: var(--accent);
}

.editor-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.editor-close {
  color: var(--fg-faint);
  font-size: 12px;
}

.tree-title {
  border-top: 1px solid var(--border);
}

.tree-body {
  flex: 1;
  min-height: 0;
  display: flex;
  overflow: hidden;
  border-top: 1px solid var(--border);
}

.tree-body :deep(.tree) {
  flex: 1;
}

.empty-line {
  padding: 8px 10px;
  color: var(--fg-faint);
  font-size: 11px;
}

.tasks-view,
.settings-view {
  overflow: auto;
}

.editor-col {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.editor-card {
  flex: 1;
  min-height: 0;
  display: flex;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r);
  overflow: hidden;
}

.welcome {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: var(--fg-dim);
}

.welcome-mark {
  width: 42px;
  height: 42px;
  border-radius: 14px;
  background: linear-gradient(135deg, var(--accent), #9d7bff);
  box-shadow: 0 0 0 8px var(--accent-soft);
  margin-bottom: 8px;
}

.welcome h1 {
  margin: 0;
  color: var(--fg);
  font-size: 20px;
  font-weight: 650;
}

.welcome p {
  margin: 0 0 8px;
  font-size: 13px;
}

.welcome .primary {
  height: 32px;
  padding: 0 16px;
  border-radius: var(--r-sm);
  background: var(--accent);
  color: var(--accent-fg);
  font-size: 12px;
  font-weight: 600;
}

.welcome .primary:hover {
  background: var(--accent-hov);
}

.shortcuts {
  margin-top: 8px;
  color: var(--fg-faint);
  font-size: 11px;
}

.resizer {
  flex: 0 0 3px;
  z-index: 5;
  border-radius: 999px;
  background: transparent;
  cursor: col-resize;
}

.resizer.h {
  flex-basis: 3px;
  cursor: row-resize;
}

.resizer:hover {
  background: var(--accent-line);
}

.bottom {
  flex: none;
  display: flex;
  flex-direction: column;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--r);
  overflow: hidden;
}

.bottom-tabs {
  flex: none;
  display: flex;
  align-items: center;
  gap: 14px;
  height: 34px;
  padding: 0 10px;
  border-bottom: 1px solid var(--border);
  font-size: 11.5px;
}

.bottom-tab {
  color: var(--fg-dim);
}

.bottom-tab.active {
  color: var(--accent);
}

.bottom-close {
  margin-left: auto;
  width: 22px;
  height: 22px;
}

.bottom-body {
  flex: 1;
  padding: 8px 12px;
  color: var(--fg-faint);
  font-family: var(--font-mono);
  font-size: 12px;
}

.right {
  flex: none;
  min-width: 0;
  display: flex;
  flex-direction: column;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--r);
  overflow: hidden;
}

.right-tabs {
  flex: none;
  display: flex;
  align-items: center;
  gap: 2px;
  height: 34px;
  padding: 0 6px;
  border-bottom: 1px solid var(--border);
  overflow-x: auto;
}

.right-tabs button {
  height: 24px;
  padding: 0 8px;
  border-radius: var(--r-xs);
  color: var(--fg-dim);
  font-size: 11px;
  white-space: nowrap;
}

.right-tabs button:hover {
  color: var(--fg);
  background: var(--panel-3);
}

.right-tabs button.active {
  color: var(--accent);
  background: var(--accent-soft);
}

.right-close {
  margin-left: auto;
}

.right-body {
  flex: 1;
  min-height: 0;
  display: flex;
  overflow: hidden;
}

.right-body :deep(.db-side) {
  width: 220px;
}

.right-body :deep(.files-pane) {
  flex-basis: 200px;
  min-width: 160px;
}

/* ---- 状态栏 ---- */
.statusbar {
  flex: none;
  height: 24px;
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 0 10px;
  background: var(--titlebar);
  border-top: 1px solid var(--border);
  color: var(--fg-dim);
  font-size: 11px;
}

.status-item {
  color: var(--fg-dim);
  font-size: 11px;
}

.status-item.dim {
  color: var(--fg-faint);
}

.status-space {
  flex: 1;
}

.zoom-control {
  display: flex;
  align-items: center;
  gap: 2px;
}

.zoom-control button {
  min-width: 24px;
  height: 20px;
  border-radius: var(--r-xs);
  color: var(--fg-dim);
  font-size: 11px;
}

.zoom-control button:hover {
  background: var(--panel-3);
  color: var(--fg);
}

@media (max-width: 1100px) {
  .right {
    position: absolute;
    top: 44px;
    right: 8px;
    bottom: 30px;
    z-index: 40;
    width: min(420px, calc(100vw - 90px)) !important;
    box-shadow: var(--shadow);
  }

  .right-off .right {
    display: none;
  }
}
</style>
