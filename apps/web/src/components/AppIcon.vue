<script setup lang="ts">
withDefaults(defineProps<{ name: string; size?: number }>(), { size: 18 })
</script>

<template>
  <svg
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.7"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <template v-if="name === 'explorer'">
      <path d="M4 4h6l2 2h8v14H4z" />
      <path d="M4 9h16" />
    </template>
    <template v-else-if="name === 'search'">
      <circle
        cx="10.5"
        cy="10.5"
        r="6.5"
      />
      <path d="m16 16 4 4" />
    </template>
    <template v-else-if="name === 'git'">
      <circle
        cx="6"
        cy="5"
        r="2.2"
      />
      <circle
        cx="6"
        cy="19"
        r="2.2"
      />
      <circle
        cx="18"
        cy="12"
        r="2.2"
      />
      <path d="M6 7.2v9.6M8.2 5H14a4 4 0 0 1 4 4v.8" />
    </template>
    <template v-else-if="name === 'db'">
      <ellipse
        cx="12"
        cy="5.5"
        rx="7"
        ry="2.8"
      />
      <path d="M5 5.5v6c0 1.5 3.1 2.8 7 2.8s7-1.3 7-2.8v-6" />
      <path d="M5 11.5v6c0 1.5 3.1 2.8 7 2.8s7-1.3 7-2.8v-6" />
    </template>
    <template v-else-if="name === 'tasks'">
      <path d="M9 6h11M9 12h11M9 18h11" />
      <path d="m4 6 1 1 2-2M4 12l1 1 2-2M4 18l1 1 2-2" />
    </template>
    <template v-else-if="name === 'agent'">
      <rect
        x="4"
        y="6"
        width="16"
        height="12"
        rx="3"
      />
      <path d="M9 11h.01M15 11h.01M9 15h6" />
    </template>
    <template v-else-if="name === 'providers'">
      <circle
        cx="8"
        cy="8"
        r="3.2"
      />
      <path d="m10.4 10.4 9.1 9.1M14.8 14.8l-2 2M17.8 17.8l-2 2" />
    </template>
    <template v-else-if="name === 'extensions'">
      <path d="M19.4 7.9a1 1 0 0 0 .3.9l1.6 1.6a2.4 2.4 0 0 1 0 3.4l-1.6 1.6a1 1 0 0 1-.8.3c-.5-.1-.8-.5-1-1a2.5 2.5 0 1 0-3.2 3.2c.5.2.9.5 1 1a1 1 0 0 1-.3.8l-1.6 1.6a2.4 2.4 0 0 1-3.4 0l-1.6-1.6a1 1 0 0 0-.9-.3c-.5.1-.8.5-1 1a2.5 2.5 0 1 1-3.2-3.2c.5-.2.9-.5 1-1a1 1 0 0 0-.3-.9L2.6 13.8a2.4 2.4 0 0 1 0-3.4l1.6-1.6a1 1 0 0 1 .9-.3c.5.1.8.5 1 1a2.5 2.5 0 1 0 3.2-3.2c-.5-.2-.9-.5-1-1a1 1 0 0 1 .3-.9l1.6-1.6a2.4 2.4 0 0 1 3.4 0l1.6 1.6a1 1 0 0 0 .9.3c.5-.1.8-.5 1-1a2.5 2.5 0 1 1 3.2 3.2c-.5.2-.9.5-1 1Z" />
    </template>
    <template v-else-if="name === 'settings'">
      <circle
        cx="12"
        cy="12"
        r="3"
      />
      <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1-2 2-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.6V21h-3v-.8a1.7 1.7 0 0 0-1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1-2-2 .1-.1A1.7 1.7 0 0 0 7.4 15a1.7 1.7 0 0 0-1.6-1H5v-3h.8a1.7 1.7 0 0 0 1.6-1 1.7 1.7 0 0 0-.3-1.9l-.1-.1 2-2 .1.1a1.7 1.7 0 0 0 1.9.3 1.7 1.7 0 0 0 1-1.6V3h3v.8a1.7 1.7 0 0 0 1 1.6 1.7 1.7 0 0 0 1.9-.3l.1-.1 2 2-.1.1a1.7 1.7 0 0 0-.3 1.9 1.7 1.7 0 0 0 1.6 1H21v3h-.8a1.7 1.7 0 0 0-1.6 1Z" />
    </template>
    <template v-else-if="name === 'sun'">
      <circle
        cx="12"
        cy="12"
        r="4"
      />
      <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
    </template>
    <template v-else-if="name === 'moon'">
      <path d="M20 14.5A8.5 8.5 0 0 1 9.5 4 8.5 8.5 0 1 0 20 14.5Z" />
    </template>
    <template v-else-if="name === 'plus'">
      <path d="M12 5v14M5 12h14" />
    </template>
    <template v-else-if="name === 'refresh'">
      <path d="M20 11a8 8 0 1 0-2.3 6.3M20 5v6h-6" />
    </template>
    <template v-else-if="name === 'chevron'">
      <path d="m9 6 6 6-6 6" />
    </template>
    <template v-else-if="name === 'folder'">
      <path d="M3 6h7l2 2h9v11H3z" />
    </template>
    <template v-else-if="name === 'smart'">
      <path d="M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M5.6 18.4l2.1-2.1M16.3 7.7l2.1-2.1" />
    </template>
    <template v-else-if="name === 'chevron-down'">
      <path d="m6 9 6 6 6-6" />
    </template>
    <template v-else-if="name === 'arrow-up'">
      <path d="M12 19V5M5 12l7-7 7 7" />
    </template>
    <template v-else-if="name === 'paperclip'">
      <path d="M21 11.5 12.5 20a5 5 0 0 1-7-7l8-8a3.5 3.5 0 0 1 5 5l-8 8a2 2 0 0 1-3-3l7-7" />
    </template>
    <template v-else-if="name === 'square'">
      <rect
        x="7"
        y="7"
        width="10"
        height="10"
        rx="1.5"
      />
    </template>
    <template v-else-if="name === 'file'">
      <path d="M6 3h8l4 4v14H6z" />
      <path d="M14 3v5h5" />
    </template>
    <template v-else-if="name === 'pencil'">
      <path d="m4 20 4.5-1 10-10a2.1 2.1 0 0 0-3-3l-10 10z" />
      <path d="m14 7 3 3" />
    </template>
    <template v-else-if="name === 'terminal'">
      <path d="m5 7 5 5-5 5M13 17h6" />
    </template>
    <template v-else-if="name === 'layers'">
      <path d="m12 3 9 5-9 5-9-5z" />
      <path d="m3 13 9 5 9-5" />
    </template>
    <template v-else-if="name === 'shield'">
      <path d="M12 3 5 6v6c0 4 3 7 7 9 4-2 7-5 7-9V6z" />
      <path d="m9 12 2 2 4-4" />
    </template>
    <template v-else-if="name === 'x'">
      <path d="m6 6 12 12M18 6 6 18" />
    </template>
  </svg>
</template>
