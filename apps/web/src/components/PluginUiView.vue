<script setup lang="ts">
/* eslint-disable vue/no-v-html -- 全组件唯一 v-html 点：renderMarkdown 先转义全文，只插入固定标签 */
// 插件 UI 视图面板（docs/plans/plugin-webview.md §3.4/§3.5/§3.6 M2-R）：渲染
// 一个视图的声明式节（kind 闭集 markdown|form|list|kv）并承载交互回传。
// 注入防御红线：
// - 所有节文本/值一律 {{ }} 插值渲染；
// - 全组件唯一 v-html 点是 markdown 节，且必须走 services/markdown.ts 的
//   renderMarkdown（先转义全部文本、再插固定标签，v1 子集无链接语法 →
//   零 <a>/javascript: 面）；
// - 不渲染插件提供的任何 URL/图片，不注入插件 CSS，全部继承 var(--*) token。
// M2-R 生命周期：节身份=(pluginId, viewId, sectionId) 稳定 id（组件 key、
// 草稿、节数据、请求都不用显示序号）；form 草稿为组件本地状态（随组件
// 销毁，password 不进任何全局/持久化存储），节语义变化按清单 default 重置；
// list/kv 经 ui/values 动态回读（source:"provider" 开关，节错误态可重试）；
// 组件卸载=视图关闭，运行时状态经 closeUiView 清理（在途请求作废并复位
// loading/busy），重开从清单 default 与新 provider 数据开始。
import { computed, onBeforeUnmount, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { renderMarkdown } from '../services/markdown'
import {
  closeUiView,
  ensureUiSectionLoaded,
  findUiView,
  getPluginUiState,
  retryUiSection,
  runUiRowCommand,
  submitUiForm,
  subscribePluginUi,
  uiSectionDataKey,
  type PluginUiState,
  type UiKvItem,
  type UiScalar,
  type UiSection,
  type UiSectionDataState,
} from '../services/pluginUi'

const props = defineProps<{ pluginId: string; viewId: string }>()

const { t } = useI18n()

const state = ref<PluginUiState>(getPluginUiState())
const unsubscribe = subscribePluginUi(next => {
  state.value = next
})
onBeforeUnmount(() => {
  unsubscribe()
  // §3.6：关闭视图清理该视图运行时状态（节数据 + 在途请求），重开从清单
  // default 与新 provider 数据开始；组件本地草稿（含 password）随之销毁。
  closeUiView(props.pluginId, props.viewId)
})

const entry = computed(() => findUiView(props.pluginId, props.viewId))
const sections = computed(() => entry.value?.sections ?? [])
const viewName = computed(() => entry.value?.viewName ?? props.viewId)
const pluginName = computed(() => entry.value?.pluginName ?? props.pluginId)

/** markdown 节 → 可安全 v-html 的片段（唯一 v-html 点；转义在 renderMarkdown 内）。 */
function markdownHtml(text: string): string {
  return renderMarkdown(text, { copyLabel: t('agent.copy') })
}

/** 单节运行时状态（无条目时给默认空态，不写入 store）。 */
function sectionData(sectionId: string): UiSectionDataState {
  return (
    state.value.sectionData[uiSectionDataKey(props.pluginId, props.viewId, sectionId)] ?? {
      status: 'idle',
      values: null,
      error: null,
      note: null,
      busy: false,
    }
  )
}

/** 交互注记文案：成功固定文案；失败带宿主错误信息。 */
function noteText(data: UiSectionDataState): string {
  if (!data.note) return ''
  return data.note.kind === 'success'
    ? t('ui.submitOk')
    : t('ui.actionFailed', { message: data.note.message })
}

// ==== form 节：草稿为组件本地状态（保留用户输入），提交时服务层强制转换 ====
// 身份=稳定 sectionId（§3.6）；语义（key/type/default/options/min/max）变化 →
// 草稿按清单 default 重置；节删除 → 草稿清除。

const formDrafts = reactive<Record<string, Record<string, unknown>>>({})
const formDraftSignatures = reactive<Record<string, string>>({})

type FormSection = Extract<UiSection, { kind: 'form' }>

/** form 字段语义指纹（与服务端 uiSectionFingerprint 同口径）：变化即重置草稿。 */
function fieldsSignature(fields: FormSection['fields']): string {
  return JSON.stringify(
    fields.map(field => [
      field.key,
      field.type,
      field.default ?? null,
      field.options ?? null,
      field.min ?? null,
      field.max ?? null,
    ]),
  )
}

function defaultDraft(section: FormSection): Record<string, unknown> {
  const draft: Record<string, unknown> = {}
  for (const field of section.fields) {
    if (field.type === 'boolean') draft[field.key] = field.default === true
    else if (field.type === 'number') {
      draft[field.key] = typeof field.default === 'number' ? String(field.default) : ''
    } else {
      draft[field.key] = typeof field.default === 'string' ? field.default : ''
    }
  }
  return draft
}

/** form 草稿访问入口：签名一致返回既有草稿，否则按清单 default 重建。 */
function formDraft(section: FormSection): Record<string, unknown> {
  const signature = fieldsSignature(section.fields)
  if (formDraftSignatures[section.id] !== signature || !formDrafts[section.id]) {
    formDraftSignatures[section.id] = signature
    formDrafts[section.id] = defaultDraft(section)
  }
  return formDrafts[section.id]!
}

function onFormSubmit(section: FormSection): void {
  if (!section.command || sectionData(section.id).busy) return
  void submitUiForm(props.pluginId, props.viewId, section.id, formDraft(section))
}

// ==== list 节：行数据来自 ui/values 回读，行点击走 rowCommand ====

function listRows(sectionId: string): Record<string, UiScalar>[] {
  return sectionData(sectionId).values?.rows ?? []
}

function cellText(row: Record<string, UiScalar>, key: string): string {
  const value = row[key]
  return value === undefined || value === null ? '' : String(value)
}

function onRowClick(section: Extract<UiSection, { kind: 'list' }>, row: Record<string, UiScalar>): void {
  if (!section.rowCommand || sectionData(section.id).busy) return
  void runUiRowCommand(props.pluginId, props.viewId, section.id, row)
}

// ==== kv 节：source:"provider" 时动态回读优先，静态 items 兜底 ====

function kvItems(section: Extract<UiSection, { kind: 'kv' }>): UiKvItem[] {
  const data = sectionData(section.id)
  if (section.source === 'provider') {
    if (data.status === 'ready') return data.values?.items ?? []
    return section.items ?? []
  }
  return section.items ?? []
}

function onSectionRetry(sectionId: string): void {
  void retryUiSection(props.pluginId, props.viewId, sectionId)
}

// 挂载与声明热更新（§3.6）：
// 1. form 草稿对齐——语义变化的节重建草稿、删除的节清草稿；
// 2. provider 节首次回读（幂等；重排/插入节不改变既有节身份，不触发重载）。
watch(
  () => entry.value?.sections,
  nextSections => {
    if (!nextSections) return
    const aliveFormIds = new Set<string>()
    for (const section of nextSections) {
      if (section === null || section.kind !== 'form') continue
      aliveFormIds.add(section.id)
      formDraft(section)
    }
    for (const id of Object.keys(formDrafts)) {
      if (!aliveFormIds.has(id)) {
        delete formDrafts[id]
        delete formDraftSignatures[id]
      }
    }
    for (const section of nextSections) {
      if (
        section &&
        (section.kind === 'list' || section.kind === 'kv') &&
        section.source === 'provider'
      ) {
        void ensureUiSectionLoaded(props.pluginId, props.viewId, section.id)
      }
    }
  },
  { immediate: true },
)
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
        :key="section === null ? `__bad-${index}` : section.id"
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

          <!-- form 节：声明 command → 可编辑提交；否则只读禁用态 -->
          <div
            v-if="section.kind === 'form'"
            class="ui-form"
          >
            <div
              v-for="field in section.fields"
              :key="field.key"
              class="ui-field"
            >
              <span class="ui-label">{{ field.label || field.key }}</span>
              <input
                v-if="field.type === 'text'"
                v-model="formDraft(section)[field.key]"
                type="text"
                class="ui-input"
                :placeholder="field.placeholder ?? ''"
                :disabled="!section.command"
              >
              <input
                v-else-if="field.type === 'password'"
                v-model="formDraft(section)[field.key]"
                type="password"
                class="ui-input"
                :placeholder="field.placeholder ?? ''"
                :disabled="!section.command"
              >
              <input
                v-else-if="field.type === 'number'"
                v-model="formDraft(section)[field.key]"
                type="number"
                class="ui-input"
                :min="field.min"
                :max="field.max"
                :placeholder="field.placeholder ?? ''"
                :disabled="!section.command"
              >
              <label
                v-else-if="field.type === 'boolean'"
                class="ui-check"
              >
                <input
                  v-model="formDraft(section)[field.key]"
                  type="checkbox"
                  :disabled="!section.command"
                >
                <span>{{ formDraft(section)[field.key] === true ? t('ui.booleanOn') : t('ui.booleanOff') }}</span>
              </label>
              <select
                v-else
                v-model="formDraft(section)[field.key]"
                class="ui-input"
                :disabled="!section.command"
              >
                <option
                  v-for="option in field.options ?? []"
                  :key="option"
                  :value="option"
                >
                  {{ option }}
                </option>
              </select>
            </div>

            <div
              v-if="section.command"
              class="ui-actions"
            >
              <button
                type="button"
                class="ui-submit"
                :disabled="sectionData(section.id).busy"
                @click="onFormSubmit(section)"
              >
                {{ sectionData(section.id).busy ? t('ui.submitting') : t('ui.submit') }}
              </button>
              <span
                v-if="sectionData(section.id).note"
                class="ui-note"
                :class="sectionData(section.id).note?.kind"
                :title="sectionData(section.id).note?.kind === 'error' ? sectionData(section.id).note?.message : ''"
              >{{ noteText(sectionData(section.id)) }}</span>
            </div>
            <p
              v-else
              class="ui-note"
            >
              {{ t('ui.formReadonly') }}
            </p>
          </div>

          <!-- list 节：source:"provider" → ui/values 动态回读（错误态可重试）；
            行点击走 rowCommand（args=整行对象） -->
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
              >{{ column.label || column.key }}</span>
            </div>

            <template v-if="section.source === 'provider'">
              <p
                v-if="sectionData(section.id).status === 'loading' || sectionData(section.id).status === 'idle'"
                class="ui-note"
              >
                {{ t('ui.loading') }}
              </p>
              <p
                v-else-if="sectionData(section.id).status === 'error'"
                class="ui-note error"
              >
                <span
                  class="ui-note-text"
                  :title="`${sectionData(section.id).error?.code}: ${sectionData(section.id).error?.message}`"
                >{{ t('ui.loadError') }}</span>
                <button
                  type="button"
                  class="ui-retry"
                  @click="onSectionRetry(section.id)"
                >
                  {{ t('ui.retry') }}
                </button>
              </p>
              <template v-else>
                <p
                  v-if="listRows(section.id).length === 0"
                  class="ui-note"
                >
                  {{ t('ui.empty') }}
                </p>
                <button
                  v-for="(row, rowIndex) in listRows(section.id)"
                  :key="rowIndex"
                  type="button"
                  class="ui-row data"
                  :class="{ clickable: section.rowCommand, busy: sectionData(section.id).busy }"
                  :disabled="!section.rowCommand || sectionData(section.id).busy"
                  @click="onRowClick(section, row)"
                >
                  <span
                    v-for="column in section.columns"
                    :key="column.key"
                    class="ui-cell"
                    :style="{ flexGrow: column.width }"
                  >{{ cellText(row, column.key) }}</span>
                </button>
              </template>
            </template>
            <p
              v-else
              class="ui-note"
            >
              {{ t('ui.listNeedsNode') }}
            </p>

            <p
              v-if="section.source === 'provider' && sectionData(section.id).note"
              class="ui-note"
              :class="sectionData(section.id).note?.kind"
              :title="sectionData(section.id).note?.kind === 'error' ? sectionData(section.id).note?.message : ''"
            >
              {{ noteText(sectionData(section.id)) }}
            </p>
          </div>

          <!-- kv 节：source:"provider" → 动态回读优先（错误态可重试）；
            否则静态 items -->
          <div
            v-else
            class="ui-kv"
          >
            <template v-if="section.source === 'provider'">
              <p
                v-if="sectionData(section.id).status === 'error'"
                class="ui-note error"
              >
                <span
                  class="ui-note-text"
                  :title="`${sectionData(section.id).error?.code}: ${sectionData(section.id).error?.message}`"
                >{{ t('ui.loadError') }}</span>
                <button
                  type="button"
                  class="ui-retry"
                  @click="onSectionRetry(section.id)"
                >
                  {{ t('ui.retry') }}
                </button>
              </p>
              <p
                v-else-if="sectionData(section.id).status === 'loading' && (section.items?.length ?? 0) === 0"
                class="ui-note"
              >
                {{ t('ui.loading') }}
              </p>
            </template>
            <div
              v-for="(item, itemIndex) in kvItems(section)"
              :key="itemIndex"
              class="ui-kv-row"
            >
              <span class="ui-kv-key">{{ item.key }}</span>
              <span class="ui-kv-value">{{ item.value }}</span>
            </div>
            <p
              v-if="!section.source && (section.items?.length ?? 0) === 0"
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

.ui-note.success {
  color: var(--green, #7ec87e);
}

.ui-note-text {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
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

.ui-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.ui-submit {
  flex: none;
  height: 26px;
  padding: 0 14px;
  border: 1px solid var(--accent-line);
  border-radius: var(--r-xs);
  background: var(--accent-soft);
  color: var(--accent);
  font-size: 12px;
}

.ui-submit:hover:not(:disabled) {
  background: var(--accent);
  color: var(--bg);
}

.ui-submit:disabled {
  opacity: 0.6;
  cursor: default;
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

.ui-row.data {
  width: 100%;
  min-height: 26px;
  padding: 0 4px;
  text-align: left;
  font-size: 12px;
  color: var(--fg-2);
  border-radius: var(--r-xs);
}

.ui-row.data.clickable:hover:not(:disabled) {
  background: var(--panel-3);
}

.ui-row.data:disabled {
  cursor: default;
}

.ui-row.data.busy {
  opacity: 0.6;
}

.ui-cell {
  flex: 1 1 0;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ui-retry {
  flex: none;
  padding: 1px 8px;
  border-radius: var(--r-xs);
  color: var(--accent);
  font-size: 11px;
}

.ui-retry:hover {
  background: var(--accent-soft);
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
