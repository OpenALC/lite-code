<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'

import AppIcon from './AppIcon.vue'
import type { DirEntryDto } from '@lc-proto'
import { readDir, readFile } from '../services/bridge'
import { resolveEditorStrategy } from '../services/largeFile'
import { activateTab, openTab, tabsStore, type TabsState } from '../services/tabs'

const props = defineProps<{ root?: string }>()

const state = ref<TabsState>(tabsStore.get())
const unsubscribe = tabsStore.subscribe(next => {
  state.value = next
})

const tab = computed(() =>
  state.value.tabs.find(item => item.id === state.value.activeId),
)

interface CrumbItem {
  name: string
  path: string
}

const items = computed<CrumbItem[]>(() => {
  const path = tab.value?.path
  if (!path) return []
  const normalized = path.replace(/\\/g, '/')
  const parts = normalized.split('/').filter(Boolean)
  const absolute = normalized.startsWith('/')
  let acc = ''
  return parts.map((name, index) => {
    if (index === 0) acc = absolute ? `/${name}` : name
    else acc = `${acc}/${name}`
    return { name, path: acc }
  })
})
const segments = computed(() => items.value.slice(-5))

const menu = ref<{ dir: string; x: number; y: number } | null>(null)
const entries = ref<DirEntryDto[]>([])
const loading = ref(false)
const menuError = ref('')

function parentOf(path: string): string {
  const normalized = path.replace(/\\/g, '/')
  const index = normalized.lastIndexOf('/')
  if (index <= 0) return props.root ?? ''
  const parent = normalized.slice(0, index)
  return /^[A-Za-z]:$/.test(parent) ? (props.root ?? '') : parent
}

async function loadEntries(dir: string): Promise<void> {
  loading.value = true
  menuError.value = ''
  const res = await readDir(props.root ?? '', dir)
  loading.value = false
  if (!res.ok) {
    entries.value = []
    menuError.value = res.error.message
    return
  }
  entries.value = [...res.data.entries].sort((a, b) =>
    a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === 'dir' ? -1 : 1,
  )
}

function openMenu(event: MouseEvent, dir: string): void {
  const x = Math.min(event.clientX, window.innerWidth - 252)
  const y = Math.min(event.clientY + 6, window.innerHeight - 348)
  menu.value = { dir, x: Math.max(4, x), y: Math.max(4, y) }
  void loadEntries(dir)
}

function closeMenu(): void {
  menu.value = null
  entries.value = []
  menuError.value = ''
}

function openSegment(event: MouseEvent, index: number): void {
  const segment = segments.value[index]
  if (!segment) return
  const isLast = index === segments.value.length - 1
  openMenu(event, isLast ? parentOf(segment.path) : segment.path)
}

async function chooseEntry(entry: DirEntryDto): Promise<void> {
  if (entry.kind === 'dir') {
    if (!menu.value) return
    menu.value = { ...menu.value, dir: entry.path }
    await loadEntries(entry.path)
    return
  }
  const existing = tabsStore.get().tabs.find(item => item.path === entry.path)
  if (existing) {
    activateTab(existing.id)
    closeMenu()
    return
  }
  const res = await readFile(props.root ?? '', entry.path)
  if (!res.ok) {
    menuError.value = res.error.message
    return
  }
  openTab({
    id: entry.path,
    title: entry.name,
    path: entry.path,
    content: res.data.text,
    strategy: resolveEditorStrategy(res.data.text.length),
  })
  closeMenu()
}

function onDocumentPointerDown(event: PointerEvent): void {
  const target = event.target
  if (target instanceof Element && target.closest('.crumb-menu')) return
  closeMenu()
}

function onWindowKey(event: KeyboardEvent): void {
  if (event.key === 'Escape' && menu.value) {
    event.preventDefault()
    closeMenu()
  }
}

onMounted(() => {
  document.addEventListener('pointerdown', onDocumentPointerDown, true)
  window.addEventListener('keydown', onWindowKey)
  window.addEventListener('resize', closeMenu)
  window.addEventListener('scroll', closeMenu, true)
})

onBeforeUnmount(() => {
  unsubscribe()
  document.removeEventListener('pointerdown', onDocumentPointerDown, true)
  window.removeEventListener('keydown', onWindowKey)
  window.removeEventListener('resize', closeMenu)
  window.removeEventListener('scroll', closeMenu, true)
})
</script>

<template>
  <div
    v-if="segments.length > 0"
    class="breadcrumbs"
    :title="tab?.path ?? ''"
  >
    <AppIcon
      name="folder"
      :size="12"
    />
    <template
      v-for="(segment, index) in segments"
      :key="`${segment.path}-${index}`"
    >
      <span class="crumb-sep">›</span>
      <button
        type="button"
        class="crumb"
        :class="{ last: index === segments.length - 1 }"
        :title="segment.path"
        @click.stop="openSegment($event, index)"
      >
        {{ segment.name }}
      </button>
    </template>

    <div
      v-if="menu"
      class="crumb-menu"
      role="menu"
      :style="{ left: `${menu.x}px`, top: `${menu.y}px` }"
    >
      <div
        v-if="loading"
        class="crumb-note"
      >
        …
      </div>
      <div
        v-else-if="menuError"
        class="crumb-note error"
      >
        {{ menuError }}
      </div>
      <div
        v-else-if="entries.length === 0"
        class="crumb-note"
      >
        —
      </div>
      <button
        v-for="entry in entries"
        :key="entry.path"
        type="button"
        class="crumb-entry"
        @click="chooseEntry(entry)"
      >
        <AppIcon
          :name="entry.kind === 'dir' ? 'folder' : 'file'"
          :size="13"
        />
        <span>{{ entry.name }}</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.breadcrumbs {
  flex: none;
  height: 24px;
  display: flex;
  align-items: center;
  gap: 3px;
  padding: 0 12px;
  border-bottom: 1px solid var(--border);
  background: var(--bg);
  color: var(--fg-faint);
  font-size: 11.5px;
  overflow: hidden;
  white-space: nowrap;
}

.crumb {
  max-width: 150px;
  padding: 1px 3px;
  border-radius: var(--r-xs);
  color: var(--fg-faint);
  font: inherit;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.crumb:hover {
  background: var(--panel-3);
  color: var(--fg);
}

.crumb.last {
  color: var(--fg-2);
}

.crumb-sep {
  color: var(--fg-faint);
}

.crumb-menu {
  position: fixed;
  z-index: 90;
  display: flex;
  flex-direction: column;
  width: 240px;
  max-height: 340px;
  padding: 4px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-2);
  box-shadow: var(--shadow);
  overflow: auto;
}

.crumb-entry {
  display: flex;
  flex: none;
  align-items: center;
  gap: 8px;
  min-height: 26px;
  padding: 0 8px;
  border-radius: var(--r-xs);
  color: var(--fg-2);
  font-size: 12px;
  text-align: left;
}

.crumb-entry:hover {
  background: var(--panel-3);
  color: var(--fg);
}

.crumb-entry span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.crumb-note {
  padding: 6px 8px;
  color: var(--fg-faint);
  font-size: 11px;
}

.crumb-note.error {
  color: var(--red);
}
</style>
