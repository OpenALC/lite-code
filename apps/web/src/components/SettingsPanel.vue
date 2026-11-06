<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  resetZoom,
  setLocale,
  setTheme,
  subscribePrefs,
  toggleTheme,
  uiPrefs,
  zoomBy,
} from '../services/themeStore'

const { t, locale } = useI18n()
const prefs = ref(uiPrefs())
const unsub = subscribePrefs(next => {
  prefs.value = next
})
onBeforeUnmount(unsub)

const zoomLabel = computed(() => `${Math.round(prefs.value.zoom * 100)}%`)

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
        <code>JetBrains Mono / 13px</code>
      </div>
      <div class="row">
        <span>{{ t('settings.tabSize') }}</span>
        <code>2</code>
      </div>
      <div class="row">
        <span>{{ t('settings.wordWrap') }}</span>
        <code>off</code>
      </div>
    </section>

    <section class="card">
      <h3>{{ t('settings.agent') }}</h3>
      <p class="hint">
        {{ t('settings.agentHint') }}
      </p>
      <code class="path">~/.lite-code/providers.json</code>
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
      <h3>{{ t('settings.about') }}</h3>
      <div class="row">
        <span>Lite Code</span>
        <code>{{ t('settings.version') }} 0.6.1-preview.3</code>
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
  padding: 12px;
  display: flex;
  flex-direction: column;
  gap: 10px;
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

.muted {
  color: var(--fg-dim);
  font-size: 11px;
}

.segmented,
.zoom {
  display: flex;
  align-items: center;
  padding: 2px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
}

.segmented button,
.zoom button {
  height: 24px;
  min-width: 54px;
  padding: 0 8px;
  border-radius: var(--r-xs);
  color: var(--fg-dim);
  font-size: 11px;
}

.zoom button {
  min-width: 26px;
}

.segmented button.active,
.zoom-value {
  background: var(--accent-soft);
  color: var(--accent);
}

.ghost {
  color: var(--fg-dim);
  font-size: 11px;
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
