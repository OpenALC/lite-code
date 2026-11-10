<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import AppIcon from './AppIcon.vue'
import ProvidersPanel from './ProvidersPanel.vue'
import {
  pluginThemeOptions,
  subscribePluginThemes,
  type PluginThemeOption,
} from '../services/pluginThemes'
import {
  AGENT_FONT_DEFAULT,
  AGENT_FONT_MAX,
  AGENT_FONT_MIN,
  FONT_SIZE_DEFAULT,
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  TAB_SIZES,
  resetZoom,
  setAgentDensity,
  setAgentFontSize,
  setAgentSendKey,
  setFontSize,
  setLocale,
  setTabSize,
  setTheme,
  setWordWrap,
  subscribePrefs,
  toggleTheme,
  uiPrefs,
  zoomBy,
} from '../services/themeStore'

const appVersion = __APP_VERSION__
const { t, locale } = useI18n()
const prefs = ref(uiPrefs())
const unsub = subscribePrefs(next => {
  prefs.value = next
})
onBeforeUnmount(unsub)

// 插件贡献的主题（docs/EXTENSIONS.md §1）；App 在启动与插件变更时同步。
const pluginThemes = ref<PluginThemeOption[]>(pluginThemeOptions())
const unsubThemes = subscribePluginThemes(next => {
  pluginThemes.value = next
})
onBeforeUnmount(unsubThemes)

function changeTheme(value: string): void {
  setTheme(value)
}

const zoomLabel = computed(() => `${Math.round(prefs.value.zoom * 100)}%`)
const fontSizeLabel = computed(() => `${prefs.value.fontSize} px`)
const agentFontLabel = computed(() => `${prefs.value.agentFontSize} px`)

export type SettingsSection = 'general' | 'editor' | 'agent' | 'providers' | 'shortcuts' | 'about'
const props = withDefaults(defineProps<{ initialSection?: SettingsSection }>(), {
  initialSection: 'general',
})
const activeSection = ref<SettingsSection>(props.initialSection)
const sections = computed<{ id: SettingsSection; label: string; icon: string }[]>(() => [
  { id: 'general', label: t('settings.general'), icon: 'settings' },
  { id: 'editor', label: t('settings.editor'), icon: 'code' },
  { id: 'agent', label: t('settings.agent'), icon: 'agent' },
  { id: 'providers', label: t('providers.title'), icon: 'providers' },
  { id: 'shortcuts', label: t('settings.shortcuts'), icon: 'keyboard' },
  { id: 'about', label: t('settings.about'), icon: 'info' },
])
const activeLabel = computed(
  () => sections.value.find(section => section.id === activeSection.value)?.label ?? '',
)

const shortcuts = computed(() => [
  { keys: 'Ctrl+P', label: t('settings.shortcutQuickOpen') },
  { keys: 'Ctrl+Shift+T', label: t('menu.reopenClosed') },
  { keys: 'Ctrl+W', label: t('menu.closeTab') },
  { keys: 'Ctrl+Tab', label: t('settings.shortcutNextEditor') },
  { keys: 'Ctrl+Shift+Tab', label: t('settings.shortcutPrevEditor') },
  { keys: 'Ctrl+B', label: t('settings.shortcutSidebar') },
  { keys: 'Ctrl+J', label: t('menu.toggleBottom') },
  { keys: 'Ctrl+,', label: t('activity.settings') },
  { keys: 'Ctrl+Alt+T', label: t('menu.toggleTheme') },
  { keys: 'Ctrl+= / - / 0', label: t('settings.zoom') },
])

function changeLocale(value: string): void {
  const next = value === 'en' ? 'en' : 'zh-CN'
  setLocale(next)
  locale.value = next
  document.documentElement.lang = next
}
</script>

