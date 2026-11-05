#!/usr/bin/env node
// 生成更新清单 latest.json（P6 A 类第 6 项，docs/UPDATER.md）。
//
// 无第三方依赖，CI 离线可跑：
//   node scripts/gen-update-manifest.mjs --help
//
// 设计约束：
//   - 只描述产物，不下载、不安装、不执行任何代码；
//   - 没有 .sig 时 signed=false、signature=""，不伪造签名；
//   - previous 保留上一版清单的版本与 URL，供人工回滚（不自动降级）。

import { createHash } from 'node:crypto';
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export const PLATFORMS = ['windows-x86_64', 'linux-x86_64', 'linux-aarch64'];

const USAGE = `用法：node scripts/gen-update-manifest.mjs [选项]

生成 latest.json。至少提供 --artifacts <目录> 或 --asset <平台=路径|URL>。

必填：
  --version <semver>          版本号，如 0.6.1-preview.1
  --repo <owner/name>         产物所在 GitHub 仓库（或改用 --base-url）

产物：
  --artifacts <dir>           递归扫描安装包目录（可重复）
  --asset <平台=路径|URL>     显式指定某平台产物（可重复，优先于目录扫描）
  --sha256 <平台=hex>         --asset 为 URL 时必填（64 位十六进制）
  --sig <平台=路径>           显式指定签名文件；默认读取安装包旁的 <文件>.sig
  --base-url <url>            下载 URL 前缀；默认 GitHub Release 资产地址

元数据：
  --tag <vX.Y.Z>              默认 v<version>，必须与版本一致
  --notes <text>              版本说明（与 --notes-file 二选一）
  --notes-file <path>         从文件读取版本说明
  --pub-date <ISO8601>        默认当前时间
  --previous-manifest <path>  读取上一版 latest.json，写入 previous（回滚通道）
  --output <path>             输出文件；省略时写 stdout
  --require-signatures        任一平台缺少 .sig 时失败（稳定版门禁用）
  --help                      显示本帮助

平台键：${PLATFORMS.join(' / ')}

示例：
  node scripts/gen-update-manifest.mjs \\
    --version 0.6.1-preview.1 --repo OpenALC/lite-code \\
    --artifacts artifacts --notes-file docs/releases/0.6.1-preview.1.md \\
    --previous-manifest old-latest.json --output latest.json
`;

class UsageError extends Error {}

function fail(message) {
  throw new UsageError(message);
}

function parsePlatformValue(raw, flag) {
  const index = raw.indexOf('=');
  if (index <= 0) fail(`${flag} 需要 <平台=值> 形式，收到：${raw}`);
  const platform = raw.slice(0, index);
  const value = raw.slice(index + 1);
  if (!PLATFORMS.includes(platform)) {
    fail(`${flag} 的平台 ${platform} 不支持（可用：${PLATFORMS.join(', ')}）`);
  }
  if (!value) fail(`${flag} 的平台 ${platform} 缺少值`);
  return { platform, value };
}

