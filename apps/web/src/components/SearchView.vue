<script setup lang="ts">
// 搜索侧栏：VS Code 式全文搜索。Rust 侧遍历工作区（尊重 .gitignore），
// 前端只负责输入、分组展示与点击定位。
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { readFile, searchWorkspace, type SearchMatch, type SearchResult } from '../services/bridge'
import { requestReveal } from '../services/editorReveal'
import { resolveEditorStrategy } from '../services/largeFile'
import { activateTab, openTab, tabsStore } from '../services/tabs'
import AppIcon from './AppIcon.vue'

const props = defineProps<{ root: string; seed?: { query: string; seq: number } }>()

const { t } = useI18n()
const query = ref('')
const caseSensitive = ref(false)
const regex = ref(false)
const result = ref<SearchResult | null>(null)
const loading = ref(false)
const error = ref<string | null>(null)
const isFallback = ref(false)

let timer: ReturnType<typeof setTimeout> | null = null
let requestSeq = 0

const summary = computed(() => {
  const data = result.value
  if (!data) return ''
  return t('search.summary', { matches: data.totalMatches, files: data.files.length })
})

async function run(q: string): Promise<void> {
  const current = ++requestSeq
  const res = await searchWorkspace(q, {
    caseSensitive: caseSensitive.value,
    regex: regex.value,
  })
  if (current !== requestSeq) return
  loading.value = false
  if (!res.ok) {
    // 非法正则用本地化文案，不把 Rust 的中文错误直接暴露到英文界面。
    error.value =
      res.error.code === 'invalid_argument' ? t('search.invalidRegex') : res.error.message
    result.value = null
    return
  }
  isFallback.value = res.fallback
  error.value = null
  result.value = res.data
}

function schedule(): void {
  if (timer) clearTimeout(timer)
  requestSeq += 1
  const q = query.value.trim()
  if (!q) {
    result.value = null
    error.value = null
    loading.value = false
    isFallback.value = false
    return
  }
  error.value = null
  loading.value = true
  timer = setTimeout(() => {
    void run(q)
  }, 280)
}

watch([query, caseSensitive, regex], schedule)

// 顶部快速打开里的“在文件中搜索”：把关键词带进侧栏并立即执行。
watch(
  () => props.seed?.seq,
  () => {
    const seed = props.seed
    if (!seed || !seed.query) return
    query.value = seed.query
    schedule()
  },
  { immediate: true },
)

// 切换工作区后旧命中路径已不在新根内：清空结果并按当前关键词重跑。
watch(
  () => props.root,
  () => {
    requestSeq += 1
    result.value = null
    error.value = null
    isFallback.value = false
    if (query.value.trim()) schedule()
  },
)

function relativePath(path: string): string {
  const normalized = path.replace(/\\/g, '/')
  const root = props.root.replace(/\\/g, '/').replace(/\/+$/, '')
  if (root && normalized.toLowerCase().startsWith(`${root.toLowerCase()}/`)) {
    return normalized.slice(root.length + 1)
  }
  return normalized
}

function splitPreview(match: SearchMatch): { before: string; hit: string; after: string } {
  const chars = Array.from(match.preview)
  const start = Math.min(Math.max(0, match.matchStart), chars.length)
  const end = Math.min(start + Math.max(0, match.length), chars.length)
  return {
    before: chars.slice(0, start).join(''),
    hit: chars.slice(start, end).join(''),
    after: chars.slice(end).join(''),
  }
}

async function openHit(path: string, line: number, column: number): Promise<void> {
  const existing = tabsStore.get().tabs.find(tab => tab.path === path)
  if (existing) {
    activateTab(existing.id)
    requestReveal(existing.id, line, column)
    return
  }
  const res = await readFile(props.root, path)
  if (!res.ok) {
    error.value = res.error.message
    return
  }
  openTab({
    id: path,
    title: path.replace(/\\/g, '/').split('/').pop() ?? path,
    path,
    content: res.data.text,
    strategy: resolveEditorStrategy(res.data.text.length),
  })
  requestReveal(path, line, column)
}

onBeforeUnmount(() => {
  if (timer) clearTimeout(timer)
  requestSeq += 1
})
</script>

