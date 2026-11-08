<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'

import AppIcon from './AppIcon.vue'
import { tabsStore, type TabsState } from '../services/tabs'

const state = ref<TabsState>(tabsStore.get())
const unsubscribe = tabsStore.subscribe(next => {
  state.value = next
})
onBeforeUnmount(unsubscribe)

const tab = computed(() =>
  state.value.tabs.find(item => item.id === state.value.activeId),
)
const segments = computed(() => {
  const path = tab.value?.path
  if (!path) return []
  return path.replace(/\\/g, '/').split('/').filter(Boolean).slice(-5)
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
      :key="`${segment}-${index}`"
    >
      <span class="crumb-sep">›</span>
      <span
        class="crumb"
        :class="{ last: index === segments.length - 1 }"
      >{{ segment }}</span>
    </template>
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
  overflow: hidden;
  text-overflow: ellipsis;
}

.crumb.last {
  color: var(--fg-2);
}

.crumb-sep {
  color: var(--fg-faint);
}
</style>
