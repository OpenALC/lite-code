// 视图层的 i18n 实例。目录与查找逻辑在 services/i18n.ts（无框架依赖）。
import { createI18n } from 'vue-i18n'

import { catalogs, DEFAULT_LOCALE } from './services/i18n'

export const i18n = createI18n({
  legacy: false,
  locale: DEFAULT_LOCALE,
  fallbackLocale: 'en',
  messages: catalogs,
})
