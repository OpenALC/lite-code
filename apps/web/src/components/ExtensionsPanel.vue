<script setup lang="ts">
// 插件管理页：「已安装」列表（docs/EXTENSIONS.md）+「浏览市场」tab
// （docs/plans/plugin-marketplace.md §3.8：搜索/安装/更新/卸载 + §3.7 确认对话框）。
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  extensionsExecuteCommand,
  extensionsList,
  extensionsOpenDir,
  extensionsReload,
  extensionsSetEnabled,
  type PluginCommand,
  type PluginInfo,
  type PluginList,
  type PluginRuntime,
} from '../services/extensions'
import {
  marketplaceInstall,
  marketplaceSearch,
  marketplaceUninstall,
  marketplaceUpdate,
  type MarketPlugin,
} from '../services/marketplace'
import {
  badgesForPlugin,
  missingConfirmFields,
  staleSourcesNotice,
} from '../services/marketplaceUi'

const { t } = useI18n()
const emit = defineEmits<{ changed: [] }>()

interface CommandFeedback {
  pluginId: string
  command: string
  ok: boolean
  text: string
  fallback: boolean
}

const plugins = ref<PluginInfo[]>([])
const extensionsDir = ref('')
const nodeAvailable = ref(true)
const loading = ref(true)
const reloading = ref(false)
const openingDir = ref(false)
const isFallback = ref(false)
const error = ref<string | null>(null)
const togglingId = ref<string | null>(null)
const runningCommand = ref<string | null>(null)
const commandFeedback = ref<CommandFeedback | null>(null)

function applyList(list: PluginList, fallback: boolean): void {
  plugins.value = list.plugins
  extensionsDir.value = list.dir
  nodeAvailable.value = list.nodeAvailable
  isFallback.value = fallback
  commandFeedback.value = null
}

async function load(): Promise<void> {
  loading.value = true
  error.value = null
  const res = await extensionsList()
  loading.value = false
  if (!res.ok) {
    error.value = res.error.message
    return
  }
  applyList(res.data, res.fallback)
}

async function reload(): Promise<void> {
  if (loading.value || reloading.value) return
  reloading.value = true
  error.value = null
  const res = await extensionsReload()
  reloading.value = false
  if (!res.ok) {
    error.value = res.error.message
    return
  }
  applyList(res.data, res.fallback)
  emit('changed')
}

async function openDir(): Promise<void> {
  if (openingDir.value) return
  openingDir.value = true
  error.value = null
  const res = await extensionsOpenDir()
  openingDir.value = false
  if (!res.ok) error.value = res.error.message
}

async function togglePlugin(plugin: PluginInfo, event: Event): Promise<void> {
  const input = event.target as HTMLInputElement
  if (togglingId.value !== null) {
    input.checked = plugin.enabled
    return
  }
  const nextEnabled = input.checked
  togglingId.value = plugin.id
  error.value = null
  const res = await extensionsSetEnabled(plugin.id, nextEnabled)
  togglingId.value = null
  if (!res.ok) {
    input.checked = plugin.enabled
    error.value = res.error.message
    return
  }
  applyList(res.data, res.fallback)
  emit('changed')
}

function canRun(plugin: PluginInfo): boolean {
  if (!plugin.enabled) return false
  // node 与 wasm 运行器都依赖 Node；declarative 没有可执行命令。
  return plugin.runtime !== 'declarative' && nodeAvailable.value
}

function runTitle(plugin: PluginInfo): string | undefined {
  if (!plugin.enabled) return t('extensions.disabledHint')
  if (plugin.runtime === 'declarative') return t('extensions.declarativeHint')
  if (!nodeAvailable.value) return t('extensions.nodeHint')
  return undefined
}

function runtimeLabel(runtime: PluginRuntime): string {
  switch (runtime) {
    case 'node':
      return t('extensions.runtimes.node')
    case 'wasm':
      return t('extensions.runtimes.wasm')
    case 'declarative':
      return t('extensions.runtimes.declarative')
  }
}

function formatResult(result: unknown): string {
  if (result === null || result === undefined) return t('extensions.noResult')
  if (typeof result === 'string') return result
  try {
    return JSON.stringify(result, null, 2) ?? String(result)
  } catch {
    return String(result)
  }
}

