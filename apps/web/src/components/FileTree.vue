<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  createEntry,
  deletePath,
  isTauri,
  movePath,
  readDir,
  readFile,
  renamePath,
  revealPath,
} from '../services/bridge'
import {
  createTreeState,
  forgetPath,
  joinPath,
  layerFailed,
  layerLoaded,
  parentPath,
  planMove,
  reRoot,
  setExpanded,
  toggleDir,
  visibleRows,
  type TreeState,
} from '../services/fileTree'
import type { DirEntryDto } from '@lc-proto'
import { resolveEditorStrategy } from '../services/largeFile'
import { openTab, relocateTabs, removeTabsUnder } from '../services/tabs'

// 一层树（docs/UI.md §3：按需加载，一次一层）。
// 目录点击 → 展开/折叠下一层；文件点击 → bridge fs_read_file 读取成功后
// 以文件路径为 id 打开标签页（services/tabs.ts）。
// 右键菜单与拖拽移动只改文件树；移动/删除成功后由 tabs 服务同步打开标签，
// 避免标签继续指向旧路径。

const props = defineProps<{ root: string; refreshToken?: number }>()
const { t } = useI18n()

const state = ref<TreeState>(createTreeState(props.root, false))
const HIDDEN_NAMES = new Set(['.git', '.github', '.zcode'])
const rows = computed(() =>
  visibleRows(state.value).filter(
    row => row.kind !== 'entry' || !HIDDEN_NAMES.has(row.entry.name),
  ),
)
const rootLayer = computed(() => state.value.layers[state.value.root])
/** 空态：真实读取成功且无条目（占位数据总有条目，不会落到这里） */
const isEmpty = computed(() => rows.value.length === 0 && rootLayer.value?.state === 'ready')
/** 未打开工作区（Tauri 模式且 root 为空；浏览器模式有占位数据可演示） */
const noRoot = computed(() => state.value.root.length === 0 && !state.value.fallback)
/** 浏览器占位数据不可写；拖拽与写操作统一关闭 */
const canMutate = computed(() => state.value.root.length > 0 && !state.value.fallback)

/** 文件打开失败的尾部提示（一次一条，成功即清除；错误码进 tooltip，UI.md §4） */
const fileError = ref<{ code: string; message: string } | null>(null)
/** 写操作失败提示：label 走 i18n，detail 只进 tooltip */
const actionError = ref<{ label: string; detail: string } | null>(null)

interface ContextMenuState {
  x: number
  y: number
  entry: DirEntryDto | null
  /** 新建目标的目录：右键目录 = 目录本身；右键文件 = 父目录；空白 = 根 */
  targetDir: string
}

const menu = ref<ContextMenuState | null>(null)
const menuEl = ref<HTMLElement | null>(null)
const dragSource = ref<DirEntryDto | null>(null)
const dropTargetPath = ref<string | null>(null)

const MENU_WIDTH = 224

type MenuAction = 'open' | 'rename' | 'delete' | 'newFile' | 'newFolder' | 'reveal' | 'refresh'

onMounted(() => {
  state.value = createTreeState(props.root, !isTauri())
  document.addEventListener('pointerdown', onDocumentPointerDown, true)
  document.addEventListener('keydown', onDocumentKeydown)
  window.addEventListener('resize', closeMenu)
  window.addEventListener('scroll', closeMenu, true)
  void loadRoot()
})

onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', onDocumentPointerDown, true)
  document.removeEventListener('keydown', onDocumentKeydown)
  window.removeEventListener('resize', closeMenu)
  window.removeEventListener('scroll', closeMenu, true)
  resetTypeahead()
})

watch(
  () => props.root,
  next => {
    closeMenu()
    dragSource.value = null
    dropTargetPath.value = null
    actionError.value = null
    state.value = reRoot(state.value, next)
    fileError.value = null
    focusedPath.value = null
    rowEls.clear()
    resetTypeahead()
    void loadRoot()
  },
)

