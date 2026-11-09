<script setup lang="ts">
import { onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { activateTab, closeTab, moveTabTo, tabsStore } from '../services/tabs'

const { t } = useI18n()

// 桥接框架无关 store 与 Vue 响应性（同 App.vue / ModeSwitcher.vue 的模式）
const state = ref(tabsStore.get())
const unsub = tabsStore.subscribe(s => {
  state.value = s
})
onBeforeUnmount(unsub)

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

.empty {
  align-self: center;
  padding: 0 12px;
  color: var(--fg-faint);
  font-size: 12px;
}
</style>
