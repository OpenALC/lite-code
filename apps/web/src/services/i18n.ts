// i18n 纯函数层：目录与查找逻辑不依赖框架。
// vue-i18n 实例在视图层（src/i18n.ts）创建。
import zhCN from '../locales/zh-CN.json'
import en from '../locales/en.json'

export interface Catalog {
  [key: string]: unknown
}

// 保留 JSON 推断出的具体类型（vue-i18n 需要具体形状），同时约束两份目录同为 Catalog
export const catalogs = {
  'zh-CN': zhCN,
  en,
} satisfies Record<Locale, Catalog>

export type Locale = 'zh-CN' | 'en'

export const DEFAULT_LOCALE: Locale = 'zh-CN'

/** 按 a.b.c 形式查找目录项；缺失返回 undefined（视图层回退 key 本身）。 */
export function lookup(catalog: Catalog, key: string): string | undefined {
  let node: unknown = catalog
  for (const part of key.split('.')) {
    if (node === null || typeof node !== 'object') return undefined
    node = (node as Record<string, unknown>)[part]
  }
  return typeof node === 'string' ? node : undefined
}

export function createT(locale: Locale): (key: string) => string {
  const catalog = catalogs[locale]
  return (key: string) => lookup(catalog, key) ?? lookup(catalogs.en, key) ?? key
}

/** 展开目录的全部叶子 key（用于双语 key 一致性校验）。 */
export function flattenKeys(catalog: Catalog, prefix = ''): string[] {
  const out: string[] = []
  for (const [k, v] of Object.entries(catalog)) {
    const key = prefix ? `${prefix}.${k}` : k
    if (v !== null && typeof v === 'object') {
      out.push(...flattenKeys(v as Catalog, key))
    } else {
      out.push(key)
    }
  }
  return out.sort()
}