export function parseArgs(argv) {
  const options = {
    version: null,
    tag: null,
    repo: null,
    baseUrl: null,
    artifacts: [],
    assets: [],
    sigs: [],
    hashes: [],
    notes: null,
    notesFile: null,
    pubDate: null,
    previousManifest: null,
    output: null,
    requireSignatures: false,
    help: false,
  };

  for (let i = 0; i < argv.length; i += 1) {
    let flag = argv[i];
    if (!flag.startsWith('--')) fail(`未知参数：${flag}`);
    let inline = null;
    const eq = flag.indexOf('=');
    if (eq >= 0) {
      inline = flag.slice(eq + 1);
      flag = flag.slice(0, eq);
    }
    const take = () => {
      if (inline !== null) return inline;
      const next = argv[i + 1];
      if (next === undefined) fail(`缺少 ${flag} 的值`);
      i += 1;
      return next;
    };

    switch (flag) {
      case '--version':
        options.version = take();
        break;
      case '--tag':
        options.tag = take();
        break;
      case '--repo':
        options.repo = take();
        break;
      case '--base-url':
        options.baseUrl = take();
        break;
      case '--artifacts':
        options.artifacts.push(take());
        break;
      case '--asset':
        options.assets.push(parsePlatformValue(take(), '--asset'));
        break;
      case '--sig':
        options.sigs.push(parsePlatformValue(take(), '--sig'));
        break;
      case '--sha256':
        options.hashes.push(parsePlatformValue(take(), '--sha256'));
        break;
      case '--notes':
        options.notes = take();
        break;
      case '--notes-file':
        options.notesFile = take();
        break;
      case '--pub-date':
        options.pubDate = take();
        break;
      case '--previous-manifest':
        options.previousManifest = take();
        break;
      case '--output':
        options.output = take();
        break;
      case '--require-signatures':
        options.requireSignatures = true;
        break;
      case '--help':
        options.help = true;
        break;
      default:
        fail(`未知参数：${flag}`);
    }
  }

  return options;
}

function validateVersion(version) {
  const match = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(
    version,
  );
  if (!match) fail(`--version '${version}' 不是合法 semver`);
  const major = match[1];
  const prerelease = match[4] ?? '';
  for (const part of prerelease.split('.')) {
    if (part.length > 1 && part.startsWith('0') && /^\d+$/.test(part)) {
      fail(`预发布标识不能有前导零（收到 '${version}'）`);
    }
  }
  if (major === '0' && !/^preview\.\d+$/.test(prerelease)) {
    fail(`0.x 版本必须带 -preview.N（收到 '${version}'，规则见 docs/VERSIONING.md）`);
  }
  if (major !== '0' && prerelease) {
    fail(`正式版不得带预发布后缀（收到 '${version}'）`);
  }
}

function isHttpsOrLoopback(url) {
  try {
    const parsed = new URL(url);
    if (parsed.protocol === 'https:') return true;
    return (
      parsed.protocol === 'http:' &&
      ['localhost', '127.0.0.1', '[::1]'].includes(parsed.hostname)
    );
  } catch {
    return false;
  }
}

function defaultBaseUrl(repo, tag) {
  if (!repo) fail('缺少 --repo（owner/name）或 --base-url');
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo)) {
    fail(`--repo '${repo}' 必须是 owner/name 形式`);
  }
  return `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}`;
}

function sha256File(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

function readSignature(path) {
  if (!existsSync(path)) fail(`签名文件不存在：${path}`);
  const signature = readFileSync(path, 'utf8').trim();
  if (!signature) fail(`签名文件为空：${path}`);
  return signature;
}

function walk(dir, files) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) walk(full, files);
    else if (entry.isFile()) files.push(full);
  }
}

/**
 * 按文件名归类 Tauri 产物：
 *   NSIS   Lite Code_0.6.1-preview.1_x64-setup.exe   → windows-x86_64
 *   AppImage/deb  *_amd64.AppImage / *_amd64.deb     → linux-x86_64
 *                 *_aarch64.AppImage / *_arm64.deb   → linux-aarch64
 * 返回 null 表示无法归类（调用方决定警告或忽略）。
 */
export function classifyArtifact(name) {
  const lower = name.toLowerCase();
  if (lower.endsWith('.exe') && lower.includes('-setup')) {
    if (/(^|[_.-])(x64|x86_64|amd64)([_.-]|$)/.test(lower)) return 'windows-x86_64';
    return null;
  }
  if (lower.endsWith('.appimage') || lower.endsWith('.deb')) {
    if (/(^|[_.-])(aarch64|arm64)([_.-]|$)/.test(lower)) return 'linux-aarch64';
    if (/(^|[_.-])(x86_64|amd64|x64)([_.-]|$)/.test(lower)) return 'linux-x86_64';
    return null;
  }
  return null;
}

