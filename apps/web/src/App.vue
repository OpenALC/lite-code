<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import AgentPanel from './components/AgentPanel.vue'
import CodeEditor from './components/CodeEditor.vue'
import FileTree from './components/FileTree.vue'
import ModeSwitcher from './components/ModeSwitcher.vue'
import RuntimeSettings from './components/RuntimeSettings.vue'
import TabBar from './components/TabBar.vue'
import { createAgentSession } from './services/agentSession'
import {
  agentCancel,
  agentSend,
  isTauri,
  onCoreEvent,
  openWorkspace,
  resolveApproval,
  sessionCreate,
  workspaceRoot,
} from './services/bridge'
import {
  LAYOUT_LIMITS,
  NARROW_WIDTH_PX,
  clamp,
  resolveProfile,
  type LayoutPrefs,
} from './services/layoutProfile'
import { loadLayoutPrefs, saveLayoutPrefs } from './services/layoutPrefs'
import { setMode, setWindowWidth, uiStore } from './services/uiStore'

const { t } = useI18n()

// 桥接框架无关 store 与 Vue 响应性
const ui = ref(uiStore.get())
const unsub = uiStore.subscribe(s => {
  ui.value = s
})

// P3 Agent 会话控制器：docked 与居中的 Agent 视图共享同一事件流、草稿与错误。
const agentSession = createAgentSession({
  sessionCreate,
  agentSend,
  agentCancel,
  resolveApproval,
  onCoreEvent,
})

const layout = ref<LayoutPrefs>(loadLayoutPrefs())
const profile = computed(() =>
  resolveProfile(ui.value.mode, ui.value.windowWidth, layout.value),
)
watch(layout, prefs => saveLayoutPrefs(prefs), { deep: true })

type ResizeTarget = 'sidebar' | 'agent' | 'bottom'