async function runCommand(plugin: PluginInfo, command: PluginCommand): Promise<void> {
  if (runningCommand.value !== null || !canRun(plugin)) return
  runningCommand.value = command.command
  commandFeedback.value = null
  const res = await extensionsExecuteCommand(command.command)
  runningCommand.value = null
  if (!res.ok) {
    commandFeedback.value = {
      pluginId: plugin.id,
      command: command.command,
      ok: false,
      text: res.error.message,
      fallback: false,
    }
    return
  }
  commandFeedback.value = {
    pluginId: plugin.id,
    command: command.command,
    ok: true,
    text: formatResult(res.data.result),
    fallback: res.fallback,
  }
}

function feedbackFor(pluginId: string, command: string): CommandFeedback | null {
  const current = commandFeedback.value
  if (current && current.pluginId === pluginId && current.command === command) return current
  return null
}

// ==== 浏览市场 tab（§3.8）：搜索/安装/更新/卸载状态由本组件自持 ====

type PanelTab = 'installed' | 'browse'

const activeTab = ref<PanelTab>('installed')
const searchQuery = ref('')
const searchResults = ref<MarketPlugin[]>([])
const staleSources = ref<string[]>([])
const searching = ref(false)
const searchLoaded = ref(false)
const browseError = ref<string | null>(null)
const browseFallback = ref(false)
const actingId = ref<string | null>(null)
const actionError = ref<string | null>(null)
let searchTimer: ReturnType<typeof setTimeout> | null = null

const staleNotice = computed(() => staleSourcesNotice(staleSources.value))

async function runSearch(refresh: boolean): Promise<void> {
  if (searching.value) return
  searching.value = true
  browseError.value = null
  // 安装/卸载/更新成功后刷新搜索，绕过 5 分钟缓存以更新已装/可更新标记。
  const res = await marketplaceSearch(searchQuery.value, refresh)
  searching.value = false
  searchLoaded.value = true
  if (!res.ok) {
    // ohos shim 未转发市场命令时 reject（not_implemented），同样落到此空态。
    browseError.value = res.error.message
    return
  }
  searchResults.value = res.data.results
  staleSources.value = res.data.staleSources
  browseFallback.value = res.fallback
}

// 搜索框防抖 300ms；切到浏览 tab 时惰性首次加载（不阻塞已安装页启动）。
watch(searchQuery, () => {
  if (searchTimer !== null) clearTimeout(searchTimer)
  searchTimer = setTimeout(() => {
    searchTimer = null
    void runSearch(false)
  }, 300)
})

watch(activeTab, tab => {
  if (tab === 'browse' && !searchLoaded.value && !searching.value) void runSearch(false)
})

// ==== 安装/更新确认对话框（§3.7 必显字段 + force 二次确认路径）====

interface ConfirmState {
  plugin: MarketPlugin
  mode: 'install' | 'update'
  /** engines/deps 预检失败原因（invalid_argument）；非 null 时确认按钮变「仍要安装」并走 force。 */
  precheckError: string | null
  acting: boolean
  actionError: string | null
}

const confirmState = ref<ConfirmState | null>(null)
const confirmDialog = ref<HTMLElement | null>(null)

async function openConfirm(plugin: MarketPlugin, mode: 'install' | 'update'): Promise<void> {
  if (actingId.value !== null) return
  actionError.value = null
  confirmState.value = {
    plugin,
    mode,
    precheckError: null,
    acting: false,
    actionError: null,
  }
  await nextTick()
  confirmDialog.value?.focus()
}

function closeConfirm(): void {
  if (confirmState.value?.acting) return
  confirmState.value = null
}

const confirmButtonLabel = computed(() => {
  const state = confirmState.value
  if (!state) return ''
  if (state.acting) {
    return state.mode === 'install'
      ? t('marketplace.installing')
      : t('marketplace.updating')
  }
  if (state.precheckError !== null) {
    // §3.5：engines/deps 预检失败由确认对话框承担 force——显示原因并要求二次点击。
    return state.mode === 'install'
      ? t('marketplace.forceInstall')
      : t('marketplace.forceUpdate')
  }
  return state.mode === 'install'
    ? t('marketplace.confirmInstall')
    : t('marketplace.confirmUpdate')
})

