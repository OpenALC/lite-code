#!/usr/bin/env node
// 把 apps/db-bridge 的 ESM 源码与 pg/mysql2/mssql 打包成单文件，直接写进
// crates/lc-core/assets/db-bridge.mjs（Rust 用 include_str! 安装到数据目录）。
//
// --check 只比较产物，不写文件；CI 用它确认提交的 bundle 与源码同步。

import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { build } from 'esbuild';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const repoRoot = path.resolve(appRoot, '..', '..');
const entry = path.join(appRoot, 'src', 'index.mjs');
const target = path.join(repoRoot, 'crates', 'lc-core', 'assets', 'db-bridge.mjs');
const checkOnly = process.argv.includes('--check');

const result = await build({
  entryPoints: [entry],
  bundle: true,
  platform: 'node',
  format: 'esm',
  target: 'node22',
  write: false,
  outfile: target,
  minify: true,
  sourcemap: false,
  // 保留第三方包的 license/@preserve 注释，避免分发时丢失版权声明。
  legalComments: 'eof',
  // ESM 产物没有 require，而 pg/mysql2/mssql 的 CJS 代码仍会用 require 加载
  // Node 内置模块。注入 createRequire，esbuild 的 external 才能正常解析。
  // src/index.mjs 的 shebang 由 esbuild 保留；不要再放 shebang banner。
  banner: {
    js: "import { createRequire as __lcCreateRequire } from 'node:module';\nconst require = __lcCreateRequire(import.meta.url);",
  },
  // node:sqlite/readline 等由 Node 提供；pg-native 是可选的本地加速器，
  // 未安装时 pg 会回退到纯 JS 实现。
  external: ['node:*', 'pg-native'],
  define: { 'process.env.NODE_ENV': '"production"' },
  logLevel: 'warning',
});

const output = result.outputFiles?.[0]?.contents;
if (!output) {
  console.error('db-bridge bundle 没有产出');
  process.exit(1);
}

if (checkOnly) {
  let current;
  try {
    current = await readFile(target);
  } catch {
    console.error(`缺少 ${path.relative(repoRoot, target)}，请运行 pnpm db:bridge:build`);
    process.exit(1);
  }
  if (!current.equals(output)) {
    console.error(
      `${path.relative(repoRoot, target)} 与 apps/db-bridge 源码不同步；请运行 pnpm db:bridge:build 并提交产物`,
    );
    process.exit(1);
  }
  console.log(
    `db-bridge bundle 已同步（${(output.length / 1024 / 1024).toFixed(2)} MiB）`,
  );
} else {
  await writeFile(target, output);
  console.log(
    `db-bridge bundle -> ${path.relative(repoRoot, target)}（${(output.length / 1024 / 1024).toFixed(2)} MiB）`,
  );
}