function startResize(target: ResizeTarget, event: PointerEvent): void {
  event.preventDefault()
  const startX = event.clientX
  const startY = event.clientY
  const start = { ...layout.value }

  const move = (e: PointerEvent): void => {
    if (target === 'sidebar') {
      layout.value = {
        ...layout.value,
        sidebarWidth: clamp(
          start.sidebarWidth + e.clientX - startX,
          LAYOUT_LIMITS.sidebar[0],
          LAYOUT_LIMITS.sidebar[1],
        ),
      }
    } else if (target === 'agent') {
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

function toggleAgent(): void {
  layout.value = { ...layout.value, agentCollapsed: !layout.value.agentCollapsed }
}

function toggleBottom(): void {
  layout.value = { ...layout.value, bottomCollapsed: !layout.value.bottomCollapsed }
}

// 工作区根由 Rust 侧经 workspace_root 注入（传输层安全边界，docs/PLAN.md R3）；
// 浏览器 dev 无 Tauri，可用 ?root=<路径> 作为占位数据前缀。
const treeCollapsed = ref(false)
const showRuntime = ref(false)
const showOpen = ref(false)
const workspaceInput = ref('')
const workspaceError = ref<string | null>(null)
const openingWorkspace = ref(false)
const projectRoot = ref(new URLSearchParams(window.location.search).get('root') ?? '')

async function submitOpenWorkspace(): Promise<void> {
  const path = workspaceInput.value.trim()
  if (!path || openingWorkspace.value) return
  openingWorkspace.value = true
  workspaceError.value = null
  const res = await openWorkspace(path)
  openingWorkspace.value = false
  if (!res.ok) {
    workspaceError.value = res.error.message
    return
  }
  if (res.data.root) {
    projectRoot.value = res.data.root
    showOpen.value = false
    workspaceInput.value = ''
  }
}

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
  agentSession.dispose()
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
        @click="showOpen = true"
      >
        {{ t('workspace.open') }}
      </button>
      <button
        type="button"
        class="runtime-btn"
        @click="showRuntime = true"
      >
        {{ t('runtime.title') }}
      </button>
      <ModeSwitcher />
    </header>

    <div
      v-if="showOpen"
      class="workspace-open"
    >
      <input
        v-model="workspaceInput"
        class="workspace-input"
        :placeholder="t('workspace.placeholder')"
        spellcheck="false"
        @keydown.enter="submitOpenWorkspace"
      >
      <button
        type="button"
        class="workspace-action"
        :disabled="openingWorkspace || workspaceInput.trim().length === 0"
        @click="submitOpenWorkspace"
      >
        {{ t('workspace.action') }}
      </button>
      <button
        type="button"
        class="workspace-cancel"
        @click="showOpen = false; workspaceError = null"
      >
        {{ t('workspace.cancel') }}
      </button>
      <div
        v-if="workspaceError"
        class="workspace-error"
      >
        {{ t('workspace.error') }}: {{ workspaceError }}
      </div>
    </div>

    <div class="main">
      <!-- 编辑器模式：可折叠、可拖宽的文件树面板（docs/UI.md §3） -->
      <aside
        v-if="profile.sidebarVisible"
        class="side"
        :style="{ width: `${profile.sidebarWidth}px` }"
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
      <div
        v-if="profile.sidebarVisible"
        class="resizer v"
        role="separator"
        aria-orientation="vertical"
        @pointerdown="startResize('sidebar', $event)"
      />
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
        <AgentPanel
          v-else
          :controller="agentSession"
        />
        <div
          v-if="profile.bottomPanelVisible"
          class="resizer h"
          role="separator"
          aria-orientation="horizontal"
          @pointerdown="startResize('bottom', $event)"
        />
        <div
          v-if="profile.bottomPanelVisible"
          class="bottom-panel"
          :style="{ height: `${profile.bottomHeight}px` }"
        >
          <div class="bottom-tabs">
            <span class="bottom-tab active">{{ t('bottom.terminal') }}</span>
            <span class="bottom-tab">{{ t('bottom.output') }}</span>
            <span class="bottom-tab">{{ t('bottom.problems') }}</span>
            <button
              type="button"
              class="bottom-collapse"
              :title="t('bottom.collapse')"
              @click="toggleBottom"
            >
              ▾
            </button>
          </div>
          <div class="bottom-body">
            —
          </div>
        </div>
      </section>

      <div
        v-if="profile.agentPanel === 'docked'"
        class="resizer v"
        role="separator"
        aria-orientation="vertical"
        @pointerdown="startResize('agent', $event)"
      />
      <aside
        class="agent-panel"
        :class="profile.agentPanel"
        :style="profile.agentPanel === 'docked' ? { width: `${profile.agentWidth}px` } : undefined"
      >
        <template v-if="profile.agentPanel === 'docked'">
          <div class="agent-head">
            <span>{{ t('agent.panelTitle') }}</span>
            <button
              type="button"
              class="agent-toggle"
              :title="t('agent.collapse')"
              @click="toggleAgent"
            >
              ×
            </button>
          </div>
          <AgentPanel :controller="agentSession" />
        </template>
        <template v-else>
          <button
            type="button"
            class="agent-icon"
            :title="t('agent.expand')"
            :disabled="ui.windowWidth < NARROW_WIDTH_PX"
            @click="toggleAgent"
          >
            A
          </button>
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
      <button
        v-if="profile.mode === 'editor' && !profile.bottomPanelVisible"
        type="button"
        class="status-btn"
        @click="toggleBottom"
      >
        {{ t('bottom.show') }}
      </button>
    </footer>
  </div>
</template>

<style scoped>
.shell {
  position: relative;
  height: 100%;
  display: flex;
  flex-direction: column;
  background: var(--bg);
}

.workspace-open {
  position: absolute;
  top: 40px;
  left: 12px;
  z-index: 40;
  width: min(560px, calc(100vw - 24px));
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 8px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  box-shadow: 0 10px 28px rgb(0 0 0 / 35%);
}

.workspace-input {
  flex: 1;
  min-width: 220px;
  height: 26px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--bg);
  color: var(--fg);
  font-family: var(--font-mono);
  font-size: 11px;
}

.workspace-input:focus {
  border-color: var(--accent);
  outline: none;
}

.workspace-action,
.workspace-cancel {
  height: 26px;
  padding: 0 10px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-3);
  color: var(--fg-2);
  font-size: 11px;
}

.workspace-action:hover:not(:disabled),
.workspace-cancel:hover {
  color: var(--fg);
  border-color: var(--accent);
}

.workspace-action:disabled {
  color: var(--fg-faint);
}

.workspace-error {
  flex-basis: 100%;
  color: #e06c75;
  font-size: 11px;
  line-height: 1.4;
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
  min-width: 0;
  display: flex;
  flex-direction: column;
  background: var(--panel);
  border-right: 1px solid var(--border);
}

.resizer {
  flex: 0 0 auto;
  z-index: 5;
  background: transparent;
}

.resizer.v {
  width: 4px;
  cursor: col-resize;
}

.resizer.h {
  height: 4px;
  cursor: row-resize;
}

.resizer:hover {
  background: var(--accent);
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

.bottom-collapse {
  margin-left: auto;
  color: var(--fg-faint);
  font-size: 10px;
}

.bottom-collapse:hover {
  color: var(--fg);
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
  flex-direction: column;
  min-width: 0;
}

.agent-panel.icon {
  width: 40px;
  align-items: center;
  justify-content: center;
}

.agent-head {
  display: flex;
  align-items: center;
  height: 28px;
  padding: 0 8px;
  border-bottom: 1px solid var(--border);
  font-size: 11px;
  color: var(--fg-dim);
}

.agent-head span {
  flex: 1;
}

.agent-toggle {
  width: 20px;
  height: 20px;
  color: var(--fg-faint);
  font-size: 14px;
  line-height: 1;
}

.agent-toggle:hover {
  color: var(--fg);
}

.agent-icon {
  color: var(--accent);
  font-weight: 600;
  background: none;
  border: 0;
  cursor: pointer;
}

.agent-icon:disabled {
  cursor: default;
  opacity: 0.5;
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

.status-btn {
  margin-left: auto;
  color: var(--accent);
  font-size: 11px;
}

.status-btn:hover {
  text-decoration: underline;
}
</style>