async function proceedConfirm(): Promise<void> {
  const state = confirmState.value
  if (!state || state.acting) return
  state.acting = true
  state.actionError = null
  const force = state.precheckError !== null
  const res =
    state.mode === 'install'
      ? await marketplaceInstall(state.plugin.id, {
          version: state.plugin.version,
          source: state.plugin.source,
          force,
        })
      : await marketplaceUpdate(state.plugin.id, force)
  if (!confirmState.value) return // 请求在途时对话框被关闭，结果不再回写。
  confirmState.value.acting = false
  if (!res.ok) {
    if (res.error.code === 'invalid_argument') {
      // 安装前预检（engines/deps）失败：保留对话框，二次点击走 force=true。
      confirmState.value.precheckError = res.error.message
    } else {
      confirmState.value.actionError = res.error.message
    }
    return
  }
  confirmState.value = null
  applyList(res.data, res.fallback)
  emit('changed')
  void runSearch(true)
}

async function uninstallPlugin(plugin: MarketPlugin): Promise<void> {
  if (actingId.value !== null) return
  actingId.value = plugin.id
  actionError.value = null
  const res = await marketplaceUninstall(plugin.id)
  actingId.value = null
  if (!res.ok) {
    actionError.value = res.error.message
    return
  }
  applyList(res.data, res.fallback)
  emit('changed')
  void runSearch(true)
}

onMounted(load)
</script>

