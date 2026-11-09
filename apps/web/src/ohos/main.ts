import './tauriShim'

import { createApp } from 'vue'

import { i18n } from '../i18n'
import { tabsStore } from '../services/tabs'
import { installTabsPersistence } from '../services/tabsPersistence'
import '../styles/tokens.css'
import OhosApp from './OhosApp.vue'
import './ohos.css'

// 只读外壳没有编辑器：恢复持久化标签后丢弃 P0 样例等非文件缓冲区。
installTabsPersistence()
const restored = tabsStore.get()
const fileTabs = restored.tabs.filter(tab => tab.path !== null)
if (fileTabs.length !== restored.tabs.length) {
  tabsStore.set({
    tabs: fileTabs,
    activeId: fileTabs.some(tab => tab.id === restored.activeId)
      ? restored.activeId
      : (fileTabs[0]?.id ?? null),
  })
}

createApp(OhosApp).use(i18n).mount('#app')
