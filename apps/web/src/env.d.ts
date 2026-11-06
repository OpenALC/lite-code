/// <reference types="vite/client" />

/** 构建时由 vite.config.ts 的 define 注入（取 apps/web/package.json 的 version）。 */
declare const __APP_VERSION__: string
