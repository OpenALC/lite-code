<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import AgentPanel from '../components/AgentPanel.vue'
import AppIcon from '../components/AppIcon.vue'
import FileTree from '../components/FileTree.vue'
import ProvidersPanel from '../components/ProvidersPanel.vue'
import { createAgentSession } from '../services/agentSession'
import { createHistoryRecorder } from '../services/agentHistory'
import {
  agentCancel,
  agentSend,
  onCoreEvent,
  resolveApproval,
  sessionCreate,
  workspaceRoot,
} from '../services/bridge'
import { basenamePath } from '../services/fileTree'
import { activeTab, tabsStore, type Tab } from '../services/tabs'
import {
  initUiPrefs,
  subscribePrefs,
  toggleTheme,
  uiPrefs,
} from '../services/themeStore'
import ReadonlyCodeView from './ReadonlyCodeView.vue'

type ShellView = 'files' | 'preview' | 'chat'

const { t, locale } = useI18n()
locale.value = uiPrefs().locale

const view = ref<ShellView>('files')
const root = ref('')
const rootError = ref('')
const treeRefresh = ref(0)
const theme = ref(uiPrefs().theme)
const active = ref<Tab | null>(activeTab(tabsStore.get()))
const providersOpen = ref(false)

const agentSession = createAgentSession({
  sessionCreate: (mode, model, thinking) => sessionCreate(mode, null, model, thinking),
  agentSend,
  agentCancel,
  resolveApproval,
  onCoreEvent,
})
const historyRecorder = createHistoryRecorder(agentSession, () => root.value)

const projectName = computed(() =>
  root.value ? basenamePath(root.value) : t('tree.noRoot'),
)
const themeLabel = computed(() =>
  theme.value === 'dark' ? t('settings.light') : t('settings.dark'),
)

const unsubscribeTabs = tabsStore.subscribe(next => {
  const previousId = active.value?.id ?? null
  active.value = activeTab(next)
  if (active.value && active.value.id !== previousId) view.value = 'preview'
})
let unsubscribePrefs: (() => void) | null = null

async function loadRoot(): Promise<void> {
  rootError.value = ''
  const res = await workspaceRoot()
  if (res.ok) root.value = res.data.root ?? ''
  else rootError.value = `${res.error.code}: ${res.error.message}`
}

function refresh(): void {
  treeRefresh.value += 1
  void loadRoot()
}

function switchTheme(): void {
  toggleTheme()
}

onMounted(() => {
  initUiPrefs()
  unsubscribePrefs = subscribePrefs(next => {
    theme.value = next.theme
    locale.value = next.locale
  })
  void loadRoot()
})

onBeforeUnmount(() => {
  unsubscribeTabs()
  unsubscribePrefs?.()
  unsubscribePrefs = null
  historyRecorder.dispose()
  agentSession.dispose()
})
</script>

<template>
  <div class="ohos-shell">
    <header class="ohos-top">
      <span class="ohos-brand">{{ t('app.title') }}</span>
      <span
        class="ohos-project"
        :title="root"
      >{{ projectName }}</span>
      <span class="spacer" />
      <button
        type="button"
        class="ohos-icon"
        :title="t('providers.title')"
        :aria-label="t('providers.title')"
        @click="providersOpen = true"
      >
        <AppIcon
          name="providers"
          :size="15"
        />
      </button>
      <button
        type="button"
        class="ohos-icon"
        :title="t('explorer.refresh')"
        :aria-label="t('explorer.refresh')"
        @click="refresh"
      >
        <AppIcon
          name="refresh"
          :size="15"
        />
      </button>
      <button
        type="button"
        class="ohos-icon"
        :title="themeLabel"
        :aria-label="themeLabel"
        @click="switchTheme"
      >
        <AppIcon
          :name="theme === 'dark' ? 'sun' : 'moon'"
          :size="15"
        />
      </button>
    </header>

    <p
      v-if="rootError"
      class="ohos-error"
    >
      {{ rootError }}
    </p>

    <div class="ohos-body">
      <aside
        class="ohos-pane files"
        :class="{ 'is-active': view === 'files' }"
      >
        <FileTree
          :root="root"
          :refresh-token="treeRefresh"
        />
      </aside>
      <section
        class="ohos-pane preview"
        :class="{ 'is-active': view === 'preview' }"
      >
        <ReadonlyCodeView />
      </section>
      <section
        class="ohos-pane chat"
        :class="{ 'is-active': view === 'chat' }"
      >
        <AgentPanel
          :controller="agentSession"
          :workspace="root"
          force-mode="plan"
        />
      </section>
    </div>

    <nav
      class="ohos-nav"
      :aria-label="t('app.title')"
    >
      <button
        type="button"
        :class="{ active: view === 'files' }"
        @click="view = 'files'"
      >
        <AppIcon
          name="file"
          :size="17"
        />
        <span>{{ t('tree.title') }}</span>
      </button>
      <button
        type="button"
        :class="{ active: view === 'preview' }"
        @click="view = 'preview'"
      >
        <AppIcon
          name="preview"
          :size="17"
        />
        <span>{{ t('agent.previewTitle') }}</span>
      </button>
      <button
        type="button"
        :class="{ active: view === 'chat' }"
        @click="view = 'chat'"
      >
        <AppIcon
          name="agent"
          :size="17"
        />
        <span>{{ t('agent.conversations') }}</span>
      </button>
    </nav>

    <section
      v-if="providersOpen"
      class="ohos-sheet"
      role="dialog"
      aria-modal="true"
      :aria-label="t('providers.title')"
    >
      <header class="ohos-sheet-head">
        <button
          type="button"
          class="ohos-icon"
          :title="t('titlebar.close')"
          :aria-label="t('titlebar.close')"
          @click="providersOpen = false"
        >
          <AppIcon
            class="ohos-back"
            name="chevron"
            :size="15"
          />
        </button>
      </header>
      <div class="ohos-sheet-body">
        <ProvidersPanel />
      </div>
    </section>
  </div>
</template>