<template>
  <div class="settings">
    <aside class="settings-nav">
      <h1>{{ t('settings.title') }}</h1>
      <div class="settings-tabs">
        <button
          v-for="section in sections"
          :key="section.id"
          type="button"
          :class="{ active: activeSection === section.id }"
          @click="activeSection = section.id"
        >
          <AppIcon
            :name="section.icon"
            :size="16"
          />
          <span>{{ section.label }}</span>
        </button>
      </div>
    </aside>

    <main class="settings-main">
      <h2>{{ activeLabel }}</h2>

      <template v-if="activeSection === 'general'">
        <section class="settings-group">
          <div class="row">
            <span>{{ t('settings.theme') }}</span>
            <select
              :value="prefs.theme"
              @change="changeTheme(($event.target as HTMLSelectElement).value)"
            >
              <option value="dark">
                {{ t('settings.dark') }}
              </option>
              <option value="light">
                {{ t('settings.light') }}
              </option>
              <optgroup
                v-if="pluginThemes.length > 0"
                :label="t('settings.pluginThemes')"
              >
                <option
                  v-for="theme in pluginThemes"
                  :key="theme.slug"
                  :value="theme.slug"
                >
                  {{ theme.label }}（{{ t('settings.pluginThemeSource', { name: theme.pluginName }) }}）
                </option>
              </optgroup>
            </select>
          </div>
          <div class="row">
            <span>{{ t('settings.zoom') }}</span>
            <div class="zoom">
              <button
                type="button"
                @click="zoomBy(-0.1)"
              >
                −
              </button>
              <button
                type="button"
                class="zoom-value"
                @click="resetZoom"
              >
                {{ zoomLabel }}
              </button>
              <button
                type="button"
                @click="zoomBy(0.1)"
              >
                +
              </button>
            </div>
          </div>
          <div class="row">
            <span>{{ t('settings.language') }}</span>
            <select
              :value="prefs.locale"
              @change="changeLocale(($event.target as HTMLSelectElement).value)"
            >
              <option value="zh-CN">
                简体中文
              </option>
              <option value="en">
                English
              </option>
            </select>
          </div>
          <div class="row">
            <span>{{ t('settings.themeShortcut') }}</span>
            <button
              type="button"
              class="ghost"
              @click="toggleTheme"
            >
              Ctrl+Alt+T
            </button>
          </div>
        </section>
      </template>

      <template v-else-if="activeSection === 'editor'">
        <section class="settings-group">
          <div class="row">
            <span>{{ t('settings.font') }}</span>
            <div class="stepper">
              <button
                type="button"
                :disabled="prefs.fontSize <= FONT_SIZE_MIN"
                @click="setFontSize(prefs.fontSize - 1)"
              >
                −
              </button>
              <button
                type="button"
                class="stepper-value"
                :title="t('settings.resetFontSize')"
                @click="setFontSize(FONT_SIZE_DEFAULT)"
              >
                {{ fontSizeLabel }}
              </button>
              <button
                type="button"
                :disabled="prefs.fontSize >= FONT_SIZE_MAX"
                @click="setFontSize(prefs.fontSize + 1)"
              >
                +
              </button>
            </div>
          </div>
          <div class="row">
            <span>{{ t('settings.tabSize') }}</span>
            <div class="segmented compact">
              <button
                v-for="size in TAB_SIZES"
                :key="size"
                type="button"
                :class="{ active: prefs.tabSize === size }"
                @click="setTabSize(size)"
              >
                {{ size }}
              </button>
            </div>
          </div>
          <div class="row">
            <span>{{ t('settings.wordWrap') }}</span>
            <div class="segmented">
              <button
                type="button"
                :class="{ active: prefs.wordWrap === 'off' }"
                @click="setWordWrap('off')"
              >
                {{ t('settings.off') }}
              </button>
              <button
                type="button"
                :class="{ active: prefs.wordWrap === 'on' }"
                @click="setWordWrap('on')"
              >
                {{ t('settings.on') }}
              </button>
            </div>
          </div>
        </section>
      </template>

      <template v-else-if="activeSection === 'agent'">
        <section class="settings-group">
          <div class="group-intro">
            <span>{{ t('settings.modelServices') }}</span>
            <p class="row-desc">
              {{ t('settings.agentHint') }}
            </p>
            <code class="path">~/.lite-code/providers.json</code>
            <button
              type="button"
              class="manage"
              @click="activeSection = 'providers'"
            >
              {{ t('settings.manageProviders') }}
            </button>
          </div>
          <div class="row">
            <span>{{ t('settings.agentSendKey') }}</span>
            <div class="segmented compact">
              <button
                type="button"
                :class="{ active: prefs.agentSendKey === 'enter' }"
                @click="setAgentSendKey('enter')"
              >
                {{ t('settings.agentSendEnter') }}
              </button>
              <button
                type="button"
                :class="{ active: prefs.agentSendKey === 'ctrl-enter' }"
                @click="setAgentSendKey('ctrl-enter')"
              >
                {{ t('settings.agentSendCtrlEnter') }}
              </button>
            </div>
          </div>
          <div class="row">
            <span>{{ t('settings.agentDensity') }}</span>
            <div class="segmented">
              <button
                type="button"
                :class="{ active: prefs.agentDensity === 'comfortable' }"
                @click="setAgentDensity('comfortable')"
              >
                {{ t('settings.agentComfortable') }}
              </button>
              <button
                type="button"
                :class="{ active: prefs.agentDensity === 'compact' }"
                @click="setAgentDensity('compact')"
              >
                {{ t('settings.agentCompact') }}
              </button>
            </div>
          </div>
          <div class="row">
            <span>{{ t('settings.agentFontSize') }}</span>
            <div class="stepper">
              <button
                type="button"
                :disabled="prefs.agentFontSize <= AGENT_FONT_MIN"
                @click="setAgentFontSize(prefs.agentFontSize - 1)"
              >
                −
              </button>
              <button
                type="button"
                :title="t('settings.resetAgentFontSize')"
                @click="setAgentFontSize(AGENT_FONT_DEFAULT)"
              >
                {{ agentFontLabel }}
              </button>
              <button
                type="button"
                :disabled="prefs.agentFontSize >= AGENT_FONT_MAX"
                @click="setAgentFontSize(prefs.agentFontSize + 1)"
              >
                +
              </button>
            </div>
          </div>
        </section>

        <section class="settings-group">
          <div class="row">
            <span>{{ t('settings.db') }}</span>
            <span class="muted">SQLite · PostgreSQL · MySQL · SQL Server</span>
          </div>
          <div class="row">
            <span>MCP</span>
            <span class="muted">stdio · {{ t('settings.projectOptIn') }}</span>
          </div>
        </section>
      </template>

      <template v-else-if="activeSection === 'providers'">
        <div class="settings-embed">
          <ProvidersPanel />
        </div>
      </template>

      <template v-else-if="activeSection === 'shortcuts'">
        <section class="settings-group">
          <div
            v-for="item in shortcuts"
            :key="item.keys"
            class="row"
          >
            <span>{{ item.label }}</span>
            <kbd>{{ item.keys }}</kbd>
          </div>
        </section>
      </template>

      <template v-else>
        <section class="settings-group">
          <div class="row">
            <span>Lite Code</span>
            <code>{{ t('settings.version') }} {{ appVersion }}</code>
          </div>
          <div class="row row-stack">
            <p class="row-desc">
              {{ t('settings.aboutHint') }}
            </p>
          </div>
        </section>
      </template>
    </main>
  </div>
