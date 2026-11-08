<script setup lang="ts">
// 插件管理页：列表、启停、重载与命令执行（docs/EXTENSIONS.md）。
import { onMounted, ref } from 'vue'
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
