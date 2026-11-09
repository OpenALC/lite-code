#!/usr/bin/env node
// Build the Lite Code HarmonyOS shell.
//
// Steps (docs/plans/ohos-shell.md §2.3):
//   1. validate DEVECO_SDK_HOME, project hvigorw and ohpm
//   2. pnpm --filter @lite-code/web build:ohos
//   3. assert dist-ohos/ohos.html exists and contains no Monaco chunk
//   4. replace rawfile/web with dist-ohos
//   5. ohpm install
//   6. hvigorw assembleHap
//   7. print the HAP path and hdc install/launch commands
//
// Usage: node scripts/ohos/build.mjs [--skip-web]
//   --skip-web  reuse the existing rawfile/web (and the committed ohos.html
//               placeholder when no web build exists)

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(SCRIPT_DIR, '..', '..');
const OHOS_ROOT = path.join(REPO_ROOT, 'apps', 'ohos');
const WEB_ROOT = path.join(REPO_ROOT, 'apps', 'web');
const WEB_DIST = path.join(WEB_ROOT, 'dist-ohos');
const RAW_DIR = path.join(OHOS_ROOT, 'entry', 'src', 'main', 'resources', 'rawfile');
const RAW_WEB = path.join(RAW_DIR, 'web');
const RAW_PLACEHOLDER = path.join(RAW_DIR, 'ohos.html');
const HVIGOR_CONFIG = path.join(OHOS_ROOT, 'hvigor', 'hvigor-config.json5');
const APP_PROFILE = path.join(OHOS_ROOT, 'build-profile.json5');
const HVIGORW = path.join(OHOS_ROOT, process.platform === 'win32' ? 'hvigorw.bat' : 'hvigorw');
const HAP_RELATIVE = path.join('entry', 'build', 'default', 'outputs', 'default', 'entry-default-unsigned.hap');
const EXPECTED_MODEL_VERSION = '5.0.0';
const BUNDLE_NAME = 'dev.litecode.ohos';
const IS_WINDOWS = process.platform === 'win32';

function fail(message) {
  throw new Error(message);
}

function log(message) {
  console.log(message);
}

function formatBytes(bytes) {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)} KiB`;
  }
  return `${(bytes / (1024 * 1024)).toFixed(2)} MiB`;
}

function quoteWin(arg) {
  if (!/[\s"]/u.test(arg)) {
    return arg;
  }
  return `"${arg.replaceAll('"', '\\"')}"`;
}

function run(command, args, options = {}) {
  const spawnOptions = { stdio: 'inherit', ...options };
  const result = IS_WINDOWS
    ? spawnSync(
        process.env.ComSpec || 'cmd.exe',
        ['/d', '/s', '/c', `"${[command, ...args].map(quoteWin).join(' ')}"`],
        { ...spawnOptions, windowsVerbatimArguments: true }
      )
    : spawnSync(command, args, spawnOptions);

  if (result.error) {
    throw new Error(`failed to start ${command}: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(`${command} exited with code ${result.status}`);
  }
}

function walkFiles(root, skipDirs = new Set()) {
  const result = [];
  if (!fs.existsSync(root)) {
    return result;
  }
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    if (entry.isDirectory() && skipDirs.has(entry.name)) {
      continue;
    }
    const fullPath = path.join(root, entry.name);
    if (entry.isDirectory()) {
      result.push(...walkFiles(fullPath, skipDirs));
    } else if (entry.isFile()) {
      result.push(fullPath);
    }
  }
  return result;
}

function resolveSdkHome(rawValue) {
  if (!rawValue) {
    fail(
      'DEVECO_SDK_HOME is not set. Example (Git Bash):\n' +
        "  export DEVECO_SDK_HOME='D:\\ide\\Huawei\\DevEco Studio\\sdk'"
    );
  }

  let sdkHome = path.resolve(rawValue);
  if (!fs.existsSync(sdkHome)) {
    fail(`DEVECO_SDK_HOME does not exist: ${sdkHome}`);
  }
  // Accept both <DevEco>/sdk and <DevEco>/sdk/default.
  if (path.basename(sdkHome).toLowerCase() === 'default') {
    sdkHome = path.dirname(sdkHome);
  }
  // Accept the DevEco installation directory as a convenience.
  if (fs.existsSync(path.join(sdkHome, 'sdk', 'default'))) {
    sdkHome = path.join(sdkHome, 'sdk');
  }
  if (!fs.existsSync(path.join(sdkHome, 'default'))) {
    fail(
      `DEVECO_SDK_HOME does not look like a HarmonyOS SDK (missing "default"): ${sdkHome}\n` +
        'Point it at <DevEco Studio>/sdk.'
    );
  }
  return sdkHome;
}