// 外部改动（新建/删除文件、生成产物）后由工具栏刷新：丢弃层缓存并重新读根，
// 展开状态一并重置，避免显示已删除目录的陈旧内容。
watch(
  () => props.refreshToken,
  () => {
    if (props.refreshToken === undefined) return
    closeMenu()
    dragSource.value = null
    dropTargetPath.value = null
    actionError.value = null
    fileError.value = null
    focusedPath.value = null
    rowEls.clear()
    resetTypeahead()
    state.value = createTreeState(props.root, !isTauri())
    void loadRoot()
  },
)

async function loadRoot(): Promise<void> {
  if (state.value.root.length === 0) return
  await loadLayer(state.value.root)
}

async function loadLayer(path: string): Promise<void> {
  const res = await readDir(props.root, path)
  if (res.ok) {
    state.value = layerLoaded(state.value, path, res.data.entries)
  } else {
    state.value = layerFailed(state.value, path, res.error)
  }
}

const focusedPath = ref<string | null>(null)
const rowEls = new Map<string, HTMLButtonElement>()
let typeahead = ''
let typeaheadTimer: ReturnType<typeof setTimeout> | null = null

function resetTypeahead(): void {
  typeahead = ''
  if (typeaheadTimer !== null) clearTimeout(typeaheadTimer)
  typeaheadTimer = null
}

function setRowEl(path: string, el: unknown): void {
  if (el instanceof HTMLButtonElement) rowEls.set(path, el)
  else rowEls.delete(path)
}

function entryRows() {
  return rows.value.filter(row => row.kind === 'entry')
}

function focusRow(path: string): void {
  focusedPath.value = path
  void nextTick(() => rowEls.get(path)?.focus())
}

/** VSC 式键盘导航：上下移动、左右展开/折叠、Home/End 跳首尾。 */
function onRowKeydown(event: KeyboardEvent, row: (typeof rows.value)[number]): void {
  if (row.kind !== 'entry') return
  const list = entryRows()
  const index = list.findIndex(item => item.entry.path === row.entry.path)
  if (
    event.key.length === 1 &&
    event.key !== ' ' &&
    !event.ctrlKey &&
    !event.metaKey &&
    !event.altKey
  ) {
    event.preventDefault()
    typeahead += event.key.toLocaleLowerCase()
    if (typeaheadTimer !== null) clearTimeout(typeaheadTimer)
    typeaheadTimer = setTimeout(resetTypeahead, 600)
    for (let offset = 1; offset <= list.length; offset += 1) {
      const candidate = list[(index + offset) % list.length]
      if (candidate?.entry.name.toLocaleLowerCase().startsWith(typeahead)) {
        focusRow(candidate.entry.path)
        break
      }
    }
    return
  }
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault()
    const next = list[index + (event.key === 'ArrowDown' ? 1 : -1)]
    if (next) focusRow(next.entry.path)
    return
  }
  if (event.key === 'Home' || event.key === 'End') {
    event.preventDefault()
    const next = event.key === 'Home' ? list[0] : list[list.length - 1]
    if (next) focusRow(next.entry.path)
    return
  }
  if (event.key === 'ArrowRight') {
    if (row.entry.kind === 'dir' && !row.expanded) {
      event.preventDefault()
      void onEntryClick(row.entry)
    }
    return
  }
  if (event.key !== 'ArrowLeft') return
  event.preventDefault()
  if (row.entry.kind === 'dir' && row.expanded) {
    void onEntryClick(row.entry)
    return
  }
  for (let i = index - 1; i >= 0; i -= 1) {
    const candidate = list[i]
    if (candidate && candidate.depth < row.depth) {
      focusRow(candidate.entry.path)
      return
    }
  }
}

