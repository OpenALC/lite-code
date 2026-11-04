<script setup lang="ts">
import { useI18n } from 'vue-i18n'

const { t } = useI18n()
</script>

<template>
  <div class="agent-placeholder">
    <div class="agent-card">
      <h2>{{ t('agent.placeholderTitle') }}</h2>
      <p>{{ t('agent.placeholderDesc') }}</p>
    </div>
  </div>
</template>

<style scoped>
.agent-placeholder {
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--bg);
}

.agent-card {
  max-width: 420px;
  padding: 24px;
  border: 1px dashed var(--border-strong);
  border-radius: var(--r);
  color: var(--fg-dim);
}

.agent-card h2 {
  margin: 0 0 8px;
  font-size: 14px;
  color: var(--fg-2);
}

.agent-card p {
  margin: 0;
  font-size: 12px;
  line-height: 1.6;
}
</style>
