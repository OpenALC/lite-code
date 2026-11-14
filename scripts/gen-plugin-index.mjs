#!/usr/bin/env node
// 生成插件市场静态 index（docs/plans/plugin-marketplace.md §3.7 发布端约定）。
//
// 无第三方依赖，CI 离线可跑：
//   node scripts/gen-plugin-index.mjs --help
//
// 输入布局（curated 市场仓库的 PR 制约定）：
//   <plugins>/<id>/<version>/plugin.json   插件清单（逐字内嵌进 index）
//   <plugins>/<id>/<version>/package.zip   安装包（客户端按 sha256 校验）
//   <plugins>/<id>/<version>/package.zip.sig  可选 minisign 签名（文本）
//
// 输出：{ "version": 1, "plugins": [ { id, version, manifest, dist: { url, sha256 },
// signature?, engines?, deps? } ] }，按 id 升序、version 升序。
//
// 设计约束（与 gen-update-manifest.mjs 同风格）：
//   - 只描述产物，不下载、不安装、不执行任何代码；
//   - 没有 .sig 时 signature=""，不伪造签名；--require-signatures 收紧为失败；
//   - 目录名与清单 id/version 必须一致（客户端解析期双向校验，提前在这里挡住）。

import { createHash } from 'node:crypto';
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const PACKAGE_FILE = 'package.zip';
const SIGNATURE_SUFFIX = '.sig';

const USAGE = `用法：node scripts/gen-plugin-index.mjs [选项]

扫描 <plugins>/<id>/<version>/ 目录树并生成市场 index JSON。

必填：
  --plugins <dir>             插件目录树根（内含 <id>/<version>/plugin.json 与 package.zip）
  --base-url <url>            安装包下载 URL 前缀（https://，或回环 http://127.0.0.1 / http://localhost，
                              或 file://）；dist.url = <base-url>/<id>/<version>/package.zip

产物：
  --output <path>             输出文件；省略时写 stdout

门禁：
  --require-signatures        任一版本缺少 package.zip.sig 时失败（官方源强制档）
  --help                      显示本帮助

示例：
  node scripts/gen-plugin-index.mjs \\
    --plugins plugins --base-url https://raw.githubusercontent.com/OpenALC/lite-code-marketplace/main/plugins \\
    --require-signatures --output index.json
`;

class UsageError extends Error {}

// ---- 校验（与 crates/lc-core/src/ext/marketplace/index.rs 的解析约束对齐） ----

/** 客户端 `manifest::valid_plugin_id` 同规则：非空、≤128、非 ./..、无空白/控制符/斜杠。 */
export function validPluginId(id) {
  return (
    typeof id === 'string' &&
    id.length > 0 &&
    id.length <= 128 &&
    id !== '.' &&
    id !== '..' &&
    ![...id].some(
      (ch) => /\s/.test(ch) || /[\u0000-\u001f\u007f]/.test(ch) || ch === '/' || ch === '\\',
    )
  );
}

const SEMVER_RE = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

export function validSemver(version) {
  return typeof version === 'string' && SEMVER_RE.test(version);
}

/** 客户端 `is_allowed_url` 同规则：https、回环 http、file。生成期先行 fail-closed。 */
export function allowedBaseUrl(url) {
  if (typeof url !== 'string' || url.length === 0 || /\s/.test(url)) return false;
  if (url.startsWith('https://') || url.startsWith('file://')) return true;
  if (url.startsWith('http://127.0.0.1') || url.startsWith('http://localhost')) return true;
  return false;
}

export function joinUrl(base, id, version) {
  const trimmed = base.replace(/\/+$/, '');
  return `${trimmed}/${id}/${version}/${PACKAGE_FILE}`;
}

/** 目录枚举：返回 [{ id, version }]（<id>/<version> 两级，忽略非目录）。 */
export function listVersions(pluginsDir) {
  const entries = [];
  for (const id of readdirSync(pluginsDir).sort()) {
    const idDir = join(pluginsDir, id);
    if (!statSync(idDir).isDirectory()) continue;
    for (const version of readdirSync(idDir).sort()) {
      const versionDir = join(idDir, version);
      if (statSync(versionDir).isDirectory()) entries.push({ id, version });
    }
  }
  return entries;
}