function findOhpm(devecoHome) {
  const binary = IS_WINDOWS ? 'ohpm.bat' : 'ohpm';
  const candidates = [];
  if (process.env.OHPM_HOME) {
    candidates.push(path.join(process.env.OHPM_HOME, 'bin', binary));
    candidates.push(path.join(process.env.OHPM_HOME, binary));
  }
  candidates.push(path.join(devecoHome, 'tools', 'ohpm', 'bin', binary));

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }
  fail(
    `ohpm was not found. Expected ${path.join(devecoHome, 'tools', 'ohpm', 'bin', binary)}.\n` +
      'Set OHPM_HOME or reinstall DevEco Studio.'
  );
}

function findHdc(sdkHome) {
  const binary = IS_WINDOWS ? 'hdc.exe' : 'hdc';
  const candidates = [
    path.join(sdkHome, 'default', 'openharmony', 'toolchains', binary),
    path.join(sdkHome, 'default', 'hms', 'toolchains', binary),
  ];
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return binary;
}

function assertHvigorConfig() {
  if (!fs.existsSync(HVIGOR_CONFIG)) {
    fail(`missing hvigor config: ${HVIGOR_CONFIG}`);
  }
  const text = fs.readFileSync(HVIGOR_CONFIG, 'utf8');
  const match = text.match(/"modelVersion"\s*:\s*"([^"]+)"/u);
  if (!match) {
    fail(`hvigor-config.json5 must pin "modelVersion" (expected ${EXPECTED_MODEL_VERSION})`);
  }
  if (match[1] !== EXPECTED_MODEL_VERSION) {
    fail(
      `hvigor modelVersion is ${match[1]}, expected ${EXPECTED_MODEL_VERSION}. ` +
        'Keep the wrapper version pinned to the DevEco Studio toolchain.'
    );
  }
  log(`  hvigor modelVersion: ${match[1]}`);
}

function assertNoSigningMaterial() {
  const profile = fs.readFileSync(APP_PROFILE, 'utf8');
  if (!/"signingConfigs"\s*:\s*\[\s*\]/u.test(profile)) {
    fail('apps/ohos/build-profile.json5 must keep "signingConfigs": [] (unsigned builds only).');
  }
  const signingField = /(storePassword|keyPassword|storeFile|profileFile|certpath|keyAlias)\s*:/u;
  if (signingField.test(profile)) {
    fail('apps/ohos/build-profile.json5 contains signing fields; never commit signing material.');
  }

  const signingFile = /\.(p12|p7b|cer|csr|key|pem)$/iu;
  const offenders = walkFiles(
    OHOS_ROOT,
    new Set(['build', 'oh_modules', '.hvigor', '.cxx', 'node_modules'])
  ).filter((file) => signingFile.test(file));
  if (offenders.length > 0) {
    fail(`signing material found in apps/ohos:\n  ${offenders.join('\n  ')}`);
  }
}

function assertWebBuild() {
  const entryHtml = path.join(WEB_DIST, 'ohos.html');
  if (!fs.existsSync(entryHtml)) {
    fail(
      `missing ${path.relative(REPO_ROOT, entryHtml)}. ` +
        'Run without --skip-web (or build apps/web with the ohos mode first).'
    );
  }

  const files = walkFiles(WEB_DIST);
  const monacoChunks = [];
  let totalBytes = 0;
  for (const file of files) {
    totalBytes += fs.statSync(file).size;
    if (!file.endsWith('.js')) {
      continue;
    }
    const relative = path.relative(WEB_DIST, file);
    if (/monaco/iu.test(relative)) {
      monacoChunks.push(relative);
      continue;
    }
    if (fs.readFileSync(file, 'utf8').includes('monaco-editor')) {
      monacoChunks.push(relative);
    }
  }
  if (monacoChunks.length > 0) {
    fail(`dist-ohos contains Monaco output: ${monacoChunks.join(', ')}`);
  }
  log(`  dist-ohos: ${files.length} files, ${formatBytes(totalBytes)}`);
}

function syncRawfile() {
  fs.rmSync(RAW_WEB, { recursive: true, force: true });
  fs.mkdirSync(RAW_WEB, { recursive: true });
  fs.cpSync(WEB_DIST, RAW_WEB, { recursive: true });
  const copied = path.join(RAW_WEB, 'ohos.html');
  if (!fs.existsSync(copied)) {
    fail(`failed to copy ohos.html into ${path.relative(REPO_ROOT, RAW_WEB)}`);
  }
  const files = walkFiles(RAW_WEB);
  const total = files.reduce((sum, file) => sum + fs.statSync(file).size, 0);
  log(`  rawfile/web: ${files.length} files, ${formatBytes(total)}`);
}

