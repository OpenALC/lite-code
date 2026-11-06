<script setup lang="ts">
// 供应商管理页：编辑 ~/.lite-code/providers.json。
// 页面只拿到脱敏后的密钥提示；新 Key 留空表示保留旧值。
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  providersLoad,
  providersSave,
  type ProviderKind,
  type ProvidersView,
  type ProviderView,
  type ProviderModelConfig,
} from '../services/bridge'

const { t } = useI18n()

interface ProviderDraft {
  id: string
  kind: ProviderKind
  label: string
  baseUrl: string
  enabled: boolean
  apiKey: string
  apiKeyRef: string
  hasApiKey: boolean
  apiKeyHint: string | null
  clearApiKey: boolean
}

interface ModelDraft {
  id: string
  provider: string
  modelId: string
  thinkingText: string
  contextK: number
  compressAtK: number
  capabilities: string[]
}

const providers = ref<ProviderDraft[]>([])
const models = ref<ModelDraft[]>([])
const defaultModel = ref<string>('')
const defaultThinking = ref<string>('')
const subagentModel = ref<string>('')
const subagentThinking = ref<string>('')
const selectedProviderId = ref('')
const loading = ref(true)
const saving = ref(false)
const error = ref<string | null>(null)
const success = ref<string | null>(null)
const isFallback = ref(false)
const showKey = ref(false)
const confirmProviderId = ref<string | null>(null)
const confirmModelIndex = ref<number | null>(null)

const selectedProvider = computed(
  () => providers.value.find(provider => provider.id === selectedProviderId.value) ?? null,
)

const providerOptions = computed(() =>
  providers.value.map(provider => ({ id: provider.id, label: provider.label || provider.id })),
)

const modelOptions = computed(() =>
  models.value
    .filter(model => model.id.trim())
    .map(model => ({ id: model.id, label: model.id })),
)

const defaultThinkingOptions = computed(() => thinkingOptions(defaultModel.value))
const subagentThinkingOptions = computed(() => thinkingOptions(subagentModel.value))

function thinkingOptions(modelId: string): string[] {
  const model = models.value.find(item => item.id === modelId)
  const levels = parseThinking(model?.thinkingText ?? '')
  return levels.length > 0 ? levels : ['off', 'low', 'medium', 'high']
}

function parseThinking(text: string): string[] {
  return [...new Set(
    text
      .split(/[,，]/)
      .map(part => part.trim())
      .filter(Boolean),
  )]
}

function applyView(view: ProvidersView): void {
  const previous = selectedProviderId.value
  providers.value = view.providers.map(toProviderDraft)
  models.value = view.models.map(toModelDraft)
  defaultModel.value = view.defaultModel ?? models.value[0]?.id ?? ''
  defaultThinking.value = view.defaultThinking ?? ''
  subagentModel.value = view.subagentModel ?? ''
  subagentThinking.value = view.subagentThinking ?? ''
  selectedProviderId.value = providers.value.some(provider => provider.id === previous)
    ? previous
    : (providers.value[0]?.id ?? '')
  showKey.value = false
  confirmProviderId.value = null
  confirmModelIndex.value = null
}

function toProviderDraft(provider: ProviderView): ProviderDraft {
  return {
    id: provider.id,
    kind: provider.kind === 'anthropic-messages' ? 'anthropic-messages' : 'openai-responses',
    label: provider.label,
    baseUrl: provider.baseUrl,
    enabled: provider.enabled,
    apiKey: '',
    apiKeyRef: provider.apiKeyRef,
    hasApiKey: provider.hasApiKey,
    apiKeyHint: provider.apiKeyHint,
    clearApiKey: false,
  }
}

function toModelDraft(model: ProviderModelConfig): ModelDraft {
  return {
    id: model.id,
    provider: model.provider,
    modelId: model.modelId,
    thinkingText: model.thinking.join(', '),
    contextK: model.contextK,
    compressAtK: model.compressAtK,
    capabilities: [...model.capabilities],
  }
}

async function load(): Promise<void> {
  loading.value = true
  error.value = null
  const res = await providersLoad()
  loading.value = false
  if (!res.ok) {
    error.value = res.error.message
    return
  }
  isFallback.value = res.fallback
  applyView(res.data)
}