/** 单个 <id>/<version> 目录 → index 条目；非法形状抛 UsageError。 */
function buildEntry(pluginsDir, baseUrl, id, version, requireSignatures) {
  const dir = join(pluginsDir, id, version);
  if (!validPluginId(id)) {
    throw new UsageError(`插件目录名不是合法插件 id：\`${id}\`（${dir}）`);
  }
  if (!validSemver(version)) {
    throw new UsageError(`插件版本目录不是合法 semver：\`${version}\`（${dir}）`);
  }
  const manifestPath = join(dir, 'plugin.json');
  if (!existsSync(manifestPath)) {
    throw new UsageError(`缺少 plugin.json：${manifestPath}`);
  }
  let manifest;
  try {
    manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
  } catch (error) {
    throw new UsageError(`plugin.json 不是合法 JSON：${manifestPath}（${error.message}）`);
  }
  if (!manifest || typeof manifest !== 'object' || Array.isArray(manifest)) {
    throw new UsageError(`plugin.json 必须是对象：${manifestPath}`);
  }
  if (manifest.id !== id) {
    throw new UsageError(
      `plugin.json 的 id 与目录不一致：\`${manifest.id}\` ≠ \`${id}\`（${manifestPath}）`,
    );
  }
  if (manifest.version !== version) {
    throw new UsageError(
      `plugin.json 的 version 与目录不一致：\`${manifest.version}\` ≠ \`${version}\`（${manifestPath}）`,
    );
  }

  const packagePath = join(dir, PACKAGE_FILE);
  if (!existsSync(packagePath) || !statSync(packagePath).isFile()) {
    throw new UsageError(`缺少安装包 ${PACKAGE_FILE}：${packagePath}`);
  }
  const sha256 = createHash('sha256').update(readFileSync(packagePath)).digest('hex');

  const signaturePath = join(dir, `${PACKAGE_FILE}${SIGNATURE_SUFFIX}`);
  let signature = '';
  if (existsSync(signaturePath)) {
    signature = readFileSync(signaturePath, 'utf8').trim();
    if (signature.length === 0) {
      throw new UsageError(`签名文件为空：${signaturePath}`);
    }
  } else if (requireSignatures) {
    throw new UsageError(`--require-signatures：缺少签名文件 ${signaturePath}`);
  }

  const entry = {
    id,
    version,
    manifest,
    dist: { url: joinUrl(baseUrl, id, version), sha256 },
  };
  if (signature.length > 0) entry.signature = signature;
  if (manifest.engines && typeof manifest.engines === 'object' && !Array.isArray(manifest.engines)) {
    entry.engines = manifest.engines;
  }
  if (Array.isArray(manifest.extensionDependencies) && manifest.extensionDependencies.length > 0) {
    entry.deps = manifest.extensionDependencies;
  }
  return entry;
}

// ---- 组装 ----

/**
 * 构建 index（测试直接调用）：返回 { index, warnings }。
 * warnings 仅用于提示（未签名但未要求强制），不改变输出结构。
 */
export function buildIndex(options) {
  const { plugins, baseUrl, requireSignatures = false } = options;
  if (!plugins) throw new UsageError('缺少 --plugins <dir>');
  if (!baseUrl) throw new UsageError('缺少 --base-url <url>');
  if (!allowedBaseUrl(baseUrl)) {
    throw new UsageError(
      `--base-url 必须是 https://、file:// 或回环 http://127.0.0.1|localhost（当前：${baseUrl}）`,
    );
  }
  const root = resolve(plugins);
  if (!existsSync(root) || !statSync(root).isDirectory()) {
    throw new UsageError(`--plugins 目录不存在：${root}`);
  }

  const warnings = [];
  const entries = [];
  for (const { id, version } of listVersions(root)) {
    const entry = buildEntry(root, baseUrl, id, version, requireSignatures);
    if (entry.signature === undefined) {
      warnings.push(`未签名：${id}/${version}（signature=\"\"，客户端按未签名条目处理）`);
    }
    entries.push(entry);
  }
  if (entries.length === 0) {
    throw new UsageError(`没有可发布的插件版本：${root}`);
  }
  entries.sort((a, b) => (a.id === b.id ? a.version.localeCompare(b.version) : a.id.localeCompare(b.id)));
  return { index: { version: 1, plugins: entries }, warnings };
}

// ---- CLI ----

export function parseArgs(argv) {
  const options = {};
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    switch (arg) {
      case '--help':
        options.help = true;
        break;
      case '--plugins':
        options.plugins = argv[++i];
        break;
      case '--base-url':
        options.baseUrl = argv[++i];
        break;
      case '--output':
        options.output = argv[++i];
        break;
      case '--require-signatures':
        options.requireSignatures = true;
        break;
      default:
        throw new UsageError(`未知参数：${arg}`);
    }
  }
  return options;
}

export function run(argv, io = { stdout: process.stdout, stderr: process.stderr }) {
  const options = parseArgs(argv);
  if (options.help) {
    io.stdout.write(USAGE);
    return 0;
  }
  const { index, warnings } = buildIndex(options);
  for (const warning of warnings) io.stderr.write(`warning: ${warning}\n`);
  const json = `${JSON.stringify(index, null, 2)}\n`;
  if (options.output) {
    const target = resolve(options.output);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, json);
    io.stderr.write(`wrote ${target}（${index.plugins.length} 个版本）\n`);
  } else {
    io.stdout.write(json);
  }
  return 0;
}

const invokedDirectly =
  process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url;

if (invokedDirectly) {
  try {
    process.exitCode = run(process.argv.slice(2));
  } catch (error) {
    if (error instanceof UsageError) {
      process.stderr.write(`error: ${error.message}\n\n${USAGE}`);
    } else {
      process.stderr.write(`error: ${error?.stack ?? error}\n`);
    }
    process.exitCode = 1;
  }
}
