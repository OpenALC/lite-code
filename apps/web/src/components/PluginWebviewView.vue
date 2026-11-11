<script setup lang="ts">
// 插件沙箱 webview 容器（docs/plans/plugin-webview.md §3.8 M3）：sandbox iframe
// + 一次性握手 + 专属 MessagePort 桥。注入防御红线：
// - iframe src 唯一来源 = pluginAssetUrl(pluginId, webviewId, entry) + 一次性
//   nonce fragment；组件不渲染插件提供的任何其它 URL/图片，全组件零 v-html；
// - sandbox 精确值 allow-scripts（不带 allow-same-origin → opaque origin、
//   无存储、禁父访问；event.origin "null" 是预期格式，不参与身份判定）；
// - 身份与授权只在服务层（services/pluginWebview.ts）：握手校验
//   event.source === contentWindow + nonce + 代次，桥 op 闭集
//   getState/setState/postCommand，导航/重载/卸载即拆桥。
// 生命周期：src 变化（受控重载/清单入口变更）→ 拆旧桥建新桥，新 nonce 重新
// 握手，旧端口不复用；插件禁用/删除 → 标签关闭（applyPluginWebviewList），
// 组件卸载随之 dispose。
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  createWebviewBridge,
  findWebview,
  getPluginWebviewState,
  pluginAssetUrl,
  subscribePluginWebview,
  type PluginWebviewState,
  type WebviewBridgeCloseReason,
  type WebviewBridgeStatus,
  type WebviewChildWindow,
} from '../services/pluginWebview'

const props = defineProps<{ pluginId: string; webviewId: string }>()

const { t } = useI18n()

const state = ref<PluginWebviewState>(getPluginWebviewState())
const unsubscribe = subscribePluginWebview(next => {
  state.value = next
})
onBeforeUnmount(() => {
  unsubscribe()
})

const entry = computed(() => findWebview(props.pluginId, props.webviewId))
const pluginName = computed(() => entry.value?.pluginName ?? props.pluginId)

// ==== iframe + 桥生命周期 ====

const iframeEl = ref<HTMLIFrameElement | null>(null)
const bridgeStatus = ref<WebviewBridgeStatus>('awaiting')
const bridgeReason = ref<WebviewBridgeCloseReason | null>(null)
/** 当前代次 nonce（拼进 src fragment；新桥 = 新 nonce = 强制重载重握手）。 */
const bridgeNonce = ref('')
/** 宿主生成的入口 URL（不含 fragment）；失败时 frameUrlError 非空。 */
const frameBase = ref('')
const frameUrlError = ref<{ code: string; message: string } | null>(null)

let bridge: ReturnType<typeof createWebviewBridge> | null = null

function detachBridge(): void {
  bridge?.dispose()
  bridge = null
}

function attachBridge(): void {
  const frame = iframeEl.value
  const target = entry.value
  if (!frame || !target || !frame.contentWindow) return
  // 受控重载/入口变更：旧桥整代作废（关端口、清 nonce、丢弃在途响应）。
  detachBridge()
  bridge = createWebviewBridge({
    pluginId: props.pluginId,
    webviewId: props.webviewId,
    contentWindow: frame.contentWindow as unknown as WebviewChildWindow,
    onStatus: (status, reason) => {
      bridgeStatus.value = status
      bridgeReason.value = reason
    },
  })
  bridgeNonce.value = bridge.nonce
}

/** 入口 URL 只经宿主生成器（§3.8）；失败显示错误态（可重试），不给坏地址。 */
async function loadFrameUrl(): Promise<void> {
  const target = entry.value
  if (!target) {
    frameBase.value = ''
    frameUrlError.value = null
    return
  }
  const res = await pluginAssetUrl(props.pluginId, props.webviewId, target.entry)
  // 入口在途变化（清单热更新/关标签）后到达的旧结果不再回写。
  if (entry.value !== target) return
  if (!res.ok) {
    frameBase.value = ''
    frameUrlError.value = { code: res.error.code, message: res.error.message }
    return
  }
  frameBase.value = res.data.url
  frameUrlError.value = null
}

/** iframe src：唯一合法来源是统一生成器（§3.8），fragment 携带一次性 nonce。 */
const frameSrc = computed(() => {
  if (frameBase.value.length === 0 || bridgeNonce.value.length === 0) return ''
  return `${frameBase.value}#${bridgeNonce.value}`
})

function onFrameLoad(): void {
  bridge?.notifyLoaded()
}

function onFrameError(): void {
  bridge?.notifyFailed()
}

function reloadFrame(): void {
  attachBridge()
  void loadFrameUrl()
}

// iframe 挂载（flush post 保证 contentWindow 可用）、清单入口变更与 URL 加载。
watch([iframeEl, entry], () => attachBridge(), { immediate: true, flush: 'post' })
watch(entry, () => void loadFrameUrl(), { immediate: true })

onBeforeUnmount(() => {
  detachBridge()
})
</script>

<template>
  <div class="plugin-webview">
    <header class="wv-head">
      <h1>{{ props.webviewId }}</h1>
      <span
        class="wv-source"
        :title="entry?.pluginId"
      >{{ t('views.fromPlugin', { name: pluginName }) }}</span>
    </header>

    <p
      v-if="!entry"
      class="wv-note error"
    >
      {{ t('webview.unavailable') }}
    </p>

    <template v-else>
      <!-- §3.8 sandbox 精确值：allow-scripts（不带 allow-same-origin）。
        src 只来自宿主生成器（pluginAssetUrl）+ nonce fragment。 -->
      <iframe
        v-if="!frameUrlError"
        ref="iframeEl"
        class="wv-frame"
        :sandbox="'allow-scripts'"
        :src="frameSrc"
        :title="props.webviewId"
        @load="onFrameLoad"
        @error="onFrameError"
      />

      <div
        v-if="frameUrlError || (bridgeStatus === 'closed' && bridgeReason !== 'dispose')"
        class="wv-lost"
      >
        <p
          class="wv-note error"
          :title="frameUrlError ? `${frameUrlError.code}: ${frameUrlError.message}` : ''"
        >
          {{ frameUrlError ? t('webview.loadFailed') : bridgeReason === 'navigation' ? t('webview.bridgeLost') : t('webview.loadFailed') }}
        </p>
        <button
          type="button"
          class="wv-reload"
          @click="reloadFrame"
        >
          {{ t('webview.reload') }}
        </button>
      </div>
    </template>
  </div>
</template>

<style scoped>
.plugin-webview {
  height: 100%;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 14px 16px 16px;
  min-width: 0;
}

.wv-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
  flex: none;
  min-width: 0;
}

.wv-head h1 {
  margin: 0;
  font-size: 14px;
  font-weight: 650;
  color: var(--fg);
}

.wv-source {
  font-size: 11px;
  color: var(--fg-faint);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.wv-frame {
  flex: 1;
  min-height: 0;
  width: 100%;
  border: 1px solid var(--border);
  border-radius: var(--r);
  background: var(--bg);
}

.wv-note {
  margin: 0;
  font-size: 11px;
  color: var(--fg-faint);
  line-height: 1.5;
}

.wv-note.error {
  color: var(--red);
}

.wv-lost {
  display: flex;
  align-items: center;
  gap: 10px;
  flex: none;
  min-width: 0;
}

.wv-reload {
  flex: none;
  height: 24px;
  padding: 0 10px;
  border: 1px solid var(--accent-line);
  border-radius: var(--r-xs);
  background: var(--accent-soft);
  color: var(--accent);
  font-size: 11px;
}

.wv-reload:hover {
  background: var(--accent);
  color: var(--accent-fg);
}
</style>