<template>
  <div class="extensions-page">
    <header class="page-head">
      <div class="head-copy">
        <h1>{{ t('extensions.title') }}</h1>
        <p>{{ t('extensions.subtitle') }}</p>
      </div>
      <div class="head-actions">
        <span
          v-if="isFallback"
          class="demo-badge"
        >{{ t('extensions.demo') }}</span>
        <button
          type="button"
          class="ghost"
          :disabled="loading || reloading"
          @click="reload"
        >
          {{ reloading ? t('extensions.reloading') : t('extensions.reload') }}
        </button>
        <button
          type="button"
          class="ghost"
          :disabled="openingDir"
          @click="openDir"
        >
          {{ openingDir ? t('extensions.openingDir') : t('extensions.openDir') }}
        </button>
      </div>
    </header>

    <nav
      class="tabs"
      role="tablist"
    >
      <button
        type="button"
        role="tab"
        class="tab"
        :class="{ active: activeTab === 'installed' }"
        :aria-selected="activeTab === 'installed'"
        @click="activeTab = 'installed'"
      >
        {{ t('marketplace.tabInstalled') }}
      </button>
      <button
        type="button"
        role="tab"
        class="tab"
        :class="{ active: activeTab === 'browse' }"
        :aria-selected="activeTab === 'browse'"
        @click="activeTab = 'browse'"
      >
        {{ t('marketplace.tabBrowse') }}
      </button>
    </nav>

    <template v-if="activeTab === 'installed'">
      <div
        v-if="error"
        class="banner error"
      >
        {{ error }}
      </div>
      <div
        v-if="isFallback"
        class="banner fallback"
      >
        {{ t('extensions.fallback') }}
      </div>
      <div
        v-if="loading"
        class="empty"
      >
        {{ t('extensions.loading') }}
      </div>

      <template v-else>
        <div
          v-if="!nodeAvailable"
          class="banner warn"
        >
          {{ t('extensions.nodeUnavailable') }}
        </div>

        <div class="dir-bar">
          <span class="dir-label">{{ t('extensions.dir') }}</span>
          <code class="dir-path">{{ extensionsDir || '—' }}</code>
        </div>

        <div
          v-if="plugins.length === 0"
          class="empty empty-card"
        >
          {{ t('extensions.empty') }}
        </div>

        <div
          v-else
          class="plugin-list"
        >
          <article
            v-for="plugin in plugins"
            :key="plugin.id"
            class="card plugin-card"
          >
            <header class="plugin-head">
              <div class="plugin-title">
                <h2>{{ plugin.name || plugin.id }}</h2>
                <span
                  class="plugin-version"
                  :title="t('extensions.version')"
                >v{{ plugin.version }}</span>
                <span
                  class="runtime-badge"
                  :class="`runtime-${plugin.runtime}`"
                  :title="t('extensions.runtime')"
                >{{ runtimeLabel(plugin.runtime) }}</span>
              </div>
              <label class="switch">
                <input
                  type="checkbox"
                  role="switch"
                  :checked="plugin.enabled"
                  :disabled="togglingId !== null"
                  :aria-label="t('extensions.enabled')"
                  @change="togglePlugin(plugin, $event)"
                >
                <span>{{ plugin.enabled ? t('extensions.enabledOn') : t('extensions.enabledOff') }}</span>
              </label>
            </header>

            <p
              v-if="plugin.description"
              class="plugin-desc"
            >
              {{ plugin.description }}
            </p>

            <div class="meta">
              <div class="meta-row">
                <span class="meta-label">{{ t('extensions.id') }}</span>
                <code class="meta-value">{{ plugin.id }}</code>
              </div>
              <div class="meta-row">
                <span class="meta-label">{{ t('extensions.path') }}</span>
                <code class="meta-value">{{ plugin.path }}</code>
              </div>
              <div
                v-if="plugin.mcpServers.length"
                class="meta-row"
              >
                <span class="meta-label">MCP</span>
                <code class="meta-value">{{ plugin.mcpServers.join(', ') }}</code>
              </div>
              <div
                v-if="plugin.languageServers.length"
                class="meta-row"
              >
                <span class="meta-label">LSP</span>
                <code class="meta-value">{{ plugin.languageServers.join(', ') }}</code>
              </div>
            </div>

            <div
              v-if="plugin.error"
              class="plugin-error"
            >
              <span class="error-label">{{ t('extensions.error') }}</span>
              <span>{{ plugin.error }}</span>
            </div>

            <section class="commands">
              <div class="commands-head">
                <h3>{{ t('extensions.commands') }}</h3>
                <span class="command-count">{{ plugin.commands.length }}</span>
              </div>
              <div
                v-if="plugin.commands.length === 0"
                class="empty small"
              >
                {{ t('extensions.noCommands') }}
              </div>
              <div
                v-else
                class="command-list"
              >
                <div
                  v-for="command in plugin.commands"
                  :key="command.command"
                  class="command-item"
                >
                  <div class="command-row">
                    <div class="command-copy">
                      <span class="command-title">{{ command.title }}</span>
                      <code class="command-id">{{ command.command }}</code>
                    </div>
                    <button
                      type="button"
                      class="ghost small run-button"
                      :disabled="runningCommand !== null || !canRun(plugin)"
                      :title="runTitle(plugin)"
                      @click="runCommand(plugin, command)"
                    >
                      {{ runningCommand === command.command ? t('extensions.running') : t('extensions.run') }}
                    </button>
                  </div>
                  <div
                    v-if="feedbackFor(plugin.id, command.command)"
                    class="command-result"
                    :class="{ failed: feedbackFor(plugin.id, command.command)?.ok === false }"
                  >
                    <div class="result-head">
                      <span class="result-label">
                        {{ feedbackFor(plugin.id, command.command)?.ok ? t('extensions.result') : t('extensions.error') }}
                      </span>
                      <span
                        v-if="feedbackFor(plugin.id, command.command)?.fallback"
                        class="demo-badge"
                      >
                        {{ t('extensions.demo') }}
                      </span>
                    </div>
                    <pre class="result-body">{{ feedbackFor(plugin.id, command.command)?.text }}</pre>
                  </div>
                </div>
              </div>
            </section>
          </article>
        </div>
      </template>
    </template>

    <section
      v-else
      class="browse"
    >
      <div class="search-bar">
        <input
          v-model="searchQuery"
          type="search"
          class="search-input"
          :placeholder="t('marketplace.searchPlaceholder')"
          :aria-label="t('marketplace.searchPlaceholder')"
        >
      </div>

      <div
        v-if="browseError"
        class="empty empty-card"
      >
        <p class="empty-title">
          {{ t('marketplace.unavailable') }}
        </p>
        <p class="empty-detail">
          {{ browseError }}
        </p>
        <button
          type="button"
          class="ghost"
          @click="runSearch(true)"
        >
          {{ t('marketplace.retry') }}
        </button>
      </div>

      <template v-else>
        <div
          v-if="!searchLoaded"
          class="empty"
        >
          {{ t('marketplace.searching') }}
        </div>

        <template v-else>
          <div
            v-if="staleNotice"
            class="banner warn"
          >
            {{ t(staleNotice.key, staleNotice.params) }}
          </div>
          <div
            v-if="browseFallback"
            class="banner fallback"
          >
            {{ t('marketplace.fallback') }}
          </div>
          <div
            v-if="actionError"
            class="banner error"
          >
            {{ actionError }}
          </div>

          <div
            v-if="searchResults.length === 0"
            class="empty empty-card"
          >
            {{ t('marketplace.empty') }}
          </div>

          <div
            v-else
            class="market-list"
          >
            <article
              v-for="plugin in searchResults"
              :key="`${plugin.source}:${plugin.id}`"
              class="card market-card"
            >
              <header class="plugin-head">
                <div class="plugin-title">
                  <h2>{{ plugin.id }}</h2>
                  <span
                    class="plugin-version"
                    :title="t('extensions.version')"
                  >v{{ plugin.version }}</span>
                  <span
                    class="runtime-badge"
                    :class="`runtime-${plugin.runtime}`"
                    :title="t('extensions.runtime')"
                  >{{ plugin.runtime }}</span>
                  <span
                    v-for="badge in badgesForPlugin(plugin)"
                    :key="badge"
                    class="market-badge"
                    :class="`badge-${badge}`"
                  >{{ t(`marketplace.badges.${badge}`) }}</span>
                  <span
                    v-if="plugin.updateAvailable"
                    class="market-badge badge-update"
                  >{{ t('marketplace.updateAvailable') }}</span>
                </div>
                <div class="market-actions">
                  <template v-if="plugin.installedVersion !== null">
                    <button
                      v-if="plugin.updateAvailable"
                      type="button"
                      class="ghost small run-button"
                      :disabled="actingId !== null"
                      @click="openConfirm(plugin, 'update')"
                    >
                      {{ t('marketplace.update') }}
                    </button>
                    <span
                      v-else
                      class="installed-mark"
                    >{{ t('marketplace.installed') }} v{{ plugin.installedVersion }}</span>
                    <button
                      type="button"
                      class="ghost small"
                      :disabled="actingId !== null"
                      @click="uninstallPlugin(plugin)"
                    >
                      {{ actingId === plugin.id ? t('marketplace.uninstalling') : t('marketplace.uninstall') }}
                    </button>
                  </template>
                  <button
                    v-else
                    type="button"
                    class="ghost small run-button"
                    :disabled="actingId !== null"
                    @click="openConfirm(plugin, 'install')"
                  >
                    {{ t('marketplace.install') }}
                  </button>
                </div>
              </header>

              <p
                v-if="plugin.description"
                class="plugin-desc"
              >
                {{ plugin.description }}
              </p>

              <div class="meta">
                <div class="meta-row">
                  <span class="meta-label">{{ t('marketplace.source') }}</span>
                  <code class="meta-value">{{ plugin.source }}</code>
                </div>
                <div
                  v-if="plugin.deps.length > 0"
                  class="meta-row"
                >
                  <span class="meta-label">{{ t('marketplace.deps') }}</span>
                  <code class="meta-value">{{ plugin.deps.join(', ') }}</code>
                </div>
              </div>
            </article>
          </div>
        </template>
      </template>
    </section>

    <div
      v-if="confirmState"
      class="modal-mask"
      @click.self="closeConfirm"
    >
      <div
        ref="confirmDialog"
        class="confirm-dialog"
        role="dialog"
        aria-modal="true"
        tabindex="-1"
        :aria-label="confirmState.mode === 'install' ? t('marketplace.confirmInstallTitle') : t('marketplace.confirmUpdateTitle')"
        @keydown.esc.prevent="closeConfirm"
      >
        <h2 class="confirm-title">
          {{ confirmState.mode === 'install' ? t('marketplace.confirmInstallTitle') : t('marketplace.confirmUpdateTitle') }}
        </h2>

        <div
          v-if="missingConfirmFields(confirmState.plugin).length > 0"
          class="banner warn"
        >
          {{ t('marketplace.confirmIncomplete') }}
        </div>

        <div class="meta confirm-meta">
          <div class="meta-row">
            <span class="meta-label">{{ t('extensions.id') }}</span>
            <code class="meta-value">{{ confirmState.plugin.id }}</code>
          </div>
          <div class="meta-row">
            <span class="meta-label">{{ t('extensions.version') }}</span>
            <code class="meta-value">v{{ confirmState.plugin.version }}</code>
          </div>
          <div class="meta-row">
            <span class="meta-label">{{ t('extensions.runtime') }}</span>
            <code class="meta-value">{{ confirmState.plugin.runtime }}</code>
          </div>
          <div class="meta-row">
            <span class="meta-label">{{ t('marketplace.source') }}</span>
            <code class="meta-value">{{ confirmState.plugin.source }}</code>
          </div>
          <div class="meta-row">
            <span class="meta-label">{{ t('marketplace.signature') }}</span>
            <code class="meta-value">{{ t(`marketplace.signatureStates.${confirmState.plugin.signatureState}`) }}</code>
          </div>
          <div class="meta-row">
            <span class="meta-label">{{ t('marketplace.size') }}</span>
            <code class="meta-value">{{ t('marketplace.sizeUnknown') }}</code>
          </div>
          <div class="meta-row">
            <span class="meta-label">{{ t('marketplace.permissions') }}</span>
            <code
              v-if="confirmState.plugin.permissions.length > 0"
              class="meta-value"
            >{{ confirmState.plugin.permissions.join(', ') }}</code>
            <code
              v-else
              class="meta-value"
            >{{ t('marketplace.noPermissions') }}</code>
          </div>
        </div>

        <p
          v-if="confirmState.plugin.runtime === 'node'"
          class="node-warning"
        >
          {{ t('marketplace.nodeWarning') }}
        </p>

        <div
          v-if="confirmState.precheckError !== null"
          class="banner error"
        >
          <span class="error-label">{{ t('marketplace.precheckFailed') }}</span>
          <span>{{ confirmState.precheckError }}</span>
        </div>
        <div
          v-else-if="confirmState.actionError !== null"
          class="banner error"
        >
          {{ confirmState.actionError }}
        </div>

        <div class="confirm-actions">
          <button
            type="button"
            class="ghost"
            :disabled="confirmState.acting"
            @click="closeConfirm"
          >
            {{ t('marketplace.cancel') }}
          </button>
          <button
            type="button"
            class="ghost small confirm-proceed"
            :disabled="confirmState.acting"
            @click="proceedConfirm"
          >
            {{ confirmButtonLabel }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.extensions-page {
  height: 100%;
  overflow: auto;
  padding: 18px 20px 28px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  width: 100%;
  max-width: 1180px;
  margin: 0 auto;
}

.page-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
  flex-wrap: wrap;
}