async function onEntryClick(entry: DirEntryDto): Promise<void> {
  fileError.value = null
  if (entry.kind === 'dir') {
    state.value = toggleDir(state.value, entry.path)
    if (state.value.expanded[entry.path] && !state.value.layers[entry.path]) {
      await loadLayer(entry.path)
    }
    return
  }
  // 文件：读取成功才开页，内容进标签 store，由 CodeEditor 渲染并保存
  const res = await readFile(props.root, entry.path)
  if (res.ok) {
    openTab({
      id: entry.path,
      title: entry.name,
      path: entry.path,
      content: res.data.text,
      strategy: resolveEditorStrategy(entry.size ?? 0),
    })
  } else {
    fileError.value = res.error
  }
}

// ==== 右键菜单 ====

function openContextMenu(event: MouseEvent, entry: DirEntryDto | null): void {
  event.preventDefault()
  const itemCount = entry ? 7 : 3
  const estimatedHeight = itemCount * 30 + 16 + (entry ? 16 : 0)
  const targetDir = entry
    ? entry.kind === 'dir'
      ? entry.path
      : parentPath(entry.path)
    : state.value.root
  menu.value = {
    x: Math.max(8, Math.min(event.clientX, window.innerWidth - MENU_WIDTH - 8)),
    y: Math.max(8, Math.min(event.clientY, window.innerHeight - estimatedHeight - 8)),
    entry,
    targetDir,
  }
  void nextTick(() => {
    menuEl.value?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus()
  })
}

function closeMenu(): void {
  menu.value = null
}

function onDocumentPointerDown(event: PointerEvent): void {
  if (!menu.value) return
  const target = event.target
  if (menuEl.value && target instanceof Node && menuEl.value.contains(target)) return
  closeMenu()
}

function onDocumentKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape' && menu.value) {
    event.preventDefault()
    closeMenu()
  }
}

async function runMenuAction(action: MenuAction): Promise<void> {
  const current = menu.value
  if (!current) return
  closeMenu()
  const entry = current.entry
  switch (action) {
    case 'open':
      if (entry) await onEntryClick(entry)
      return
    case 'rename':
      if (entry) await renameEntry(entry)
      return
    case 'delete':
      if (entry) await deleteEntry(entry)
      return
    case 'newFile':
      await createIn(current.targetDir, false)
      return
    case 'newFolder':
      await createIn(current.targetDir, true)
      return
    case 'reveal':
      if (entry) await revealEntry(entry)
      return
    case 'refresh':
      await refreshTree()
  }
}

function clearActionError(): void {
  actionError.value = null
}

function fail(error: { code: string; message: string }): void {
  actionError.value = {
    label: t('tree.menu.actionFailed'),
    detail: `${error.code}: ${error.message}`,
  }
}

function invalid(key: string): void {
  actionError.value = { label: t(key), detail: '' }
}

function validName(name: string): boolean {
  return (
    name.length > 0 &&
    name !== '.' &&
    name !== '..' &&
    !name.includes('/') &&
    !name.includes('\\')
  )
}

async function createIn(targetDir: string, dir: boolean): Promise<void> {
  clearActionError()
  if (!canMutate.value || !targetDir) return
  const answer = window.prompt(
    dir ? t('tree.menu.newFolderPrompt') : t('tree.menu.newFilePrompt'),
    '',
  )
  if (answer === null) return
  const name = answer.trim()
  if (!validName(name)) {
    invalid('tree.menu.invalidName')
    return
  }
  const res = await createEntry(joinPath(targetDir, name), dir)
  if (!res.ok) {
    fail(res.error)
    return
  }
  await showDir(targetDir)
}

async function renameEntry(entry: DirEntryDto): Promise<void> {
  clearActionError()
  if (!canMutate.value) return
  const answer = window.prompt(
    t('tree.menu.renamePrompt', { name: entry.name }),
    entry.name,
  )
  if (answer === null) return
  const name = answer.trim()
  if (!validName(name)) {
    invalid('tree.menu.invalidName')
    return
  }
  const to = joinPath(parentPath(entry.path), name)
  if (to === entry.path) return
  const res = await renamePath(entry.path, to)
  if (!res.ok) {
    fail(res.error)
    return
  }
  await afterRelocation(entry, to)
}