<template>
  <div class="search-view">
    <div
      class="search-box"
      :class="{ invalid: Boolean(error) }"
    >
      <AppIcon
        name="search"
        :size="14"
      />
      <input
        v-model="query"
        type="text"
        :placeholder="t('search.workspacePlaceholder')"
        spellcheck="false"
        @keydown.enter="schedule"
      >
      <button
        type="button"
        class="toggle"
        :class="{ active: caseSensitive }"
        :title="t('search.caseSensitive')"
        @click="caseSensitive = !caseSensitive"
      >
        Aa
      </button>
      <button
        type="button"
        class="toggle"
        :class="{ active: regex }"
        :title="t('search.regex')"
        @click="regex = !regex"
      >
        .*
      </button>
    </div>

    <div
      v-if="loading"
      class="meta"
    >
      {{ t('search.loading') }}
    </div>
    <div
      v-else-if="error"
      class="error"
    >
      {{ error }}
    </div>
    <template v-else-if="result">
      <div class="meta">
        <span>{{ summary }}</span>
        <span
          v-if="result.truncated"
          class="warn"
        >{{ t('search.truncated') }}</span>
      </div>
      <div
        v-if="isFallback"
        class="meta demo"
      >
        {{ t('search.demo') }}
      </div>
      <div
        v-if="result.files.length === 0"
        class="empty"
      >
        {{ t('search.noResults') }}
      </div>
      <div
        v-for="file in result.files"
        :key="file.path"
        class="file-group"
      >
        <button
          type="button"
          class="file-head"
          :title="relativePath(file.path)"
          @click="openHit(file.path, file.matches[0].line, file.matches[0].column)"
        >
          <span class="file-name">{{ relativePath(file.path).split('/').pop() }}</span>
          <span class="file-dir">{{ relativePath(file.path).split('/').slice(0, -1).join('/') }}</span>
          <span class="file-count">{{ file.matches.length }}</span>
        </button>
        <button
          v-for="match in file.matches"
          :key="`${file.path}:${match.line}:${match.matchStart}`"
          type="button"
          class="hit"
          @click="openHit(file.path, match.line, match.column)"
        >
          <span class="hit-line">{{ match.line }}</span>
          <span class="hit-preview">
            <span>{{ splitPreview(match).before }}</span><mark>{{ splitPreview(match).hit }}</mark><span>{{ splitPreview(match).after }}</span>
          </span>
        </button>
      </div>
    </template>
    <div
      v-else
      class="empty hint"
    >
      {{ t('search.workspaceHint') }}
    </div>
  </div>
</template>

<style scoped>
.search-view {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  overflow: auto;
  padding: 10px;
  gap: 8px;
}

.search-box {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 8px;
  height: 32px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--input-bg, var(--bg));
  color: var(--fg-dim);
  flex: 0 0 auto;
}

.search-box:focus-within {
  border-color: var(--accent);
  box-shadow: 0 0 0 2px var(--accent-soft, transparent);
}

.search-box.invalid {
  border-color: var(--danger, #d64545);
}

.search-box input {
  flex: 1;
  min-width: 0;
  border: 0;
  outline: 0;
  background: transparent;
  color: var(--fg);
  font-size: 12px;
}

.toggle {
  border: 1px solid transparent;
  background: transparent;
  color: var(--fg-dim);
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1;
  padding: 3px 4px;
  border-radius: 4px;
  cursor: pointer;
}

.toggle:hover {
  background: var(--hover);
  color: var(--fg);
}

.toggle.active {
  border-color: var(--accent);
  color: var(--accent);
  background: var(--accent-soft, transparent);
}

.meta {
  font-size: 11px;
  color: var(--fg-dim);
  padding: 0 2px;
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

.meta.demo {
  color: var(--warn, #b8860b);
}

.warn {
  color: var(--warn, #b8860b);
}

.error {
  font-size: 12px;
  color: var(--danger, #d64545);
  padding: 6px 8px;
  border: 1px solid var(--danger, #d64545);
  border-radius: var(--r-sm);
  word-break: break-word;
}

.empty {
  font-size: 12px;
  color: var(--fg-dim);
  padding: 12px 4px;
  text-align: center;
}

.empty.hint {
  text-align: left;
  line-height: 1.6;
}

.file-group {
  display: flex;
  flex-direction: column;
}

.file-head {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  border: 0;
  background: transparent;
  color: var(--fg);
  font-size: 12px;
  text-align: left;
  padding: 5px 6px;
  border-radius: 4px;
  cursor: pointer;
}

.file-head:hover {
  background: var(--hover);
}

.file-name {
  font-weight: 600;
  white-space: nowrap;
}

.file-dir {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--fg-dim);
  font-size: 11px;
}

.file-count {
  flex: 0 0 auto;
  color: var(--fg-dim);
  font-size: 10px;
  background: var(--panel-2);
  border-radius: 8px;
  padding: 0 6px;
}

.hit {
  display: flex;
  gap: 8px;
  width: 100%;
  border: 0;
  background: transparent;
  color: var(--fg-2);
  font-family: var(--font-mono);
  font-size: 11px;
  text-align: left;
  padding: 2px 6px 2px 10px;
  border-radius: 4px;
  cursor: pointer;
}

.hit:hover {
  background: var(--hover);
}

.hit-line {
  flex: 0 0 34px;
  color: var(--fg-dim);
  text-align: right;
  user-select: none;
}

.hit-preview {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  white-space: pre;
  text-overflow: ellipsis;
}

.hit-preview mark {
  background: var(--accent-soft, rgba(58, 127, 255, 0.22));
  color: inherit;
  border-radius: 2px;
}
</style>