function nextProviderId(): string {
  // 随机后缀避免删除后重建复用旧 id，从而误继承旧配置里的密钥。
  let id = ''
  do {
    id = `provider-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`
  } while (providers.value.some(provider => provider.id === id))
  return id
}

function addProvider(): void {
  const provider: ProviderDraft = {
    id: nextProviderId(),
    kind: 'openai-responses',
    label: t('providers.newProvider'),
    baseUrl: 'https://api.openai.com/v1',
    enabled: true,
    apiKey: '',
    apiKeyRef: '',
    hasApiKey: false,
    apiKeyHint: null,
    clearApiKey: false,
  }
  providers.value.push(provider)
  selectedProviderId.value = provider.id
  showKey.value = false
  success.value = null
}

function nextModelId(): string {
  let n = models.value.length + 1
  let id = t('providers.newModelName', { n })
  while (models.value.some(model => model.id === id)) {
    n += 1
    id = t('providers.newModelName', { n })
  }
  return id
}

function addModel(): void {
  const provider = selectedProvider.value ?? providers.value[0]
  if (!provider) {
    error.value = t('providers.needProviderFirst')
    return
  }
  const model: ModelDraft = {
    id: nextModelId(),
    provider: provider.id,
    modelId: 'gpt-5',
    thinkingText: 'off, low, medium, high',
    contextK: 400,
    compressAtK: 300,
    capabilities: ['tools'],
  }
  models.value.push(model)
  if (!defaultModel.value) defaultModel.value = model.id
  success.value = null
}

function providerModelCount(providerId: string): number {
  return models.value.filter(model => model.provider === providerId).length
}

function removeProvider(provider: ProviderDraft): void {
  if (providerModelCount(provider.id) > 0) {
    error.value = t('providers.deleteProviderBlocked')
    return
  }
  if (confirmProviderId.value !== provider.id) {
    confirmProviderId.value = provider.id
    confirmModelIndex.value = null
    return
  }
  providers.value = providers.value.filter(item => item.id !== provider.id)
  if (selectedProviderId.value === provider.id) {
    selectedProviderId.value = providers.value[0]?.id ?? ''
  }
  confirmProviderId.value = null
  success.value = null
}

function removeModel(model: ModelDraft, index: number): void {
  if (confirmModelIndex.value !== index) {
    confirmModelIndex.value = index
    confirmProviderId.value = null
    return
  }
  models.value = models.value.filter((_, i) => i !== index)
  if (defaultModel.value === model.id) defaultModel.value = models.value[0]?.id ?? ''
  if (subagentModel.value === model.id) subagentModel.value = ''
  confirmModelIndex.value = null
  success.value = null
}

function selectProvider(provider: ProviderDraft): void {
  selectedProviderId.value = provider.id
  showKey.value = false
  confirmProviderId.value = null
  confirmModelIndex.value = null
}

function onApiKeyInput(provider: ProviderDraft): void {
  provider.clearApiKey = false
  success.value = null
}

function clearSavedKey(provider: ProviderDraft): void {
  provider.clearApiKey = true
  provider.apiKey = ''
}

function undoClearKey(provider: ProviderDraft): void {
  provider.clearApiKey = false
}

function keyPlaceholder(provider: ProviderDraft): string {
  if (provider.clearApiKey) return t('providers.keyWillClear')
  if (provider.apiKeyHint) return t('providers.keySaved', { hint: provider.apiKeyHint })
  if (provider.apiKeyRef) return t('providers.keyEnv', { ref: provider.apiKeyRef })
  return t('providers.keyPlaceholder')
}

function normalizeDefaults(): void {
  const ids = models.value.map(model => model.id.trim()).filter(Boolean)
  if (!defaultModel.value || !ids.includes(defaultModel.value)) {
    defaultModel.value = ids[0] ?? ''
  }
  if (defaultThinking.value && !defaultThinkingOptions.value.includes(defaultThinking.value)) {
    defaultThinking.value = defaultThinkingOptions.value[0] ?? ''
  }
  if (subagentModel.value && !ids.includes(subagentModel.value)) {
    subagentModel.value = ''
    subagentThinking.value = ''
  }
  if (subagentThinking.value && !subagentThinkingOptions.value.includes(subagentThinking.value)) {
    subagentThinking.value = subagentThinkingOptions.value[0] ?? ''
  }
}

