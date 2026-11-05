<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DbSchemaNode, DbSchemaNodeKind } from '../services/dbTypes'

defineOptions({ name: 'DbSchemaTree' })

const props = withDefaults(
  defineProps<{
    nodes: DbSchemaNode[]
    path?: string[]
    depth?: number
  }>(),
  {
    path: () => [],
    depth: 0,
  },
)

const emit = defineEmits<{ insertTable: [path: string[]] }>()
const { t } = useI18n()

const expanded = ref<Record<string, boolean>>({})

function nodeKey(node: DbSchemaNode, index: number): string {
  return `${index}:${node.name}`
}

function nodePath(node: DbSchemaNode): string[] {
  return [...props.path, node.name]
}

function isExpanded(key: string): boolean {
  return expanded.value[key] === true
}

function toggle(key: string): void {
  expanded.value = { ...expanded.value, [key]: !isExpanded(key) }
}

function isTableNode(node: DbSchemaNode): boolean {
  return node.kind === 'table' || node.kind === 'view'
}

function insertTable(node: DbSchemaNode): void {
  if (!isTableNode(node)) return
  emit('insertTable', nodePath(node))
}

function iconOf(kind: DbSchemaNodeKind): string {
  switch (kind) {
    case 'database':
      return '▤'
    case 'schema':
      return '◫'
    case 'table':
      return '▦'
    case 'view':
      return '◉'
    case 'column':
      return '•'
  }
}

function kindLabel(kind: DbSchemaNodeKind): string {
  return t(`db.nodeKinds.${kind}`)
}

function rowTitle(node: DbSchemaNode): string {
  const parts = [node.name, kindLabel(node.kind)]
  if (node.detail) parts.push(node.detail)
  return parts.join(' · ')
}
</script>

<template>
  <ul
    class="schema-tree"
    :class="{ root: depth === 0 }"
  >
    <li
      v-for="(node, index) in nodes"
      :key="nodeKey(node, index)"
      class="schema-item"
    >
      <div
        class="node-row"
        :style="{ paddingLeft: `${depth * 12}px` }"
      >
        <button
          v-if="node.children.length > 0"
          type="button"
          class="twisty"
          :title="isExpanded(nodeKey(node, index)) ? t('db.collapse') : t('db.expand')"
          :aria-expanded="isExpanded(nodeKey(node, index))"
          @click="toggle(nodeKey(node, index))"
        >
          {{ isExpanded(nodeKey(node, index)) ? '▾' : '▸' }}
        </button>
        <span
          v-else
          class="twisty spacer"
        />
        <button
          type="button"
          class="node-label"
          :class="[`kind-${node.kind}`, { insertable: isTableNode(node) }]"
          :title="isTableNode(node) ? t('db.insertTable', { name: node.name }) : rowTitle(node)"
          :disabled="!isTableNode(node)"
          @click="insertTable(node)"
        >
          <span class="node-icon">{{ iconOf(node.kind) }}</span>
          <span class="node-name">{{ node.name }}</span>
          <span
            v-if="node.detail"
            class="node-detail"
          >{{ node.detail }}</span>
          <span
            v-if="isTableNode(node)"
            class="insert-hint"
          >↵</span>
        </button>
      </div>
      <DbSchemaTree
        v-if="node.children.length > 0 && isExpanded(nodeKey(node, index))"
        :nodes="node.children"
        :path="nodePath(node)"
        :depth="depth + 1"
        @insert-table="emit('insertTable', $event)"
      />
    </li>
  </ul>
</template>

<style scoped>
.schema-tree {
  margin: 0;
  padding: 0;
  list-style: none;
}

.schema-tree.root {
  padding: 2px 0;
}

.schema-item {
  min-width: 0;
}

.node-row {
  display: flex;
  align-items: center;
  min-width: 0;
  height: 22px;
}

.twisty {
  flex: none;
  width: 16px;
  height: 18px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--fg-faint);
  font-size: 9px;
}

.twisty:hover {
  color: var(--fg);
}

.twisty.spacer {
  pointer-events: none;
}

.node-label {
  flex: 1;
  min-width: 0;
  height: 20px;
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 0 6px 0 2px;
  border-radius: var(--r-sm);
  color: var(--fg-2);
  font-size: 11px;
  text-align: left;
}

.node-label:disabled {
  cursor: default;
  opacity: 1;
}

.node-label.insertable:hover {
  background: var(--panel-3);
  color: var(--fg);
}

.node-icon {
  flex: none;
  width: 12px;
  color: var(--fg-faint);
  font-size: 10px;
  text-align: center;
}

.kind-database .node-icon,
.kind-schema .node-icon {
  color: var(--accent);
}

.kind-view .node-icon {
  color: var(--yellow);
}

.kind-column .node-icon {
  color: var(--fg-faint);
}

.node-name {
  flex: none;
  max-width: 70%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.kind-database .node-name,
.kind-schema .node-name {
  color: var(--fg);
  font-weight: 600;
}

.kind-table .node-name {
  color: var(--fg-2);
}

.kind-column .node-name {
  color: var(--fg-dim);
}

.node-detail {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  color: var(--fg-faint);
  font-family: var(--font-mono);
  font-size: 10px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.insert-hint {
  margin-left: auto;
  color: var(--fg-faint);
  font-family: var(--font-mono);
  font-size: 10px;
}

.node-label.insertable:hover .insert-hint {
  color: var(--accent);
}
</style>