.head-copy {
  min-width: 0;
}

.page-head h1 {
  margin: 0 0 4px;
  font-size: 18px;
  font-weight: 650;
  color: var(--fg);
}

.page-head p {
  margin: 0;
  font-size: 12px;
  color: var(--fg-dim);
  line-height: 1.5;
}

.head-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.demo-badge {
  font-size: 11px;
  color: var(--yellow);
  border: 1px solid var(--yellow);
  border-radius: 999px;
  padding: 2px 8px;
  white-space: nowrap;
}

.banner {
  border-radius: var(--r-sm);
  padding: 8px 10px;
  font-size: 12px;
  line-height: 1.5;
  border: 1px solid var(--border);
  overflow-wrap: anywhere;
}

.banner.error {
  color: var(--red);
  border-color: var(--red);
}

.banner.warn {
  color: var(--yellow);
  border-color: var(--yellow);
}

.banner.fallback {
  color: var(--fg-dim);
  background: var(--panel-2);
}

.dir-bar {
  display: flex;
  align-items: baseline;
  gap: 8px;
  flex-wrap: wrap;
  min-width: 0;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-2);
  font-size: 11px;
}

.dir-label {
  color: var(--fg-dim);
  flex: 0 0 auto;
}

.dir-path {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--fg-2);
  overflow-wrap: anywhere;
  min-width: 0;
}

