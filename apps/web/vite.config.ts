import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'

import webPackage from './package.json' with { type: 'json' }

// ohos 是独立 MPA 入口：只打包 src/ohos/*，不把 App.vue / Monaco 带进构建图。
export default defineConfig(({ mode }) => ({
  plugins: [vue()],
  define: {
    __APP_VERSION__: JSON.stringify(webPackage.version),
  },
  // ohos 产物只应包含 ohos.html + assets/*；public/ 是桌面入口的图标资源。
  publicDir: mode === 'ohos' ? false : undefined,
  build:
    mode === 'ohos'
      ? {
          outDir: 'dist-ohos',
          rollupOptions: { input: 'ohos.html' },
        }
      : undefined,
  test: {
    environment: 'node',
    include: ['src/**/*.spec.ts'],
  },
}))