function collectFromDir(dir, sources, explicit, warnings) {
  if (!dir) fail('--artifacts 缺少目录');
  const root = resolve(dir);
  if (!existsSync(root) || !statSync(root).isDirectory()) {
    fail(`--artifacts 目录不存在：${dir}`);
  }
  const files = [];
  walk(root, files);
  for (const file of files) {
    const name = basename(file);
    if (name.endsWith('.sig')) continue;
    const platform = classifyArtifact(name);
    if (!platform) {
      if (/\.(exe|msi|appimage|deb)$/i.test(name)) {
        warnings.push(`跳过无法归类架构的产物：${name}`);
      }
      continue;
    }
    if (sources.has(platform)) {
      const current = sources.get(platform);
      // Linux 同时有 AppImage 与 deb 时优先 AppImage（Tauri 自更新路径）。
      if (
        !explicit.has(platform) &&
        name.toLowerCase().endsWith('.appimage') &&
        !current.toLowerCase().endsWith('.appimage')
      ) {
        warnings.push(
          `平台 ${platform} 有多个产物，使用 ${name}（忽略 ${basename(current)}）`,
        );
        sources.set(platform, file);
      } else {
        warnings.push(`平台 ${platform} 已有产物，忽略 ${name}`);
      }
      continue;
    }
    sources.set(platform, file);
  }
}

function readPrevious(path, version) {
  let previous;
  try {
    previous = JSON.parse(readFileSync(path, 'utf8'));
  } catch (error) {
    fail(`读取 --previous-manifest 失败：${error.message}`);
  }
  if (typeof previous?.version !== 'string' || !previous.version) {
    fail('--previous-manifest 缺少 version');
  }
  if (previous.version === version) {
    fail('previous 版本不能与当前版本相同');
  }
  if (!previous.platforms || typeof previous.platforms !== 'object' || Array.isArray(previous.platforms)) {
    fail('--previous-manifest 缺少 platforms');
  }
  const platforms = {};
  for (const [platform, asset] of Object.entries(previous.platforms)) {
    if (!asset || typeof asset.url !== 'string' || !asset.url) {
      fail(`--previous-manifest 的平台 ${platform} 缺少 url`);
    }
    platforms[platform] = {
      url: asset.url,
      signature: typeof asset.signature === 'string' ? asset.signature : '',
      sha256: typeof asset.sha256 === 'string' ? asset.sha256 : '',
    };
  }
  if (Object.keys(platforms).length === 0) {
    fail('--previous-manifest 没有可用平台');
  }
  return {
    version: previous.version,
    notes: typeof previous.notes === 'string' ? previous.notes : '',
    platforms,
  };
}

