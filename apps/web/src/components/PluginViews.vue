<script setup lang="ts">
import { onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  getPluginViewsState,
  retryPluginViewSlot,
  runPluginViewCommand,
  subscribePluginViews,
  togglePluginViewNode,
  togglePluginViewSection,
  visibleViewRows,
  type PluginViewsState,
  type ViewSectionState,
} from '../services/pluginViews'

// 插件树视图分区（docs/EXTENSIONS.md §4）：渲染在资源管理器侧栏底部、
// 文件树之下。分区头=视图 name 并标注来源插件；节点行贴 FileTree 风格
// （缩进 + 折叠箭头），点击带 command 的节点走既有命令执行。

const { t } = useI18n()

const state = ref<PluginViewsState>(getPluginViewsState())
const unsubscribe = subscribePluginViews(next => {
  state.value = next
})
onBeforeUnmount(() => {
  unsubscribe()
})

function rows(section: ViewSectionState) {
  return visibleViewRows(section)
}

function onNodeClick(section: ViewSectionState, row: ReturnType<typeof rows>[number]): void {
  if (row.kind !== 'node') return
  if (row.node.command) {
    void runPluginViewCommand(row.node.command)
    return
  }
  void togglePluginViewNode(section.section.key, row.path, row.node.id)
}

function onTwistClick(section: ViewSectionState, row: ReturnType<typeof rows>[number]): void {
  if (row.kind !== 'node') return
  void togglePluginViewNode(section.section.key, row.path, row.node.id)
}

/** 错误行/错误横幅的重试：根节点 path 为 ''，nodeId 为 null。 */
function retry(
  section: ViewSectionState,
  path: string,
  nodeId: string | null,
): void {
  void retryPluginViewSlot(section.section.key, path, nodeId)
}
</script>

<template>
  <div
    v-if="state.sections.length > 0"
    class="plugin-views"
  >
    <section
      v-for="section in state.sections"
      :key="section.section.key"
      class="view-section"
    >
      <button
        type="button"
        class="section-head"
        :aria-expanded="!section.collapsed"
        :aria-label="
          section.collapsed
            ? t('views.expandSection', { name: section.section.viewName })
            : t('views.collapseSection', { name: section.section.viewName })
        "
        @click="togglePluginViewSection(section.section.key)"
      >
        <span class="twist">{{ section.collapsed ? '▸' : '▾' }}</span>
        <span class="section-name">{{ section.section.viewName }}</span>
        <span
          class="section-source"
          :title="section.section.pluginId"
        >{{ t('views.fromPlugin', { name: section.section.pluginName }) }}</span>
      </button>

      <div
        v-if="!section.collapsed"
        class="section-body"
        role="tree"
        :aria-label="section.section.viewName"
      >
        <div
          v-if="!section.slots[''] || section.slots[''].status === 'loading'"
          class="row note"
        >
          {{ t('views.loading') }}
        </div>
        <div
          v-else-if="section.slots[''].status === 'error' && section.slots[''].error"
          class="row note error"
        >
          <span
            class="note-text"
            :title="`${section.slots[''].error.code}: ${section.slots[''].error.message}`"
          >{{ t('views.loadError') }}</span>
          <button
            type="button"
            class="retry"
            @click="retry(section, '', null)"
          >
            {{ t('views.retry') }}
          </button>
        </div>
        <template v-else>
          <template
            v-for="(row, index) in rows(section)"
            :key="row.kind === 'node' ? row.path : `${row.kind}-${row.path}-${index}`"
          >
            <div
              v-if="row.kind === 'node'"
              class="row node"
              role="treeitem"
              :aria-expanded="row.expanded"
              :style="{ paddingLeft: `${12 + row.depth * 14}px` }"
            >
              <button
                type="button"
                class="twist-btn"
                :aria-label="
                  row.expanded
                    ? t('views.collapseNode', { label: row.node.label })
                    : t('views.expandNode', { label: row.node.label })
                "
                @click="onTwistClick(section, row)"
              >
                {{ row.expanded ? '▾' : '▸' }}
              </button>
              <button
                type="button"
                class="label-btn"
                :title="row.node.tooltip ?? row.node.label"
                @click="onNodeClick(section, row)"
              >
                <span
                  v-if="row.node.icon"
                  class="icon"
                >{{ row.node.icon }}</span>
                <span class="name">{{ row.node.label }}</span>
              </button>
            </div>

            <div
              v-else-if="row.kind === 'loading'"
              class="row note"
              :style="{ paddingLeft: `${12 + row.depth * 14}px` }"
            >
              {{ t('views.loading') }}
            </div>

            <div
              v-else
              class="row note error"
              :style="{ paddingLeft: `${12 + row.depth * 14}px` }"
              :title="`${row.error.code}: ${row.error.message}`"
            >
              <span class="note-text">{{ t('views.loadError') }}</span>
              <button
                type="button"
                class="retry"
                @click="retry(section, row.path, row.path.split('/').pop() ?? null)"
              >
                {{ t('views.retry') }}
              </button>
            </div>
          </template>
          <div
            v-if="rows(section).length === 0"
            class="row note"
          >
            {{ t('views.empty') }}
          </div>
        </template>
      </div>
    </section>
  </div>
</template>

<style scoped>
.plugin-views {
  flex: none;
  max-height: 45%;
  overflow-y: auto;
  border-top: 1px solid var(--border);
}

.view-section + .view-section {
  border-top: 1px solid var(--border);
}

.section-head {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 100%;
  height: 28px;
  padding: 0 10px;
  text-align: left;
  color: var(--fg-dim);
  font-size: 10px;
  font-weight: 650;
  letter-spacing: 0.06em;
  text-transform: uppercase;
}

.section-head:hover {
  background: var(--panel-3);
  color: var(--fg);
}

.section-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.section-source {
  margin-left: auto;
  overflow: hidden;
  color: var(--fg-faint);
  font-size: 10px;
  font-weight: 400;
  letter-spacing: 0;
  text-transform: none;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.section-body {
  padding-bottom: 4px;
}

.row {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 100%;
  min-height: 24px;
  padding-right: 12px;
  text-align: left;
  font-size: 12px;
  color: var(--fg-2);
}

.row.node:hover {
  background: var(--panel-3);
}

.twist-btn {
  width: 14px;
  flex: none;
  color: var(--fg-faint);
  font-size: 10px;
}

.label-btn {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 4px;
  height: 24px;
  padding: 0;
  text-align: left;
  color: var(--fg-2);
  font-size: 12px;
}

.label-btn:focus-visible {
  outline: 1px solid var(--accent-line);
  outline-offset: -1px;
  background: var(--accent-soft);
}

.row.note {
  color: var(--fg-faint);
  cursor: default;
}

.row.note.error {
  color: var(--red, #e06c6c);
}

.note-text {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.retry {
  flex: none;
  padding: 1px 8px;
  border-radius: var(--r-xs);
  color: var(--accent);
  font-size: 11px;
}

.retry:hover {
  background: var(--accent-soft);
}

.twist {
  width: 10px;
  flex: none;
  color: var(--fg-faint);
  font-size: 10px;
}

.icon {
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
</style>