.plugin-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
}

.card {
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: var(--r);
  padding: 14px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
}

.plugin-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 10px 16px;
  flex-wrap: wrap;
  min-width: 0;
}

.plugin-title {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  min-width: 0;
}

.plugin-title h2 {
  margin: 0;
  font-size: 14px;
  font-weight: 650;
  color: var(--fg);
  overflow-wrap: anywhere;
}

.plugin-version {
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--fg-faint);
}

.runtime-badge {
  font-size: 10px;
  line-height: 1.6;
  padding: 0 7px;
  border-radius: 999px;
  border: 1px solid var(--border);
  color: var(--fg-dim);
  background: var(--panel-3);
  white-space: nowrap;
}

.runtime-node {
  color: var(--green);
  border-color: color-mix(in srgb, var(--green) 45%, transparent);
  background: color-mix(in srgb, var(--green) 12%, transparent);
}

.runtime-wasm {
  color: var(--accent);
  border-color: var(--accent-line);
  background: var(--accent-soft);
}

.runtime-declarative {
  color: var(--fg-dim);
}

.switch {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
  color: var(--fg-dim);
  cursor: pointer;
  flex: 0 0 auto;
}

.switch input {
  width: 14px;
  height: 14px;
  margin: 0;
  accent-color: var(--accent);
  cursor: pointer;
}

