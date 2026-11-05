<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { isTauri, readFile, readDir } from '../services/bridge'
import {
  createTreeState,
  layerFailed,
  layerLoaded,
  reRoot,
  toggleDir,
  visibleRows,
  type TreeState,
} from '../services/fileTree'
import type { DirEntryDto } from '../services/contract'
import { openTab } from '../services/tabs'

// 一层树（docs/UI.md §3：按需加载，一次一层）。
// 目录点击 → 展开/折叠下一层；文件点击 → bridge fs_read_file 读取成功后
// 以文件路径为 id 打开标签页（services/tabs.ts）。编辑与保存不在本批：
// tabs 服务尚无内容通道（Editor UI lane 接管），本组件只保证"可读才开页"。

const props = defineProps<{ root: string }>()
const { t } = useI18n()

const state = ref<TreeState>(createTreeState(props.root, false))
const rows = computed(() => visibleRows(state.value))
const rootLayer = computed(() => state.value.layers[state.value.root])
/** 空态：真实读取成功且无条目（占位数据总有条目，不会落到这里） */
const isEmpty = computed(() => rows.value.length === 0 && rootLayer.value?.state === 'ready')
/** 未打开工作区（Tauri 模式且 root 为空；浏览器模式有占位数据可演示） */
const noRoot = computed(() => state.value.root.length === 0 && !state.value.fallback)

/** 文件打开失败的尾部提示（一次一条，成功即清除；错误码进 tooltip，UI.md §4） */
const fileError = ref<{ code: string; message: string } | null>(null)

onMounted(() => {
  state.value = createTreeState(props.root, !isTauri())
  void loadRoot()
})

watch(
  () => props.root,
  next => {
    state.value = reRoot(state.value, next)
    fileError.value = null
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

async function onEntryClick(entry: DirEntryDto): Promise<void> {
  fileError.value = null
  if (entry.kind === 'dir') {
    state.value = toggleDir(state.value, entry.path)
    if (state.value.expanded[entry.path] && !state.value.layers[entry.path]) {
      await loadLayer(entry.path)
    }
    return
  }
  // 文件：读取成功才开页（内容渲染由 Editor UI lane 接入 CodeEditor）
  const res = await readFile(props.root, entry.path)
  if (res.ok) {
    openTab({ id: entry.path, title: entry.name })
  } else {
    fileError.value = res.error
  }
}
</script>

<template>
  <div
    class="tree"
    role="tree"
    :aria-label="t('tree.title')"
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
          type="button"
          class="row"
          role="treeitem"
          :aria-expanded="row.entry.kind === 'dir' ? row.expanded : undefined"
          :style="{ paddingLeft: `${12 + row.depth * 14}px` }"
          :title="row.entry.path"
          @click="onEntryClick(row.entry)"
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
      v-if="!noRoot"
      class="hint"
    >
      {{ t('tree.readOnly') }}
    </div>
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

.row:hover {
  background: var(--panel-3);
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
</style>
