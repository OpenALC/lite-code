<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import AgentPlaceholder from './components/AgentPlaceholder.vue'
import CodeEditor from './components/CodeEditor.vue'
import ModeSwitcher from './components/ModeSwitcher.vue'
import TabBar from './components/TabBar.vue'
import { resolveProfile } from './services/layoutProfile'
import { setMode, setWindowWidth, uiStore } from './services/uiStore'

const { t } = useI18n()

// 桥接框架无关 store 与 Vue 响应性
const ui = ref(uiStore.get())
const unsub = uiStore.subscribe(s => {
  ui.value = s
})

const profile = computed(() => resolveProfile(ui.value.mode, ui.value.windowWidth))

function onResize(): void {
  setWindowWidth(window.innerWidth)
}

function onKey(e: KeyboardEvent): void {
  // P3 将实现编辑器 ↔ Agent 无损切换，P0 先接上快捷键骨架
  if (e.ctrlKey && e.shiftKey && (e.key === 'M' || e.key === 'm')) {
    setMode(ui.value.mode === 'editor' ? 'agent' : 'editor')
  }
}

onMounted(() => {
  setWindowWidth(window.innerWidth)
  window.addEventListener('resize', onResize)
  window.addEventListener('keydown', onKey)
})

onBeforeUnmount(() => {
  unsub()
  window.removeEventListener('resize', onResize)
  window.removeEventListener('keydown', onKey)
})
</script>

<template>
  <div
    class="shell"
    :class="`mode-${profile.mode}`"
  >
    <header class="topbar">
      <div class="brand">
        {{ t('app.title') }}
      </div>
      <div class="spacer" />
      <ModeSwitcher />
    </header>

    <div class="main">
      <aside
        class="rail"
        aria-hidden="true"
      >
        LC
      </aside>

      <section class="center">
        <TabBar v-if="profile.tabsVisible" />
        <CodeEditor v-if="profile.centerView === 'editor'" />
        <AgentPlaceholder v-else />
        <div
          v-if="profile.bottomPanelVisible"
          class="bottom-panel"
        >
          <div class="bottom-tabs">
            <span class="bottom-tab active">{{ t('bottom.terminal') }}</span>
            <span class="bottom-tab">{{ t('bottom.output') }}</span>
            <span class="bottom-tab">{{ t('bottom.problems') }}</span>
          </div>
          <div class="bottom-body">
            —
          </div>
        </div>
      </section>

      <aside
        class="agent-panel"
        :class="profile.agentPanel"
      >
        <template v-if="profile.agentPanel === 'docked'">
          <AgentPlaceholder />
        </template>
        <template v-else>
          <div
            class="agent-icon"
            :title="t('agent.placeholderTitle')"
          >
            A
          </div>
        </template>
      </aside>
    </div>

    <footer class="statusbar">
      <span class="item">{{ t('status.branch') }}</span>
      <span class="item dim">Node —</span>
      <span class="item dim">MCP 0</span>
      <span class="item dim">{{ t('status.sessions') }}</span>
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

.topbar {
  display: flex;
  align-items: center;
  gap: 12px;
  height: 36px;
  padding: 0 12px;
  background: var(--panel);
  border-bottom: 1px solid var(--border);
}

.brand {
  font-weight: 600;
  color: var(--fg-2);
  font-size: 12px;
  letter-spacing: 0.04em;
}

.spacer {
  flex: 1;
}

.main {
  flex: 1;
  min-height: 0;
  display: flex;
}

.rail {
  width: 40px;
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding-top: 10px;
  background: var(--panel);
  border-right: 1px solid var(--border);
  color: var(--fg-faint);
  font-family: var(--font-mono);
  font-size: 11px;
}

.center {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
}

.bottom-panel {
  height: 180px;
  display: flex;
  flex-direction: column;
  border-top: 1px solid var(--border);
  background: var(--panel);
}

.bottom-tabs {
  display: flex;
  gap: 12px;
  padding: 0 12px;
  height: 26px;
  align-items: center;
  border-bottom: 1px solid var(--border);
}

.bottom-tab {
  font-size: 12px;
  color: var(--fg-dim);
}

.bottom-tab.active {
  color: var(--accent);
}

.bottom-body {
  flex: 1;
  padding: 8px 12px;
  color: var(--fg-faint);
  font-family: var(--font-mono);
  font-size: 12px;
}

.agent-panel {
  background: var(--panel);
  border-left: 1px solid var(--border);
  display: flex;
  min-width: 0;
}

.agent-panel.docked {
  width: 400px;
}

.agent-panel.icon {
  width: 40px;
  align-items: center;
  justify-content: center;
}

.agent-icon {
  color: var(--accent);
  font-weight: 600;
}

.statusbar {
  display: flex;
  gap: 16px;
  height: 24px;
  align-items: center;
  padding: 0 12px;
  background: var(--panel);
  border-top: 1px solid var(--border);
  font-size: 11px;
  color: var(--fg-2);
}

.item.dim {
  color: var(--fg-faint);
}
</style>
