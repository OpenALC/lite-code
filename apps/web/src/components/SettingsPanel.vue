<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'

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
const emit = defineEmits<{ 'manage-providers': [] }>()
const prefs = ref(uiPrefs())
const unsub = subscribePrefs(next => {
  prefs.value = next
})
onBeforeUnmount(unsub)

const zoomLabel = computed(() => `${Math.round(prefs.value.zoom * 100)}%`)
const fontSizeLabel = computed(() => `${prefs.value.fontSize} px`)
const agentFontLabel = computed(() => `${prefs.value.agentFontSize} px`)

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
    <section class="card">
      <h3>{{ t('settings.appearance') }}</h3>
      <div class="row">
        <span>{{ t('settings.theme') }}</span>
        <div class="segmented">
          <button
            type="button"
            :class="{ active: prefs.theme === 'dark' }"
            @click="setTheme('dark')"
          >
            {{ t('settings.dark') }}
          </button>
          <button
            type="button"
            :class="{ active: prefs.theme === 'light' }"
            @click="setTheme('light')"
          >
            {{ t('settings.light') }}
          </button>
        </div>
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

    <section class="card">
      <h3>{{ t('settings.editor') }}</h3>
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

    <section class="card">
      <h3>{{ t('settings.agent') }}</h3>
      <p class="hint">
        {{ t('settings.agentHint') }}
      </p>
      <code class="path">~/.lite-code/providers.json</code>
      <button
        type="button"
        class="manage"
        @click="emit('manage-providers')"
      >
        {{ t('settings.manageProviders') }}
      </button>
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
      <div class="row">
        <span>{{ t('settings.db') }}</span>
        <span class="muted">SQLite · PostgreSQL · MySQL · SQL Server</span>
      </div>
      <div class="row">
        <span>MCP</span>
        <span class="muted">stdio · {{ t('settings.projectOptIn') }}</span>
      </div>
    </section>

    <section class="card">
      <h3>{{ t('settings.shortcuts') }}</h3>
      <div
        v-for="item in shortcuts"
        :key="item.keys"
        class="row"
      >
        <span>{{ item.label }}</span>
        <kbd>{{ item.keys }}</kbd>
      </div>
    </section>

    <section class="card">
      <h3>{{ t('settings.about') }}</h3>
      <div class="row">
        <span>Lite Code</span>
        <code>{{ t('settings.version') }} {{ appVersion }}</code>
      </div>
      <p class="hint">
        {{ t('settings.aboutHint') }}
      </p>
    </section>
  </div>
</template>

<style scoped>
.settings {
  height: 100%;
  overflow: auto;
  padding: 18px 20px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  width: 100%;
  max-width: 920px;
  margin: 0 auto;
}

.card {
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: var(--r);
  padding: 12px;
}

h3 {
  margin: 0 0 10px;
  font-size: 12px;
  font-weight: 600;
  color: var(--fg-2);
  letter-spacing: 0.02em;
}

.row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 6px 12px;
  min-height: 30px;
  font-size: 12px;
  color: var(--fg-2);
}

.row + .row {
  border-top: 1px solid var(--border);
}

code {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--fg-dim);
}

.path {
  display: block;
  margin-top: 6px;
  padding: 6px 8px;
  background: var(--bg);
  border-radius: var(--r-sm);
  overflow-wrap: anywhere;
}

.hint {
  margin: 0;
  font-size: 11px;
  line-height: 1.5;
  color: var(--fg-dim);
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

.muted {
  color: var(--fg-dim);
  font-size: 11px;
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

.segmented.compact button {
  min-width: 38px;
}

.segmented button.active,
.zoom-value,
.stepper-value {
  background: var(--accent-soft);
  color: var(--accent);
}

.stepper button:disabled {
  opacity: 0.4;
  cursor: default;
}

.ghost {
  color: var(--fg-dim);
  font-size: 11px;
}

.manage {
  align-self: flex-start;
  height: 28px;
  padding: 0 10px;
  border: 1px solid var(--accent);
  border-radius: var(--r-sm);
  background: var(--accent-soft, transparent);
  color: var(--accent);
  font-size: 11px;
  cursor: pointer;
}

.manage:hover {
  background: var(--accent);
  color: var(--accent-fg, #fff);
}

select {
  height: 26px;
  padding: 0 8px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  font-size: 11px;
}
</style>