export function buildManifest(options) {
  if (typeof options.version !== 'string' || !options.version) {
    fail('缺少 --version');
  }
  validateVersion(options.version);

  const tag = options.tag ?? `v${options.version}`;
  if (tag !== `v${options.version}`) {
    fail(`--tag '${tag}' 必须等于 v${options.version}`);
  }
  const baseUrl = (options.baseUrl ?? defaultBaseUrl(options.repo, tag)).replace(/\/+$/, '');
  if (!isHttpsOrLoopback(baseUrl)) {
    fail(`--base-url 必须是 https（仅回环地址允许 http）：${baseUrl}`);
  }

  const warnings = [];
  const sources = new Map();
  const explicit = new Set();
  for (const { platform, value } of options.assets ?? []) {
    if (sources.has(platform)) fail(`平台 ${platform} 重复指定`);
    sources.set(platform, value);
    explicit.add(platform);
  }
  for (const dir of options.artifacts ?? []) {
    collectFromDir(dir, sources, explicit, warnings);
  }
  if (sources.size === 0) {
    fail('没有找到安装包：用 --artifacts <目录> 或 --asset <平台=路径>');
  }

  const signatures = new Map();
  for (const { platform, value } of options.sigs ?? []) {
    if (signatures.has(platform)) fail(`平台 ${platform} 重复指定 --sig`);
    if (!sources.has(platform)) fail(`--sig 的平台 ${platform} 没有对应安装包`);
    signatures.set(platform, value);
  }
  const hashes = new Map();
  for (const { platform, value } of options.hashes ?? []) {
    if (hashes.has(platform)) fail(`平台 ${platform} 重复指定 --sha256`);
    hashes.set(platform, value);
  }

  const platforms = {};
  let allSigned = true;
  for (const platform of PLATFORMS) {
    if (!sources.has(platform)) continue;
    const source = sources.get(platform);
    const remote = /^https?:\/\//i.test(source);
    let url;
    let sha256;
    let signature = null;

    if (remote) {
      if (!isHttpsOrLoopback(source)) {
        fail(`平台 ${platform} 的 URL 必须是 https（仅回环地址允许 http）：${source}`);
      }
      url = source;
      sha256 = hashes.get(platform);
      if (!sha256) fail(`URL 形式的 ${platform} 需要 --sha256 ${platform}=<64位hex>`);
    } else {
      const path = resolve(source);
      if (!existsSync(path) || !statSync(path).isFile()) {
        fail(`平台 ${platform} 的安装包不存在：${source}`);
      }
      sha256 = sha256File(path);
      // GitHub 会把上传资产文件名中的空格替换成点（release.yml 的
      // SHA256SUMS 也做同样处理），URL 必须按上传后的名字生成。
      const assetName = basename(path).replaceAll(' ', '.');
      url = `${baseUrl}/${encodeURIComponent(assetName)}`;
    }

    if (!/^[0-9a-fA-F]{64}$/.test(sha256)) {
      fail(`平台 ${platform} 的 sha256 不是 64 位十六进制：${sha256}`);
    }

    const signaturePath = signatures.get(platform) ?? (!remote ? `${resolve(source)}.sig` : null);
    if (signaturePath && existsSync(signaturePath)) {
      signature = readSignature(signaturePath);
    } else if (signatures.has(platform)) {
      fail(`签名文件不存在：${signaturePath}`);
    }
    if (!signature) allSigned = false;
    platforms[platform] = {
      url,
      signature: signature ?? '',
      sha256,
    };
  }

  if (options.requireSignatures && !allSigned) {
    fail('--require-signatures 要求所有平台都有 .sig，但至少一个平台缺失');
  }

  if (options.notesFile && options.notes !== null && options.notes !== undefined) {
    fail('--notes 与 --notes-file 只能选一个');
  }
  let notes = options.notes ?? '';
  if (options.notesFile) {
    try {
      notes = readFileSync(options.notesFile, 'utf8');
    } catch (error) {
      fail(`读取 --notes-file 失败：${error.message}`);
    }
  }
  const pubDate = options.pubDate ?? new Date().toISOString();
  if (Number.isNaN(Date.parse(pubDate))) {
    fail(`--pub-date 不是合法时间：${pubDate}`);
  }

  const manifest = {
    version: options.version,
    notes,
    pub_date: pubDate,
    signed: allSigned,
    platforms,
  };
  if (options.previousManifest) {
    manifest.previous = readPrevious(options.previousManifest, options.version);
  }
  return { manifest, warnings };
}

export function run(argv, io = { stdout: process.stdout, stderr: process.stderr }) {
  const options = parseArgs(argv);
  if (options.help) {
    io.stdout.write(USAGE);
    return 0;
  }
  const { manifest, warnings } = buildManifest(options);
  for (const warning of warnings) io.stderr.write(`warning: ${warning}\n`);
  const json = `${JSON.stringify(manifest, null, 2)}\n`;
  if (options.output) {
    const target = resolve(options.output);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, json);
    io.stderr.write(`wrote ${target}\n`);
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
