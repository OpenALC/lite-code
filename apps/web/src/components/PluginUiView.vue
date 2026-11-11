<script setup lang="ts">
/* eslint-disable vue/no-v-html -- 全组件唯一 v-html 点：renderMarkdown 先转义全文，只插入固定标签 */
// 插件 UI 视图面板（docs/plans/plugin-webview.md §3.4）：渲染一个视图的
// 声明式节（kind 闭集 markdown|form|list|kv）。注入防御红线：
// - 所有节文本/值一律 {{ }} 插值渲染；
// - 全组件唯一 v-html 点是 markdown 节，且必须走 services/markdown.ts 的
//   renderMarkdown（先转义全部文本、再插固定标签，v1 子集无链接语法 →
//   零 <a>/javascript: 面）；
// - 不渲染插件提供的任何 URL/图片，不注入插件 CSS，全部继承 var(--*) token。
// M1 只渲染静态节：form 只读 + 禁用态，list/kv 动态数据注记「需要 Node 运行时」。
import { computed, onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { renderMarkdown } from '../services/markdown'
import {
  findUiView,
  getPluginUiState,
  subscribePluginUi,
  type PluginUiState,
  type UiField,
} from '../services/pluginUi'

const props = defineProps<{ pluginId: string; viewId: string }>()

const { t } = useI18n()

const state = ref<PluginUiState>(getPluginUiState())
const unsubscribe = subscribePluginUi(next => {
  state.value = next
})
onBeforeUnmount(() => {
  unsubscribe()
})

const entry = computed(() => findUiView(props.pluginId, props.viewId))
const sections = computed(() => entry.value?.sections ?? [])
const viewName = computed(() => entry.value?.viewName ?? props.viewId)
const pluginName = computed(() => entry.value?.pluginName ?? props.pluginId)

/** markdown 节 → 可安全 v-html 的片段（唯一 v-html 点；转义在 renderMarkdown 内）。 */
function markdownHtml(text: string): string {
  return renderMarkdown(text, { copyLabel: t('agent.copy') })
}

function fieldTextDefault(field: UiField): string {
  return typeof field.default === 'string' ? field.default : ''
}

function fieldNumberDefault(field: UiField): string {
  return typeof field.default === 'number' ? String(field.default) : ''
}

function fieldChecked(field: UiField): boolean {
  return field.default === true
}

function isSelectedOption(field: UiField, option: string): boolean {
  return field.default === option
}
</script>

<template>
  <div class="plugin-ui">
    <header class="ui-head">
      <h1>{{ viewName }}</h1>
      <span
        class="ui-source"
        :title="entry?.pluginId"
      >{{ t('views.fromPlugin', { name: pluginName }) }}</span>
    </header>

    <p
      v-if="!entry"
      class="ui-note error"
    >
      {{ t('ui.unavailable') }}
    </p>

    <template v-else>
      <section
        v-for="(section, index) in sections"
        :key="index"
        class="ui-section"
      >
        <p
          v-if="section === null"
          class="ui-note error"
        >
          {{ t('ui.sectionError') }}
        </p>

        <!-- markdown 节：唯一 v-html 点，renderMarkdown 先转义后插固定标签 -->
        <div
          v-else-if="section.kind === 'markdown'"
          class="ui-md"
          v-html="markdownHtml(section.text)"
        />

        <template v-else>
          <h2
            v-if="section.title"
            class="ui-section-title"
          >
            {{ section.title }}
          </h2>

          <!-- form 节：M1 只读展示静态 default，禁用态 + 交互注记（M2 提交） -->
          <div
            v-if="section.kind === 'form'"
            class="ui-form"
          >
            <div
              v-for="field in section.fields"
              :key="field.key"
              class="ui-field"
            >
              <span class="ui-label">{{ field.label }}</span>
              <input
                v-if="field.type === 'text'"
                type="text"
                class="ui-input"
                :value="fieldTextDefault(field)"
                :placeholder="field.placeholder ?? ''"
                disabled
              >
              <input
                v-else-if="field.type === 'password'"
                type="password"
                class="ui-input"
                :value="fieldTextDefault(field)"
                :placeholder="field.placeholder ?? ''"
                disabled
              >
              <input
                v-else-if="field.type === 'number'"
                type="number"
                class="ui-input"
                :value="fieldNumberDefault(field)"
                :min="field.min"
                :max="field.max"
                :placeholder="field.placeholder ?? ''"
                disabled
              >
              <label
                v-else-if="field.type === 'boolean'"
                class="ui-check"
              >
                <input
                  type="checkbox"
                  :checked="fieldChecked(field)"
                  disabled
                >
                <span>{{ fieldChecked(field) ? t('ui.booleanOn') : t('ui.booleanOff') }}</span>
              </label>
              <select
                v-else
                class="ui-input"
                disabled
              >
                <option
                  v-for="option in field.options ?? []"
                  :key="option"
                  :value="option"
                  :selected="isSelectedOption(field, option)"
                >
                  {{ option }}
                </option>
              </select>
            </div>
            <p class="ui-note">
              {{ t('ui.formReadonly') }}
            </p>
          </div>

          <!-- list 节：M1 无数据回读，渲染列头 + 「需要 Node 运行时」空态注记 -->
          <div
            v-else-if="section.kind === 'list'"
            class="ui-list"
          >
            <div class="ui-row head">
              <span
                v-for="column in section.columns"
                :key="column.key"
                class="ui-cell"
                :style="{ flexGrow: column.width }"
              >{{ column.label }}</span>
            </div>
            <p class="ui-note">
              {{ t('ui.listNeedsNode') }}
            </p>
          </div>

          <!-- kv 节：静态 items 渲染；声明 valuesCommand 但无静态 items → 空态注记 -->
          <div
            v-else
            class="ui-kv"
          >
            <div
              v-for="(item, itemIndex) in section.items ?? []"
              :key="itemIndex"
              class="ui-kv-row"
            >
              <span class="ui-kv-key">{{ item.key }}</span>
              <span class="ui-kv-value">{{ item.value }}</span>
            </div>
            <p
              v-if="(section.items?.length ?? 0) === 0 && section.valuesCommand"
              class="ui-note"
            >
              {{ t('ui.kvNeedsNode') }}
            </p>
          </div>
        </template>
      </section>
    </template>
  </div>
</template>

<style scoped>
.plugin-ui {
  height: 100%;
  overflow: auto;
  padding: 18px 20px 28px;
  display: flex;
  flex-direction: column;
  gap: 14px;
  width: 100%;
  max-width: 860px;
  margin: 0 auto;
}

.ui-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
  min-width: 0;
}