async function deleteEntry(entry: DirEntryDto): Promise<void> {
  clearActionError()
  if (!canMutate.value) return
  const message =
    entry.kind === 'dir'
      ? t('tree.menu.deleteDirConfirm', { name: entry.name })
      : t('tree.menu.deleteConfirm', { name: entry.name })
  if (!window.confirm(message)) return
  const res = await deletePath(entry.path)
  if (!res.ok) {
    fail(res.error)
    return
  }
  const parent = parentPath(entry.path)
  removeTabsUnder(entry.path)
  state.value = forgetPath(state.value, entry.path)
  if (parent) await loadLayer(parent)
}

async function revealEntry(entry: DirEntryDto): Promise<void> {
  clearActionError()
  if (!canMutate.value) return
  const res = await revealPath(entry.path)
  if (!res.ok) fail(res.error)
}

async function refreshTree(): Promise<void> {
  clearActionError()
  const { root, expanded } = state.value
  if (!root) return
  const paths = new Set([root, ...Object.keys(expanded).filter(path => expanded[path])])
  await Promise.all([...paths].map(path => loadLayer(path)))
}

/** 展开目标目录并刷新一层，让新建/落点结果立即可见。 */
async function showDir(dir: string): Promise<void> {
  state.value = setExpanded(state.value, dir, true)
  await loadLayer(dir)
}

/** 重命名成功后同步标签路径、丢弃旧层缓存，并刷新源/目标父目录。 */
async function afterRelocation(source: DirEntryDto, to: string): Promise<void> {
  const wasExpanded =
    source.kind === 'dir' && state.value.expanded[source.path] === true
  const fromParent = parentPath(source.path)
  const toParent = parentPath(to)

  relocateTabs(source.path, to)
  state.value = forgetPath(state.value, source.path)
  if (wasExpanded) state.value = setExpanded(state.value, to, true)
  if (toParent) state.value = setExpanded(state.value, toParent, true)

  const dirs = new Set<string>([fromParent, toParent])
  if (wasExpanded) dirs.add(to)
  await Promise.all([...dirs].filter(Boolean).map(path => loadLayer(path)))
}

// ==== 拖拽移动 ====

function onDragStart(entry: DirEntryDto, event: DragEvent): void {
  if (!canMutate.value) {
    event.preventDefault()
    return
  }
  closeMenu()
  clearActionError()
  dragSource.value = entry
  dropTargetPath.value = null
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', entry.path)
  }
}

function onDragOver(entry: DirEntryDto, event: DragEvent): void {
  const source = dragSource.value
  if (!source || !canMutate.value) return
  const plan = planMove(props.root, source, entry)
  // 始终接管 dragover：非法目标也触发 drop，由 drop 显示明确错误。
  event.preventDefault()
  if (event.dataTransfer) event.dataTransfer.dropEffect = plan.ok ? 'move' : 'none'
  dropTargetPath.value = plan.ok ? entry.path : null
}

function onDragLeave(entry: DirEntryDto): void {
  if (dropTargetPath.value === entry.path) dropTargetPath.value = null
}

async function onDrop(entry: DirEntryDto, event: DragEvent): Promise<void> {
  event.preventDefault()
  const source = dragSource.value
  dragSource.value = null
  dropTargetPath.value = null
  if (!source) return

  const plan = planMove(props.root, source, entry)
  if (!plan.ok) {
    if (plan.reason === 'sameParent') return
    invalid('tree.menu.invalidMove')
    return
  }

  clearActionError()
  const res = await movePath(source.path, plan.to)
  if (!res.ok) {
    fail(res.error)
    return
  }

  const wasExpanded =
    source.kind === 'dir' && state.value.expanded[source.path] === true
  const fromParent = parentPath(source.path)
  relocateTabs(source.path, plan.to)
  state.value = forgetPath(state.value, source.path)
  state.value = setExpanded(state.value, plan.destDir, true)
  if (wasExpanded) state.value = setExpanded(state.value, plan.to, true)

  const dirs = new Set<string>([fromParent, plan.destDir])
  if (wasExpanded) dirs.add(plan.to)
  await Promise.all([...dirs].filter(Boolean).map(path => loadLayer(path)))
}

