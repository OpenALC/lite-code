<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { isTauri, revealPath } from '../services/bridge'
import { copyText } from '../services/clipboard'
import {
  activateTab,
  closeOtherTabs,
  closeSavedTabs,
  closeTab,
  closeTabsToRight,
  moveTabTo,
  tabsStore,
} from '../services/tabs'

const { t } = useI18n()

// 桥接框架无关 store 与 Vue 响应性（同 App.vue / ModeSwitcher.vue 的模式）
const state = ref(tabsStore.get())
const unsub = tabsStore.subscribe(s => {
  state.value = s
})

// 关闭保护确认态：dirty 标签第一次点关闭 → services 返回 needsConfirm，
// 标签进入确认态；再次点击才 force 关闭（不用模态框，UI.md §7）。
const confirmingId = ref<string | null>(null)
const dragId = ref<string | null>(null)
const dropTarget = ref<{ id: string; position: 'before' | 'after' } | null>(null)

function onActivate(tabId: string): void {
  confirmingId.value = null
  activateTab(tabId)
}

/** 中键关闭标签，与 VS Code 一致；dirty 标签仍走二次确认。 */
function onAuxClick(event: MouseEvent, tabId: string): void {
  if (event.button !== 1) return
  event.preventDefault()
  onClose(tabId)
}

/** 标签溢出时把纵向滚轮转成横向滚动，不劫持可滚动区域以外的事件。 */
function onWheel(event: WheelEvent): void {
  const element = event.currentTarget as HTMLElement | null
  if (!element || element.scrollWidth <= element.clientWidth) return
  const delta = event.deltaY !== 0 ? event.deltaY : event.deltaX
  if (delta === 0) return
  element.scrollLeft += delta
  event.preventDefault()
}

function onDragStart(event: DragEvent, tabId: string): void {
  dragId.value = tabId
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', tabId)
  }
}

function onDragOver(event: DragEvent, tabId: string): void {
  if (!dragId.value || dragId.value === tabId) {
    dropTarget.value = null
    return
  }
  const element = event.currentTarget as HTMLElement | null
  if (!element) return
  const rect = element.getBoundingClientRect()
  dropTarget.value = {
    id: tabId,
    position: event.clientX < rect.left + rect.width / 2 ? 'before' : 'after',
  }
}

function onDrop(event: DragEvent, tabId: string): void {
  event.preventDefault()
  const fromId = dragId.value ?? event.dataTransfer?.getData('text/plain') ?? ''
  const position = dropTarget.value?.id === tabId ? dropTarget.value.position : 'before'
  if (fromId && fromId !== tabId) moveTabTo(fromId, tabId, position)
  resetDrag()
}

function resetDrag(): void {
  dragId.value = null
  dropTarget.value = null
}

const menu = ref<{ id: string; x: number; y: number } | null>(null)
const MENU_WIDTH = 188
const MENU_HEIGHT = 196

function menuTab() {
  return state.value.tabs.find(t => t.id === menu.value?.id) ?? null
}

function menuHasRight(): boolean {
  if (!menu.value) return false
  const index = state.value.tabs.findIndex(t => t.id === menu.value?.id)
  return index >= 0 && index < state.value.tabs.length - 1
}

function openContextMenu(event: MouseEvent, tabId: string): void {
  confirmingId.value = null
  resetDrag()
  menu.value = {
    id: tabId,
    x: Math.max(4, Math.min(event.clientX, window.innerWidth - MENU_WIDTH - 4)),
    y: Math.max(4, Math.min(event.clientY, window.innerHeight - MENU_HEIGHT - 4)),
  }
}

function runMenu(
  action: 'close' | 'others' | 'right' | 'saved' | 'copy' | 'reveal',
): void {
  const current = menu.value
  if (!current) return
  const tab = state.value.tabs.find(t => t.id === current.id)
  menu.value = null
  switch (action) {
    case 'close':
      onClose(current.id)
      break
    case 'others':
      closeOtherTabs(current.id)
      break
    case 'right':
      closeTabsToRight(current.id)
      break
    case 'saved':
      closeSavedTabs()
      break
    case 'copy':
      if (tab?.path) copyText(tab.path)
      break
    case 'reveal':
      if (tab?.path && isTauri()) void revealPath(tab.path)
      break
  }
}

function onDocumentPointerDown(event: PointerEvent): void {
  const target = event.target
  if (target instanceof Element && target.closest('.tab-menu')) return
  menu.value = null
}

function onWindowKey(event: KeyboardEvent): void {
  if (event.key === 'Escape' && menu.value) {
    event.preventDefault()
    menu.value = null
  }
}

onMounted(() => {
  document.addEventListener('pointerdown', onDocumentPointerDown, true)
  window.addEventListener('keydown', onWindowKey)
})

onBeforeUnmount(() => {
  unsub()
  document.removeEventListener('pointerdown', onDocumentPointerDown, true)
  window.removeEventListener('keydown', onWindowKey)
})

function onClose(tabId: string): void {
  if (confirmingId.value === tabId) {
    confirmingId.value = null
    closeTab(tabId, { force: true })
    return
  }
  const result = closeTab(tabId)
  confirmingId.value = result.kind === 'needsConfirm' ? tabId : null
}
</script>