function localError(): string | null {
  if (providers.value.length === 0) return t('providers.validation.providerRequired')
  if (models.value.length === 0) return t('providers.validation.modelRequired')
  if (providers.value.some(provider => !provider.label.trim() || !provider.baseUrl.trim())) {
    return t('providers.validation.providerFields')
  }
  if (
    providers.value.some(
      provider =>
        !provider.baseUrl.startsWith('http://') && !provider.baseUrl.startsWith('https://'),
    )
  ) {
    return t('providers.validation.baseUrl')
  }
  if (models.value.some(model => !model.id.trim() || !model.modelId.trim() || !model.provider)) {
    return t('providers.validation.modelFields')
  }
  if (models.value.some(model => !Number.isFinite(model.contextK) || model.contextK <= 0)) {
    return t('providers.validation.contextK')
  }
  if (
    models.value.some(
      model =>
        !Number.isFinite(model.compressAtK) ||
        model.compressAtK <= 0 ||
        model.compressAtK >= model.contextK,
    )
  ) {
    return t('providers.validation.compressAtK')
  }
  const ids = new Set(models.value.map(model => model.id.trim()))
  if (ids.size !== models.value.length) return t('providers.validation.duplicateModel')
  const providerIds = new Set(providers.value.map(provider => provider.id))
  if (models.value.some(model => !providerIds.has(model.provider))) {
    return t('providers.validation.modelProvider')
  }
  return null
}

async function save(): Promise<void> {
  if (saving.value) return
  error.value = null
  success.value = null
  normalizeDefaults()
  const problem = localError()
  if (problem) {
    error.value = problem
    return
  }
  saving.value = true
  const input = {
    providers: providers.value.map(provider => ({
      id: provider.id,
      kind: provider.kind,
      label: provider.label.trim(),
      baseUrl: provider.baseUrl.trim(),
      enabled: provider.enabled,
      apiKey: provider.apiKey.trim() || undefined,
      apiKeyRef: provider.apiKeyRef.trim() || undefined,
      clearApiKey: provider.clearApiKey,
    })),
    models: models.value.map(model => ({
      id: model.id.trim(),
      provider: model.provider,
      modelId: model.modelId.trim(),
      thinking: parseThinking(model.thinkingText),
      contextK: Math.round(model.contextK),
      compressAtK: Math.round(model.compressAtK),
      capabilities: model.capabilities,
    })),
    defaultModel: defaultModel.value || null,
    defaultThinking: defaultThinking.value || null,
    subagentModel: subagentModel.value || null,
    subagentThinking: subagentThinking.value || null,
  }
  const res = await providersSave(input)
  saving.value = false
  if (!res.ok) {
    error.value = res.error.message
    return
  }
  isFallback.value = res.fallback
  applyView(res.data)
  success.value = res.fallback ? t('providers.savedDemo') : t('providers.saved')
}

onMounted(load)
</script>