function onDragEnd(): void {
  dragSource.value = null
  dropTargetPath.value = null
}
</script>

<template>
  <div
    class="tree"
    role="tree"
    :aria-label="t('tree.title')"
    @contextmenu.prevent="openContextMenu($event, null)"
  >
    <div
      v-if="state.fallback"
      class="banner"
    >
      {{ t('tree.fallback') }}
    </div>
    <div
      v-else-if="noRoot"
      class="banner"
    >
      {{ t('tree.noRoot') }}
    </div>

    <div class="rows">
      <template
        v-for="(row, index) in rows"
        :key="row.kind === 'entry' ? row.entry.path : `${row.kind}-${index}`"
      >
        <button
          v-if="row.kind === 'entry'"
          :ref="el => setRowEl(row.entry.path, el)"
          type="button"
          class="row"
          role="treeitem"
          :aria-expanded="row.entry.kind === 'dir' ? row.expanded : undefined"
          :aria-selected="focusedPath === row.entry.path"
          :tabindex="focusedPath === row.entry.path || (!focusedPath && index === 0) ? 0 : -1"
          :style="{ paddingLeft: `${12 + row.depth * 14}px` }"
          :title="row.entry.path"
          :draggable="canMutate"
          :class="{
            dragging: dragSource?.path === row.entry.path,
            'drop-target': dropTargetPath === row.entry.path,
          }"
          @focus="focusedPath = row.entry.path"
          @keydown="onRowKeydown($event, row)"
          @click="onEntryClick(row.entry)"
          @contextmenu.prevent.stop="openContextMenu($event, row.entry)"
          @dragstart="onDragStart(row.entry, $event)"
          @dragover="onDragOver(row.entry, $event)"
          @dragleave="onDragLeave(row.entry)"
          @drop="onDrop(row.entry, $event)"
          @dragend="onDragEnd"
        >
          <span class="twist">{{ row.entry.kind === 'dir' ? (row.expanded ? '▾' : '▸') : '' }}</span>
          <span
            class="name"
            :class="{ dim: row.entry.kind !== 'file' }"
          >{{ row.entry.name }}</span>
          <span
            v-if="row.entry.kind === 'file' && row.entry.size != null"
            class="size"
          >{{ row.entry.size }}</span>
        </button>

        <div
          v-else-if="row.kind === 'loading'"
          class="row note"
          :style="{ paddingLeft: `${12 + row.depth * 14}px` }"
        >
          {{ t('tree.loading') }}
        </div>

        <div
          v-else
          class="row note error"
          :style="{ paddingLeft: `${12 + row.depth * 14}px` }"
          :title="`${row.error.code}: ${row.error.message}`"
        >
          {{ t('tree.loadError') }}
        </div>
      </template>

      <div
        v-if="isEmpty"
        class="row note"
      >
        {{ t('tree.empty') }}
      </div>
    </div>

    <div
      v-if="fileError"
      class="banner error"
      :title="`${fileError.code}: ${fileError.message}`"
    >
      {{ t('tree.fileError') }}
    </div>
    <div
      v-if="actionError"
      class="banner error"
      :title="actionError.detail"
    >
      {{ actionError.label }}
    </div>
    <div
      v-if="!noRoot"
      class="hint"
    >
      {{ t('tree.editableHint') }}
    </div>

    <Teleport to="body">
      <div
        v-if="menu"
        ref="menuEl"
        class="menu"
        role="menu"
        :aria-label="t('tree.menu.label')"
        :style="{ left: `${menu.x}px`, top: `${menu.y}px` }"
        @contextmenu.prevent.stop
        @pointerdown.stop
      >
        <template v-if="menu.entry">
          <button
            type="button"
            class="menu-item"
            role="menuitem"
            @click="runMenuAction('open')"
          >
            {{ t('tree.menu.open') }}
          </button>
          <button
            type="button"
            class="menu-item"
            role="menuitem"
            :disabled="!canMutate"
            @click="runMenuAction('rename')"
          >
            {{ t('tree.menu.rename') }}
          </button>
          <button
            type="button"
            class="menu-item"
            role="menuitem"
            :disabled="!canMutate"
            @click="runMenuAction('delete')"
          >
            {{ t('tree.menu.delete') }}
          </button>
          <div
            class="menu-sep"
            role="separator"
          />
        </template>

        <button
          type="button"
          class="menu-item"
          role="menuitem"
          :disabled="!canMutate"
          @click="runMenuAction('newFile')"
        >
          {{ t('tree.menu.newFile') }}
        </button>
        <button
          type="button"
          class="menu-item"
          role="menuitem"
          :disabled="!canMutate"
          @click="runMenuAction('newFolder')"
        >
          {{ t('tree.menu.newFolder') }}
        </button>

        <template v-if="menu.entry">
          <div
            class="menu-sep"
            role="separator"
          />
          <button
            type="button"
            class="menu-item"
            role="menuitem"
            :disabled="!canMutate"
            @click="runMenuAction('reveal')"
          >
            {{ t('tree.menu.reveal') }}
          </button>
        </template>

        <button
          type="button"
          class="menu-item"
          role="menuitem"
          :disabled="!state.root"
          @click="runMenuAction('refresh')"
        >
          {{ t('tree.menu.refresh') }}
        </button>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.tree {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.banner {
  padding: 4px 12px;
  font-size: 11px;
  color: var(--fg-dim);
  background: var(--panel-3);
  border-bottom: 1px solid var(--border);
}

.banner.error,
.row.note.error {
  color: var(--red, #e06c6c);
}

.rows {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.row {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 100%;
  height: 26px;
  padding-right: 12px;
  text-align: left;
  font-size: 12px;
  color: var(--fg-2);
}

.row:focus-visible {
  outline: 1px solid var(--accent-line);
  outline-offset: -1px;
  background: var(--accent-soft);
}

.row:hover {
  background: var(--panel-3);
}

.row.dragging {
  opacity: 0.5;
}

.row.drop-target {
  background: var(--accent-soft);
  outline: 1px solid var(--accent-line);
  outline-offset: -1px;
}

.twist {
  width: 10px;
  flex: none;
  color: var(--fg-faint);
  font-size: 10px;
}

.name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.name.dim {
  color: var(--fg);
}

.size {
  flex: none;
  color: var(--fg-faint);
  font-size: 10px;
  font-family: var(--font-mono);
}

.row.note {
  color: var(--fg-faint);
  cursor: default;
}

.hint {
  padding: 4px 12px;
  font-size: 10px;
  color: var(--fg-faint);
  border-top: 1px solid var(--border);
}

.menu {
  position: fixed;
  z-index: 1000;
  min-width: 200px;
  max-width: 260px;
  padding: 4px;
  display: flex;
  flex-direction: column;
  background: var(--panel);
  border: 1px solid var(--border-strong);
  border-radius: 8px;
  box-shadow: 0 10px 30px rgba(0, 0, 0, 0.45);
}

.menu-item {
  height: 30px;
  padding: 0 10px;
  display: flex;
  align-items: center;
  border-radius: var(--r-sm);
  text-align: left;
  font-size: 12px;
  color: var(--fg-2);
  white-space: nowrap;
}

.menu-item:hover:not(:disabled) {
  background: var(--panel-3);
  color: var(--fg);
}

.menu-item:disabled {
  color: var(--fg-faint);
  cursor: default;
}

.menu-sep {
  height: 1px;
  margin: 4px 6px;
  background: var(--border);
}
</style>