<template>
  <div
    class="tabbar"
    role="tablist"
    :aria-label="t('tabs.aria')"
    @wheel="onWheel"
  >
    <div
      v-for="tab in state.tabs"
      :key="tab.id"
      role="presentation"
      class="tab"
      :data-tab-id="tab.id"
      :class="{
        active: tab.id === state.activeId,
        confirming: tab.id === confirmingId,
        dragging: tab.id === dragId,
        'drop-before': dropTarget?.id === tab.id && dropTarget.position === 'before',
        'drop-after': dropTarget?.id === tab.id && dropTarget.position === 'after',
      }"
      draggable="true"
      @dragstart="onDragStart($event, tab.id)"
      @dragover.prevent="onDragOver($event, tab.id)"
      @drop.prevent="onDrop($event, tab.id)"
      @dragend="resetDrag"
      @auxclick="onAuxClick($event, tab.id)"
      @contextmenu.prevent.stop="openContextMenu($event, tab.id)"
    >
      <button
        role="tab"
        type="button"
        class="tab-btn"
        :aria-selected="tab.id === state.activeId"
        @click="onActivate(tab.id)"
      >
        <span
          v-if="tab.dirty"
          class="dirty-dot"
          :title="t('tabs.dirty')"
        />
        <span>{{ tab.title }}</span>
      </button>
      <button
        type="button"
        class="tab-close"
        :class="{ confirming: tab.id === confirmingId }"
        :title="tab.id === confirmingId ? t('tabs.closeConfirm') : t('tabs.close')"
        :aria-label="tab.id === confirmingId ? t('tabs.closeConfirm') : t('tabs.close')"
        @click="onClose(tab.id)"
      >
        ×
      </button>
    </div>
    <div
      v-if="state.tabs.length === 0"
      class="empty"
    >
      {{ t('tabs.empty') }}
    </div>
    <div
      v-if="menu"
      class="tab-menu"
      role="menu"
      :style="{ left: `${menu.x}px`, top: `${menu.y}px` }"
    >
      <button
        type="button"
        @click="runMenu('close')"
      >
        {{ t('tabs.menu.close') }}
      </button>
      <button
        type="button"
        @click="runMenu('others')"
      >
        {{ t('tabs.menu.closeOthers') }}
      </button>
      <button
        type="button"
        :disabled="!menuHasRight()"
        @click="runMenu('right')"
      >
        {{ t('tabs.menu.closeRight') }}
      </button>
      <button
        type="button"
        @click="runMenu('saved')"
      >
        {{ t('tabs.menu.closeSaved') }}
      </button>
      <div class="tab-menu-sep" />
      <button
        type="button"
        :disabled="!menuTab()?.path"
        @click="runMenu('copy')"
      >
        {{ t('tabs.menu.copyPath') }}
      </button>
      <button
        type="button"
        :disabled="!menuTab()?.path || !isTauri()"
        @click="runMenu('reveal')"
      >
        {{ t('tabs.menu.reveal') }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.tabbar {
  display: flex;
  height: 34px;
  background: var(--panel);
  border-bottom: 1px solid var(--border);
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
}

.tabbar::-webkit-scrollbar {
  display: none;
}

.tab {
  position: relative;
  display: flex;
  flex: none;
  align-items: stretch;
  font-size: 12.5px;
  color: var(--fg-dim);
  border-right: 1px solid var(--border);
}

.tab.dragging {
  opacity: 0.45;
}

.tab.drop-before::before,
.tab.drop-after::after {
  content: '';
  position: absolute;
  top: 4px;
  bottom: 4px;
  width: 2px;
  background: var(--accent);
  z-index: 1;
}

.tab.drop-before::before {
  left: -1px;
}

.tab.drop-after::after {
  right: -1px;
}

.tab.active {
  background: var(--bg);
  color: var(--fg);
  box-shadow: inset 0 1px 0 var(--accent);
}

/* dirty 关闭确认态：强调色描边，视觉上可逆、无模态框 */
.tab.confirming {
  outline: 1px solid var(--accent-line);
  outline-offset: -1px;
}

.tab-btn {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 8px 0 12px;
}

.dirty-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--yellow);
}

.tab-close {
  align-self: center;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  margin-right: 6px;
  padding: 0;
  border-radius: var(--r-sm);
  color: var(--fg-faint);
  font-size: 12px;
  line-height: 1;
  opacity: 0;
  transition: color var(--dur) var(--ease), background var(--dur) var(--ease),
    opacity var(--dur) var(--ease);
}

.tab:hover .tab-close,
.tab.active .tab-close,
.tab.confirming .tab-close {
  opacity: 1;
}

.tab-close:hover {
  color: var(--fg);
  background: var(--panel-3);
}

.tab-close.confirming {
  color: var(--accent-fg);
  background: var(--accent);
}

.tab-menu {
  position: fixed;
  z-index: 80;
  width: 188px;
  padding: 4px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-2);
  box-shadow: var(--shadow);
  display: flex;
  flex-direction: column;
}

.tab-menu button {
  height: 26px;
  padding: 0 8px;
  border-radius: var(--r-xs);
  color: var(--fg-2);
  font-size: 12px;
  text-align: left;
}

.tab-menu button:hover:not(:disabled) {
  background: var(--panel-3);
  color: var(--fg);
}

.tab-menu button:disabled {
  color: var(--fg-faint);
  cursor: default;
}

.tab-menu-sep {
  height: 1px;
  margin: 4px 6px;
  background: var(--border);
}

.empty {
  align-self: center;
  padding: 0 12px;
  color: var(--fg-faint);
  font-size: 12px;
}
</style>
