<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import AgentPlaceholder from './components/AgentPlaceholder.vue'
import CodeEditor from './components/CodeEditor.vue'
import FileTree from './components/FileTree.vue'
import ModeSwitcher from './components/ModeSwitcher.vue'
import RuntimeSettings from './components/RuntimeSettings.vue'
import TabBar from './components/TabBar.vue'
import { isTauri, workspaceRoot } from './services/bridge'
import { resolveProfile } from './services/layoutProfile'
import { setMode, setWindowWidth, uiStore } from './services/uiStore'

const { t } = useI18n()

// 桥接框架无关 store 与 Vue 响应性
const ui = ref(uiStore.get())
const unsub = uiStore.subscribe(s => {
  ui.value = s
})

const profile = computed(() => resolveProfile(ui.value.mode, ui.value.windowWidth))

// 工作区根由 Rust 侧经 workspace_root 注入（传输层安全边界，docs/PLAN.md R3）；
// 浏览器 dev 无 Tauri，可用 ?root=<路径> 作为占位数据前缀。
const treeCollapsed = ref(false)
const showRuntime = ref(false)
const projectRoot = ref(new URLSearchParams(window.location.search).get('root') ?? '')

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
  void workspaceRoot().then(res => {
    if (res.ok && res.data.root) {
      projectRoot.value = res.data.root
    } else if (!isTauri() && projectRoot.value === '') {
      // 浏览器 dev 没有真实工作区；用哨兵根加载占位树，避免永远“读取中”。
      projectRoot.value = 'workspace'
    }
  })
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
      <button
        type="button"
        class="runtime-btn"
        @click="showRuntime = true"
      >
        {{ t('runtime.title') }}
      </button>
      <ModeSwitcher />
    </header>

    <div class="main">
      <!-- 编辑器模式：可折叠文件树面板（docs/UI.md §3）；Agent 模式保留占位 rail -->
      <aside
        v-if="profile.mode === 'editor'"
        class="side"
      >
        <div class="side-head">
          <button
            type="button"
            class="side-toggle"
            :title="t('tree.toggle')"
            :aria-expanded="!treeCollapsed"
            @click="treeCollapsed = !treeCollapsed"
          >
            {{ treeCollapsed ? '▸' : '▾' }}
          </button>
          <span
            v-if="!treeCollapsed"
            class="side-title"
          >{{ t('tree.title') }}</span>
        </div>
        <FileTree
          v-if="!treeCollapsed"
          :root="projectRoot"
        />
      </aside>
      <aside
        v-else
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

    <RuntimeSettings
      v-if="showRuntime"
      @close="showRuntime = false"
    />

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

.runtime-btn {
  height: 22px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-3);
  color: var(--fg-dim);
  font-size: 11px;
}

.runtime-btn:hover {
  color: var(--fg);
  border-color: var(--accent);
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

.side {
  width: 220px;
  min-width: 0;
  display: flex;
  flex-direction: column;
  background: var(--panel);
  border-right: 1px solid var(--border);
}

.side-head {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 28px;
  padding: 0 8px;
  border-bottom: 1px solid var(--border);
}

.side-toggle {
  width: 18px;
  height: 18px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--r-sm);
  color: var(--fg-faint);
  font-size: 10px;
}

.side-toggle:hover {
  color: var(--fg);
  background: var(--panel-3);
}

.side-title {
  font-size: 11px;
  color: var(--fg-dim);
  letter-spacing: 0.04em;
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