<template>
  <div class="providers-page">
    <header class="page-head">
      <div>
        <h1>{{ t('providers.title') }}</h1>
        <p>{{ t('providers.subtitle') }}</p>
      </div>
      <div class="head-actions">
        <span
          v-if="isFallback"
          class="demo-badge"
        >{{ t('providers.demo') }}</span>
        <button
          type="button"
          class="primary"
          :disabled="saving || loading"
          @click="save"
        >
          {{ saving ? t('providers.saving') : t('providers.save') }}
        </button>
      </div>
    </header>

    <div
      v-if="error"
      class="banner error"
    >
      {{ error }}
    </div>
    <div
      v-if="success"
      class="banner success"
    >
      {{ success }}
    </div>
    <div
      v-if="loading"
      class="empty"
    >
      {{ t('providers.loading') }}
    </div>

    <fieldset
      v-else
      class="form-body"
      :disabled="saving"
    >
      <div class="layout">
        <aside class="provider-list">
          <div class="list-head">
            <span>{{ t('providers.providerList') }}</span>
            <button
              type="button"
              class="ghost"
              @click="addProvider"
            >
              + {{ t('providers.addProvider') }}
            </button>
          </div>
          <button
            v-for="provider in providers"
            :key="provider.id"
            type="button"
            class="provider-card"
            :class="{ active: provider.id === selectedProviderId }"
            @click="selectProvider(provider)"
          >
            <span
              class="dot"
              :class="{ on: provider.enabled }"
            />
            <span class="provider-text">
              <span class="provider-name">{{ provider.label || provider.id }}</span>
              <span class="provider-meta">{{ provider.baseUrl }}</span>
            </span>
            <span class="provider-count">{{ providerModelCount(provider.id) }}</span>
          </button>
          <div
            v-if="providers.length === 0"
            class="list-empty"
          >
            {{ t('providers.noProviders') }}
          </div>
        </aside>

        <section class="detail">
          <div
            v-if="selectedProvider"
            class="card"
          >
            <div class="card-head">
              <h2>{{ t('providers.providerConfig') }}</h2>
              <div class="card-actions">
                <label class="switch">
                  <input
                    v-model="selectedProvider.enabled"
                    type="checkbox"
                  >
                  <span>{{ t('providers.enabled') }}</span>
                </label>
                <button
                  type="button"
                  class="danger ghost"
                  @click="removeProvider(selectedProvider)"
                >
                  {{ confirmProviderId === selectedProvider.id ? t('providers.confirmDelete') : t('providers.delete') }}
                </button>
              </div>
            </div>

            <div class="grid two">
              <label class="field">
                <span>{{ t('providers.providerName') }}</span>
                <input
                  v-model="selectedProvider.label"
                  type="text"
                  :placeholder="t('providers.providerNamePlaceholder')"
                >
              </label>
              <label class="field">
                <span>{{ t('providers.format') }}</span>
                <select v-model="selectedProvider.kind">
                  <option value="openai-responses">
                    OpenAI Responses
                  </option>
                  <option
                    value="anthropic-messages"
                    :disabled="selectedProvider.kind !== 'anthropic-messages'"
                  >
                    Anthropic Messages{{ selectedProvider.kind === 'anthropic-messages' ? '' : t('providers.anthropicSoon') }}
                  </option>
                </select>
              </label>
            </div>

            <label class="field">
              <span>{{ t('providers.baseUrl') }}</span>
              <input
                v-model="selectedProvider.baseUrl"
                type="url"
                spellcheck="false"
                placeholder="https://api.openai.com/v1"
              >
            </label>

            <label class="field">
              <span>{{ t('providers.apiKey') }}</span>
              <div class="key-row">
                <input
                  v-model="selectedProvider.apiKey"
                  :type="showKey ? 'text' : 'password'"
                  spellcheck="false"
                  autocomplete="off"
                  :placeholder="keyPlaceholder(selectedProvider)"
                  :disabled="selectedProvider.clearApiKey"
                  @input="onApiKeyInput(selectedProvider)"
                >
                <button
                  type="button"
                  class="ghost small"
                  @click="showKey = !showKey"
                >
                  {{ showKey ? t('providers.hideKey') : t('providers.showKey') }}
                </button>
                <button
                  v-if="selectedProvider.hasApiKey && !selectedProvider.clearApiKey"
                  type="button"
                  class="ghost small"
                  @click="clearSavedKey(selectedProvider)"
                >
                  {{ t('providers.clearKey') }}
                </button>
                <button
                  v-else-if="selectedProvider.clearApiKey"
                  type="button"
                  class="ghost small"
                  @click="undoClearKey(selectedProvider)"
                >
                  {{ t('providers.undoClearKey') }}
                </button>
              </div>
              <small
                v-if="selectedProvider.apiKeyRef && !selectedProvider.apiKeyHint"
                class="hint"
              >
                {{ t('providers.envHint', { ref: selectedProvider.apiKeyRef }) }}
              </small>
              <small class="hint">{{ t('providers.keyStorageHint') }}</small>
            </label>
          </div>

          <div class="card">
            <div class="card-head">
              <div>
                <h2>{{ t('providers.models') }}</h2>
                <p class="card-sub">
                  {{ t('providers.modelsHint') }}
                </p>
              </div>
              <button
                type="button"
                class="ghost"
                @click="addModel"
              >
                + {{ t('providers.addModel') }}
              </button>
            </div>

            <div
              v-if="models.length === 0"
              class="empty small"
            >
              {{ t('providers.noModels') }}
            </div>
            <div
              v-else
              class="model-list"
            >
              <div
                v-for="(model, index) in models"
                :key="index"
                class="model-card"
              >
                <div class="model-fields">
                  <label class="field">
                    <span>{{ t('providers.provider') }}</span>
                    <select v-model="model.provider">
                      <option
                        v-for="provider in providerOptions"
                        :key="provider.id"
                        :value="provider.id"
                      >
                        {{ provider.label }}
                      </option>
                    </select>
                  </label>
                  <label class="field">
                    <span>{{ t('providers.modelName') }}</span>
                    <input
                      v-model="model.id"
                      type="text"
                      spellcheck="false"
                    >
                  </label>
                  <label class="field">
                    <span>{{ t('providers.modelId') }}</span>
                    <input
                      v-model="model.modelId"
                      type="text"
                      spellcheck="false"
                    >
                  </label>
                  <label class="field">
                    <span>{{ t('providers.contextK') }}</span>
                    <input
                      v-model.number="model.contextK"
                      type="number"
                      min="1"
                    >
                  </label>
                  <label class="field">
                    <span>{{ t('providers.compressAtK') }}</span>
                    <input
                      v-model.number="model.compressAtK"
                      type="number"
                      min="1"
                    >
                  </label>
                  <label class="field">
                    <span>{{ t('providers.thinking') }}</span>
                    <input
                      v-model="model.thinkingText"
                      type="text"
                      spellcheck="false"
                      :placeholder="t('providers.thinkingPlaceholder')"
                    >
                  </label>
                </div>
                <button
                  type="button"
                  class="danger ghost small model-delete"
                  @click="removeModel(model, index)"
                >
                  {{ confirmModelIndex === index ? t('providers.confirmDelete') : t('providers.delete') }}
                </button>
              </div>
            </div>
          </div>

          <div class="card">
            <div class="card-head">
              <h2>{{ t('providers.defaults') }}</h2>
            </div>
            <div class="grid two">
              <label class="field">
                <span>{{ t('providers.defaultModel') }}</span>
                <select v-model="defaultModel">
                  <option
                    v-for="model in modelOptions"
                    :key="model.id"
                    :value="model.id"
                  >
                    {{ model.label }}
                  </option>
                </select>
                <small class="hint">{{ t('providers.defaultModelHint') }}</small>
              </label>
              <label class="field">
                <span>{{ t('providers.defaultThinking') }}</span>
                <select v-model="defaultThinking">
                  <option
                    v-for="level in defaultThinkingOptions"
                    :key="level"
                    :value="level"
                  >
                    {{ level }}
                  </option>
                </select>
              </label>
              <label class="field">
                <span>{{ t('providers.subagentModel') }}</span>
                <select v-model="subagentModel">
                  <option value="">
                    {{ t('providers.subagentNone') }}
                  </option>
                  <option
                    v-for="model in modelOptions"
                    :key="model.id"
                    :value="model.id"
                  >
                    {{ model.label }}
                  </option>
                </select>
                <small class="hint">{{ t('providers.subagentHint') }}</small>
              </label>
              <label class="field">
                <span>{{ t('providers.subagentThinking') }}</span>
                <select
                  v-model="subagentThinking"
                  :disabled="!subagentModel"
                >
                  <option value="">
                    {{ t('providers.subagentNone') }}
                  </option>
                  <option
                    v-for="level in subagentThinkingOptions"
                    :key="level"
                    :value="level"
                  >
                    {{ level }}
                  </option>
                </select>
              </label>
            </div>
          </div>
        </section>
      </div>
    </fieldset>
  </div>