</template>

<style scoped>
.settings {
  flex: 1 1 auto;
  min-width: 0;
  width: 100%;
  height: 100%;
  display: grid;
  grid-template-columns: 184px minmax(0, 1fr);
  background: var(--bg);
}

.settings-nav {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 14px 8px;
  border-right: 1px solid var(--border);
  overflow-y: auto;
}

.settings-nav h1 {
  margin: 0 10px;
  color: var(--fg);
  font-size: 14px;
  font-weight: 600;
}

.settings-tabs {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.settings-tabs button {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 34px;
  padding: 0 10px;
  border-radius: 8px;
  color: var(--fg-dim);
  font-size: 12.5px;
  text-align: left;
}

.settings-tabs button:hover {
  background: var(--panel-3);
  color: var(--fg);
}

.settings-tabs button.active {
  background: var(--accent-soft);
  color: var(--accent);
}

.settings-main {
  min-width: 0;
  overflow-y: auto;
  padding: 22px 30px 40px;
}

.settings-main h2 {
  margin: 0 0 14px;
  color: var(--fg);
  font-size: 18px;
  font-weight: 600;
}

.settings-group {
  max-width: 760px;
  margin-bottom: 16px;
  border-radius: 16px;
  background: var(--panel-2);
  overflow: hidden;
}

.row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 12px 18px;
  min-height: 52px;
  padding: 12px 16px;
  color: var(--fg-2);
  font-size: 12.5px;
}

