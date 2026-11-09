// ESLint 9 flat config。lint 范围包含 services 层的框架禁令（UI.md §8）。
import js from '@eslint/js'
import pluginVue from 'eslint-plugin-vue'
import tseslint from 'typescript-eslint'

export default tseslint.config(
  { ignores: ['dist/', 'dist-ohos/', 'node_modules/'] },
  js.configs.recommended,
  // TS 已接管未定义全局的检查（window/HTMLElement 等），no-undef 会误报
  { rules: { 'no-undef': 'off' } },
  ...tseslint.configs.recommended,
  ...pluginVue.configs['flat/recommended'],
  {
    files: ['**/*.vue'],
    languageOptions: { parserOptions: { parser: tseslint.parser } },
  },
  {
    // services 层禁止 import 任何框架 API：只暴露纯函数与 subscribe()。
    // 这让将来换框架只重写视图层。
    files: ['src/services/**/*.ts'],
    rules: {
      'no-restricted-imports': [
        'error',
        {
          patterns: [
            {
              group: ['vue', 'vue-i18n', '@tauri-apps/*', 'vue*'],
              message: 'services 层禁止 import 框架 API，只暴露纯函数与 subscribe()',
            },
          ],
        },
      ],
    },
  },
)
