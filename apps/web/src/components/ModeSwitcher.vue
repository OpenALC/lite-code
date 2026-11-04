<script setup lang="ts">
import { onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { UiMode } from '../services/layoutProfile'
import { setMode, uiStore } from '../services/uiStore'

const { t } = useI18n()
const ui = ref(uiStore.get())
const unsub = uiStore.subscribe(s => {
  ui.value = s
})
onBeforeUnmount(unsub)

const modes: UiMode[] = ['editor', 'agent']
</script>

<template>
  <div
    class="mode-switcher"
    role="radiogroup"
    :aria-label="t('mode.hint')"
  >
    <button
      v-for="m in modes"
      :key="m"
      type="button"
      role="radio"
      :aria-checked="ui.mode === m"
      class="mode-btn"
      :class="{ active: ui.mode === m, agent: m === 'agent' }"
      @click="setMode(m)"
    >
      {{ t(`mode.${m}`) }}
    </button>
  </div>
</template>

<style scoped>
.mode-switcher {
  display: inline-flex;
  gap: 2px;
  padding: 2px;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
}

.mode-btn {
  padding: 3px 10px;
  border-radius: 3px;
  color: var(--fg-dim);
  transition: color var(--dur) var(--ease), background var(--dur) var(--ease);
}

.mode-btn:hover {
  color: var(--fg);
}

.mode-btn.active {
  color: var(--accent-fg);
  background: var(--accent);
}

/* 完全自主类模式需要视觉警示——这里 agent 模式只是占位，用描边示意 */
.mode-btn.agent.active {
  color: var(--accent);
  background: var(--accent-soft);
  outline: 1px solid var(--accent-line);
}
</style>