.row > :first-child {
  flex: 1 1 150px;
  min-width: 0;
}

.row > :not(:first-child) {
  flex: 0 0 auto;
}

.row + .row {
  border-top: 1px solid var(--border);
}

.row-stack {
  display: block;
}

.group-intro {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 14px 16px;
  border-bottom: 1px solid var(--border);
  color: var(--fg-2);
  font-size: 12.5px;
}

.row-desc {
  margin: 0;
  color: var(--fg-dim);
  font-size: 11.5px;
  line-height: 1.45;
}

code {
  color: var(--fg-dim);
  font-family: var(--font-mono);
  font-size: 11px;
}

.path {
  display: block;
  padding: 6px 8px;
  border-radius: var(--r-sm);
  background: var(--bg);
  overflow-wrap: anywhere;
}

.muted {
  color: var(--fg-dim);
  font-size: 11px;
}

kbd {
  padding: 1px 6px;
  border: 1px solid var(--border);
  border-radius: var(--r-xs);
  background: var(--bg);
  color: var(--fg-dim);
  font: 10.5px/1.6 var(--font-mono);
  white-space: nowrap;
}

.ghost {
  padding: 3px 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-xs);
  color: var(--fg-dim);
  font-size: 11px;
}

.manage {
  align-self: flex-start;
  height: 28px;
  padding: 0 10px;
  border: 1px solid var(--accent-line);
  border-radius: var(--r-sm);
  color: var(--accent);
  font-size: 11.5px;
}

.segmented,
.zoom,
.stepper {
  display: flex;
  align-items: center;
  padding: 2px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
}

.segmented button,
.zoom button,
.stepper button {
  height: 24px;
  min-width: 54px;
  padding: 0 8px;
  border-radius: var(--r-xs);
  color: var(--fg-dim);
  font-size: 11px;
}

.zoom button,
.stepper button {
  min-width: 26px;
}

.segmented button.active {
  background: var(--accent-soft);
  color: var(--accent);
}

.segmented.compact button {
  min-width: 40px;
}

.zoom-value,
.stepper-value {
  min-width: 54px !important;
  color: var(--fg-2) !important;
  font-family: var(--font-mono);
}

select {
  height: 28px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--bg);
  color: var(--fg-2);
  font-size: 11.5px;
}

.settings-embed {
  flex: 1 1 auto;
  min-height: 0;
  display: flex;
}

.settings-embed :deep(.providers-page) {
  max-width: none;
  padding: 0 4px 16px;
}

@media (max-width: 760px) {
  .settings {
    display: flex;
    flex-direction: column;
  }

  .settings-nav {
    flex: none;
    flex-direction: row;
    align-items: center;
    padding: 8px 12px;
    border-right: 0;
    border-bottom: 1px solid var(--border);
    overflow-x: auto;
  }

  .settings-nav h1 {
    margin: 0 8px 0 0;
    white-space: nowrap;
  }

  .settings-tabs {
    flex-direction: row;
  }

  .settings-tabs button {
    height: 30px;
    white-space: nowrap;
  }

  .settings-main {
    padding: 16px;
  }
}
</style>