.switch input:disabled {
  opacity: 0.6;
  cursor: default;
}

.plugin-desc {
  margin: 0;
  font-size: 12px;
  color: var(--fg-2);
  line-height: 1.55;
  overflow-wrap: anywhere;
}

.meta {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.meta-row {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
  font-size: 11px;
}

.meta-label {
  color: var(--fg-faint);
  flex: 0 0 auto;
}

.meta-value {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--fg-dim);
  overflow-wrap: anywhere;
  word-break: break-word;
  min-width: 0;
}

.plugin-error {
  display: flex;
  align-items: baseline;
  gap: 6px;
  flex-wrap: wrap;
  padding: 7px 9px;
  border-radius: var(--r-sm);
  border: 1px solid color-mix(in srgb, var(--red) 45%, transparent);
  background: color-mix(in srgb, var(--red) 8%, transparent);
  color: var(--red);
  font-size: 11px;
  line-height: 1.5;
  overflow-wrap: anywhere;
}

.error-label {
  font-weight: 600;
}

.commands {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 0;
  border-top: 1px solid var(--border);
  padding-top: 10px;
}

.commands-head {
  display: flex;
  align-items: center;
  gap: 6px;
}

.commands-head h3 {
  margin: 0;
  font-size: 12px;
  font-weight: 600;
  color: var(--fg-2);
}

.command-count {
  font-size: 10px;
  color: var(--fg-faint);
  background: var(--bg);
  border-radius: 8px;
  padding: 0 6px;
}

.command-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}

.command-item {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 7px 9px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--bg);
  min-width: 0;
}

.command-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px 12px;
  flex-wrap: wrap;
  min-width: 0;
}

.command-copy {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.command-title {
  font-size: 12px;
  color: var(--fg-2);
  overflow-wrap: anywhere;
}

.command-id {
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--fg-faint);
  overflow-wrap: anywhere;
}

.run-button {
  color: var(--accent);
  border-color: var(--accent-line);
  background: var(--accent-soft);
  flex: 0 0 auto;
}

.run-button:hover:not(:disabled) {
  background: var(--accent);
  color: var(--accent-fg);
}

.run-button:disabled {
  opacity: 0.5;
  cursor: default;
}

.command-result {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
  border-top: 1px solid var(--border);
  padding-top: 6px;
}