</template>

<style scoped>
.providers-page {
  height: 100%;
  overflow: auto;
  padding: 18px 20px 28px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  width: 100%;
  max-width: 1180px;
  margin: 0 auto;
}

.form-body {
  border: 0;
  margin: 0;
  padding: 0;
  min-width: 0;
}

.page-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
  flex-wrap: wrap;
}

.page-head h1 {
  margin: 0 0 4px;
  font-size: 18px;
  font-weight: 650;
  color: var(--fg);
}

.page-head p {
  margin: 0;
  font-size: 12px;
  color: var(--fg-dim);
  line-height: 1.5;
}

.head-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.demo-badge {
  font-size: 11px;
  color: var(--warn, #b8860b);
  border: 1px solid var(--warn, #b8860b);
  border-radius: 999px;
  padding: 2px 8px;
}

.banner {
  border-radius: var(--r-sm);
  padding: 8px 10px;
  font-size: 12px;
  line-height: 1.5;
}

.banner.error {
  color: var(--danger, #d64545);
  border: 1px solid var(--danger, #d64545);
}

.banner.success {
  color: var(--green, #2f9e56);
  border: 1px solid var(--green, #2f9e56);
}

.layout {
  display: grid;
  grid-template-columns: 250px minmax(0, 1fr);
  gap: 12px;
  align-items: start;
}

.provider-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}

.list-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 0 2px 2px;
  font-size: 11px;
  color: var(--fg-dim);
}

.provider-card {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--panel-2);
  color: var(--fg-2);
  text-align: left;
  cursor: pointer;
}

.provider-card:hover {
  border-color: var(--accent-line);
}

.provider-card.active {
  border-color: var(--accent);
  background: var(--accent-soft, var(--panel-3));
  color: var(--fg);
}

.dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--fg-faint);
  flex: 0 0 auto;
}

.dot.on {
  background: var(--green, #2f9e56);
}

.provider-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
  flex: 1;
}

.provider-name {
  font-size: 12px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.provider-meta {
  font-size: 10px;
  color: var(--fg-faint);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.provider-count {
  font-size: 10px;
  color: var(--fg-faint);
  background: var(--bg);
  border-radius: 8px;
  padding: 0 6px;
}

.list-empty,
.empty {
  color: var(--fg-dim);
  font-size: 12px;
  padding: 14px 6px;
  text-align: center;
}

.empty.small {
  padding: 10px;
}

.detail {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
}

.card {
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: var(--r);
  padding: 14px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.card-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 10px;
  flex-wrap: wrap;
}

.card-head h2 {
  margin: 0;
  font-size: 13px;
  font-weight: 650;
  color: var(--fg);
}

.card-sub,
.hint {
  margin: 0;
  font-size: 10px;
  color: var(--fg-faint);
  line-height: 1.5;
}

.card-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.grid.two {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px 14px;
}

.field {
  display: flex;
  flex-direction: column;
  gap: 5px;
  font-size: 11px;
  color: var(--fg-dim);
  min-width: 0;
}

.field input,
.field select {
  height: 30px;
  padding: 0 9px;
  border: 1px solid var(--border);
  border-radius: var(--r-xs);
  background: var(--bg);
  color: var(--fg);
  font-size: 12px;
  min-width: 0;
}

.field input:focus,
.field select:focus {
  outline: none;
  border-color: var(--accent);
}

.field input:disabled {
  opacity: 0.6;
}

.key-row {
  display: flex;
  gap: 6px;
  min-width: 0;
}

.key-row input {
  flex: 1;
  min-width: 0;
}

.switch {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
  color: var(--fg-dim);
  cursor: pointer;
}

.model-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.model-card {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 10px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--bg);
}

.model-fields {
  flex: 1;
  min-width: 0;
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 8px 10px;
}

.model-delete {
  align-self: flex-end;
}

@media (max-width: 760px) {
  .model-fields {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}

@media (max-width: 560px) {
  .model-fields {
    grid-template-columns: 1fr;
  }
}

button {
  font: inherit;
}

.primary {
  height: 30px;
  padding: 0 14px;
  border-radius: var(--r-sm);
  border: 1px solid var(--accent);
  background: var(--accent);
  color: var(--accent-fg, #fff);
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}

.primary:disabled {
  opacity: 0.55;
  cursor: default;
}

.ghost {
  height: 28px;
  padding: 0 9px;
  border: 1px solid var(--border);
  border-radius: var(--r-xs);
  background: transparent;
  color: var(--fg-2);
  font-size: 11px;
  cursor: pointer;
}

.ghost:hover {
  background: var(--panel-3);
  color: var(--fg);
}

.ghost.small {
  height: 30px;
  padding: 0 8px;
}

.danger {
  color: var(--danger, #d64545);
}

.danger:hover {
  border-color: var(--danger, #d64545);
  background: color-mix(in srgb, var(--danger, #d64545) 10%, transparent);
}

@media (max-width: 980px) {
  .layout {
    grid-template-columns: 1fr;
  }

  .grid.two {
    grid-template-columns: 1fr;
  }
}
</style>