function checkExistingAssets() {
  if (fs.existsSync(path.join(RAW_WEB, 'ohos.html'))) {
    log('  --skip-web: reusing existing rawfile/web');
    return;
  }
  if (!fs.existsSync(RAW_PLACEHOLDER)) {
    fail('--skip-web was requested but neither rawfile/web/ohos.html nor rawfile/ohos.html exists.');
  }
  log('  --skip-web: using committed placeholder rawfile/ohos.html');
}

function locateHap() {
  const expected = path.join(OHOS_ROOT, HAP_RELATIVE);
  if (fs.existsSync(expected)) {
    return expected;
  }
  const haps = walkFiles(path.join(OHOS_ROOT, 'entry', 'build')).filter((file) =>
    file.endsWith('.hap')
  );
  if (haps.length === 0) {
    fail('hvigorw assembleHap succeeded but no .hap file was produced.');
  }
  return haps[0];
}

function printUsage() {
  log('Usage: node scripts/ohos/build.mjs [--skip-web]');
  log('');
  log('  --skip-web   skip the web build and rawfile sync, reusing current assets');
}

function main() {
  const args = process.argv.slice(2);
  if (args.includes('--help') || args.includes('-h')) {
    printUsage();
    return;
  }
  const unknown = args.filter((arg) => arg !== '--skip-web');
  if (unknown.length > 0) {
    fail(`unknown argument(s): ${unknown.join(' ')}\n\nRun with --help for usage.`);
  }
  const skipWeb = args.includes('--skip-web');

  log('Lite Code · HarmonyOS shell build');
  log('');
  log('[1/7] validate environment');

  const sdkHome = resolveSdkHome(process.env.DEVECO_SDK_HOME);
  const devecoHome = path.dirname(sdkHome);
  const ohpm = findOhpm(devecoHome);
  const hdc = findHdc(sdkHome);
  const hvigorUserHome =
    process.env.HVIGOR_USER_HOME || path.join(os.tmpdir(), 'lc-hvigor');
  fs.mkdirSync(hvigorUserHome, { recursive: true });

  if (!fs.existsSync(HVIGORW)) {
    fail(`missing hvigor wrapper: ${HVIGORW}`);
  }
  for (const required of ['oh-package.json5', 'hvigorfile.ts', 'entry/build-profile.json5']) {
    if (!fs.existsSync(path.join(OHOS_ROOT, required))) {
      fail(`missing apps/ohos/${required}`);
    }
  }

  log(`  DEVECO_SDK_HOME: ${sdkHome}`);
  log(`  HVIGOR_USER_HOME: ${hvigorUserHome}`);
  log(`  ohpm: ${ohpm}`);
  assertHvigorConfig();
  assertNoSigningMaterial();

  const childEnv = {
    ...process.env,
    DEVECO_SDK_HOME: sdkHome,
    HVIGOR_USER_HOME: hvigorUserHome,
  };

  if (skipWeb) {
    log('[2/7] skip web build');
    log('[3/7] skip dist-ohos validation');
    log('[4/7] skip rawfile/web sync');
    checkExistingAssets();
  } else {
    const webPackage = JSON.parse(fs.readFileSync(path.join(WEB_ROOT, 'package.json'), 'utf8'));
    if (!webPackage.scripts || !webPackage.scripts['build:ohos']) {
      fail(
        'apps/web/package.json does not define "build:ohos". ' +
          'The ohos web entry must land before a full build; use --skip-web for the shell slice.'
      );
    }
    log('[2/7] pnpm --filter @lite-code/web build:ohos');
    run('pnpm', ['--filter', '@lite-code/web', 'build:ohos'], {
      cwd: REPO_ROOT,
      env: childEnv,
    });

    log('[3/7] validate dist-ohos');
    assertWebBuild();

    log('[4/7] sync rawfile/web');
    syncRawfile();
  }

  log('[5/7] ohpm install');
  run(ohpm, ['install'], { cwd: OHOS_ROOT, env: childEnv });

  log('[6/7] hvigorw assembleHap');
  run(
    HVIGORW,
    [
      'assembleHap',
      '--mode',
      'module',
      '-p',
      'product=default',
      '-p',
      'module=entry@default',
      '-p',
      'buildMode=debug',
      '--no-daemon',
    ],
    { cwd: OHOS_ROOT, env: childEnv }
  );

  log('[7/7] locate HAP');
  const hap = locateHap();
  const hapSize = fs.statSync(hap).size;
  log('');
  log(`HAP: ${hap}`);
  log(`Size: ${formatBytes(hapSize)}`);
  log('');
  log('Install / launch (unsigned HAP works on the emulator only):');
  log(`  "${hdc}" install -r "${hap}"`);
  log(`  "${hdc}" shell aa start -a EntryAbility -b ${BUNDLE_NAME}`);
}

try {
  main();
} catch (error) {
  console.error('');
  console.error(`ERROR: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
}
