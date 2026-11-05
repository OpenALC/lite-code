<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { TaskController } from '../services/taskStore'

const props = defineProps<{ controller: TaskController }>()
const emit = defineEmits<{ reveal: [] }>()
const { t } = useI18n()

const state = ref(props.controller.getState())
const unsubscribe = props.controller.subscribe(next => {
  state.value = next
})
onBeforeUnmount(unsubscribe)

const count = computed(() => state.value.needsInputCount)

function jump(): void {
  if (props.controller.activateFirstNeedsInput()) emit('reveal')
}
</script>

<template>
  <button
    type="button"
    class="needs"
    :class="{ idle: count === 0 }"
    :disabled="count === 0"
    :title="t('task.needsInputBadge')"
    @click="jump"
  >
    <span class="dot" />
    {{ t('task.needsInput', { count }) }}
  </button>
</template>

<style scoped>
.needs {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 18px;
  padding: 0 7px;
  border: 1px solid rgb(217 174 95 / 45%);
  border-radius: 9px;
  color: var(--yellow);
  font-size: 10px;
  line-height: 1;
}

.needs:hover:not(:disabled) {
  background: rgb(217 174 95 / 10%);
}

.needs.idle {
  border-color: var(--border);
  color: var(--fg-faint);
  cursor: default;
}

.needs.idle .dot {
  background: var(--fg-faint);
}

.dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: var(--yellow);
}
</style>