.result-head {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.result-label {
  font-size: 10px;
  color: var(--fg-dim);
}

.command-result.failed .result-label {
  color: var(--red);
}

.result-body {
  margin: 0;
  padding: 7px 9px;
  border: 1px solid var(--border);
  border-radius: var(--r-xs);
  background: var(--panel-2);
  color: var(--fg-2);
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.5;
  max-height: 220px;
  overflow: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  min-width: 0;
}

.command-result.failed .result-body {
  color: var(--red);
  border-color: color-mix(in srgb, var(--red) 45%, transparent);
}

.empty {
  color: var(--fg-dim);
  font-size: 12px;
  padding: 14px 6px;
  text-align: center;
}

.empty.small {
  padding: 8px 4px;
  text-align: left;
}

.empty-card {
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: var(--r);
}

button {
  font: inherit;
}

.ghost {
  height: 28px;
  padding: 0 10px;
  border: 1px solid var(--border);
  border-radius: var(--r-xs);
  background: transparent;
  color: var(--fg-2);
  font-size: 11px;
  cursor: pointer;
}

.ghost:hover:not(:disabled) {
  background: var(--panel-3);
  color: var(--fg);
}

.ghost.small {
  height: 26px;
  padding: 0 9px;
}

.ghost:disabled {
  opacity: 0.5;
  cursor: default;
}

.tabs {
  display: flex;
  align-items: center;
  gap: 4px;
  border-bottom: 1px solid var(--border);
}

.tab {
  height: 30px;
  padding: 0 12px;
  border: none;
  border-bottom: 2px solid transparent;
  background: transparent;
  color: var(--fg-dim);
  font-size: 12px;
  cursor: pointer;
}

.tab:hover {
  color: var(--fg);
}

.tab.active {
  color: var(--fg);
  border-bottom-color: var(--accent);
  font-weight: 600;
}

.browse {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
}

.search-bar {
  display: flex;
  min-width: 0;
}

.search-input {
  flex: 1;
  min-width: 0;
  height: 30px;
  padding: 0 10px;
  border: 1px solid var(--border);
  border-radius: var(--r-xs);
  background: var(--panel-2);
  color: var(--fg);
  font: inherit;
  font-size: 12px;
}

.search-input::placeholder {
  color: var(--fg-faint);
}

.search-input:focus {
  outline: none;
  border-color: var(--accent-line);
}

.market-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
}

.market-badge {
  font-size: 10px;
  line-height: 1.6;
  padding: 0 7px;
  border-radius: 999px;
  border: 1px solid var(--border);
  color: var(--fg-dim);
  background: var(--panel-3);
  white-space: nowrap;
}

.badge-unsigned {
  color: var(--yellow);
  border-color: color-mix(in srgb, var(--yellow) 45%, transparent);
  background: color-mix(in srgb, var(--yellow) 12%, transparent);
}

.badge-fileSource {
  color: var(--fg-2);
  border-style: dashed;
}

.badge-update {
  color: var(--accent);
  border-color: var(--accent-line);
  background: var(--accent-soft);
}

.market-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  flex: 0 0 auto;
}

.installed-mark {
  font-size: 11px;
  color: var(--fg-dim);
  white-space: nowrap;
}

.empty-title {
  margin: 0 0 6px;
  font-size: 13px;
  color: var(--fg-2);
}

.empty-detail {
  margin: 0 0 10px;
  font-size: 12px;
  color: var(--fg-dim);
  overflow-wrap: anywhere;
}

.modal-mask {
  position: fixed;
  inset: 0;
  z-index: 90;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background: rgb(0 0 0 / 45%);
}

.confirm-dialog {
  width: min(520px, calc(100vw - 48px));
  max-height: calc(100vh - 64px);
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  border: 1px solid var(--border);
  border-radius: var(--r);
  background: var(--panel-2);
  outline: none;
}

.confirm-title {
  margin: 0;
  font-size: 14px;
  font-weight: 650;
  color: var(--fg);
}

.confirm-meta {
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--bg);
}

.node-warning {
  margin: 0;
  padding: 8px 10px;
  border-radius: var(--r-sm);
  border: 1px solid color-mix(in srgb, var(--yellow) 45%, transparent);
  background: color-mix(in srgb, var(--yellow) 8%, transparent);
  color: var(--yellow);
  font-size: 12px;
  line-height: 1.5;
}

.confirm-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
}

.confirm-proceed {
  color: var(--accent);
  border-color: var(--accent-line);
  background: var(--accent-soft);
}

.confirm-proceed:hover:not(:disabled) {
  background: var(--accent);
  color: var(--accent-fg);
}

@media (max-width: 640px) {
  .extensions-page {
    padding: 14px 12px 24px;
  }

  .page-head {
    flex-direction: column;
  }

  .head-actions {
    width: 100%;
  }

  .plugin-head {
    flex-direction: column;
  }

  .switch {
    align-self: flex-start;
  }
}
</style>