.ui-head h1 {
  margin: 0;
  font-size: 18px;
  font-weight: 650;
  color: var(--fg);
}

.ui-source {
  font-size: 11px;
  color: var(--fg-faint);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ui-section {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 0;
  padding: 12px 14px;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: var(--r);
}

.ui-section-title {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--fg-2);
}

.ui-note {
  margin: 0;
  font-size: 11px;
  color: var(--fg-faint);
  line-height: 1.5;
}

.ui-note.error {
  color: var(--red);
}

/* markdown 节：与 Agent 消息同一套 md 类名，复用全局样式基线。 */
.ui-md {
  font-size: 12.5px;
  line-height: 1.6;
  color: var(--fg-2);
  overflow-wrap: anywhere;
}

.ui-form {
  display: flex;
  flex-direction: column;
  gap: 10px;
  min-width: 0;
}

.ui-field {
  display: grid;
  grid-template-columns: minmax(96px, 160px) 1fr;
  align-items: center;
  gap: 8px 12px;
  min-width: 0;
}

.ui-label {
  font-size: 12px;
  color: var(--fg-2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ui-input {
  height: 28px;
  min-width: 0;
  padding: 0 10px;
  border: 1px solid var(--border);
  border-radius: var(--r-xs);
  background: var(--bg);
  color: var(--fg);
  font: inherit;
  font-size: 12px;
}

.ui-input:disabled {
  opacity: 0.65;
  cursor: default;
}

.ui-check {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--fg-dim);
}

.ui-check input {
  width: 14px;
  height: 14px;
  margin: 0;
  accent-color: var(--accent);
}

.ui-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.ui-row.head {
  padding-bottom: 6px;
  border-bottom: 1px solid var(--border);
  font-size: 11px;
  font-weight: 600;
  color: var(--fg-dim);
}

.ui-cell {
  flex: 1 1 0;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ui-kv {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.ui-kv-row {
  display: flex;
  align-items: baseline;
  gap: 10px;
  min-width: 0;
  font-size: 12px;
}

.ui-kv-key {
  flex: 0 0 auto;
  min-width: 96px;
  color: var(--fg-dim);
}

.ui-kv-value {
  flex: 1;
  min-width: 0;
  color: var(--fg-2);
  overflow-wrap: anywhere;
}

@media (max-width: 640px) {
  .plugin-ui {
    padding: 14px 12px 24px;
  }

  .ui-field {
    grid-template-columns: 1fr;
    gap: 4px;
  }
}
</style>
