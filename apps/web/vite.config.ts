import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'

import webPackage from './package.json' with { type: 'json' }

export default defineConfig({
  plugins: [vue()],
  define: {
    __APP_VERSION__: JSON.stringify(webPackage.version),
  },
  test: {
    environment: 'node',
    include: ['src/**/*.spec.ts'],
  },
})
