// node --test scripts/gen-update-manifest.test.mjs
// 覆盖：无签名 / 有签名 / 混合签名 / 回滚 previous / URL 资产与参数校验。

import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import {
  buildManifest,
  classifyArtifact,
  parseArgs,
} from './gen-update-manifest.mjs';

const VERSION = '0.6.1-preview.1';
const TAG = `v${VERSION}`;

function tempDir() {
  return mkdtempSync(join(tmpdir(), 'lc-updater-manifest-'));
}

function writeArtifact(dir, name, content = 'installer-binary') {
  const path = join(dir, name);
  writeFileSync(path, content);
  return path;
}

function baseOptions(dir) {
  return {
    version: VERSION,
    repo: 'OpenALC/lite-code',
    artifacts: [dir],
    pubDate: '2026-11-05T00:00:00.000Z',
  };
}

test('无签名产物生成 signed=false 与空 signature', () => {
  const dir = tempDir();
  const win = writeArtifact(dir, 'Lite Code_0.6.1-preview.1_x64-setup.exe', 'win');
  writeArtifact(dir, 'Lite Code_0.6.1-preview.1_amd64.AppImage', 'app');
  writeArtifact(dir, 'lite-code_0.6.1-preview.1_amd64.deb', 'deb');

  const { manifest } = buildManifest(baseOptions(dir));
  assert.equal(manifest.version, VERSION);
  assert.equal(manifest.signed, false);
  assert.equal(manifest.pub_date, '2026-11-05T00:00:00.000Z');

  assert.deepEqual(Object.keys(manifest.platforms), [
    'windows-x86_64',
    'linux-x86_64',
  ]);
  assert.equal(manifest.platforms['windows-x86_64'].signature, '');
  assert.equal(
    manifest.platforms['windows-x86_64'].sha256,
    createHash('sha256').update('win').digest('hex'),
  );
  assert.match(
    manifest.platforms['windows-x86_64'].url,
    /^https:\/\/github\.com\/OpenALC\/lite-code\/releases\/download\/v0\.6\.1-preview\.1\/Lite\.Code_0\.6\.1-preview\.1_x64-setup\.exe$/,
  );
  assert.match(manifest.platforms['linux-x86_64'].url, /\.AppImage$/);
  assert.ok(win);
});

test('全部 .sig 存在时 signed=true 并写入签名内容', () => {
  const dir = tempDir();
  const win = writeArtifact(dir, 'Lite Code_0.6.1-preview.1_x64-setup.exe');
  const app = writeArtifact(dir, 'Lite Code_0.6.1-preview.1_amd64.AppImage');
  const deb = writeArtifact(dir, 'lite-code_0.6.1-preview.1_amd64.deb');
  writeFileSync(`${win}.sig`, 'trusted-comment\nwindows-signature\n');
  writeFileSync(`${app}.sig`, 'app-signature\n');
  writeFileSync(`${deb}.sig`, 'deb-signature\n');

  const { manifest } = buildManifest({ ...baseOptions(dir), requireSignatures: true });
  assert.equal(manifest.signed, true);
  assert.equal(
    manifest.platforms['windows-x86_64'].signature,
    'trusted-comment\nwindows-signature',
  );
  assert.equal(manifest.platforms['linux-x86_64'].signature, 'app-signature');
});

test('混合签名 signed=false，--require-signatures 拒绝', () => {
  const dir = tempDir();
  const win = writeArtifact(dir, 'Lite Code_0.6.1-preview.1_x64-setup.exe');
  writeArtifact(dir, 'Lite Code_0.6.1-preview.1_amd64.AppImage');
  writeFileSync(`${win}.sig`, 'windows-signature');

  const { manifest } = buildManifest(baseOptions(dir));
  assert.equal(manifest.signed, false);
  assert.equal(
    manifest.platforms['windows-x86_64'].signature,
    'windows-signature',
  );
  assert.equal(manifest.platforms['linux-x86_64'].signature, '');

  assert.throws(
    () => buildManifest({ ...baseOptions(dir), requireSignatures: true }),
    /--require-signatures/,
  );
});

test('previous 保留上一版版本与 URL，供人工回滚', () => {
  const dir = tempDir();
  writeArtifact(dir, 'Lite Code_0.6.1-preview.1_x64-setup.exe');
  const previousPath = join(dir, 'old-latest.json');
  writeFileSync(
    previousPath,
    JSON.stringify({
      version: '0.6.0-preview.1',
      notes: 'previous release',
      platforms: {
        'windows-x86_64': {
          url: 'https://github.com/OpenALC/lite-code/releases/download/v0.6.0-preview.1/Lite.Code_0.6.0-preview.1_x64-setup.exe',
          signature: 'old-signature',
          sha256: 'b'.repeat(64),
        },
      },
    }),
  );

  const { manifest } = buildManifest({
    ...baseOptions(dir),
    previousManifest: previousPath,
  });
  assert.equal(manifest.previous.version, '0.6.0-preview.1');
  assert.equal(manifest.previous.notes, 'previous release');
  assert.match(manifest.previous.platforms['windows-x86_64'].url, /v0\.6\.0-preview\.1/);
  assert.equal(manifest.previous.platforms['windows-x86_64'].signature, 'old-signature');
});

test('URL 资产必须显式提供 sha256 与签名文件', () => {
  const options = {
    version: VERSION,
    repo: 'OpenALC/lite-code',
    pubDate: '2026-11-05T00:00:00.000Z',
    assets: [
      {
        platform: 'windows-x86_64',
        value: 'https://example.com/Lite.Code_0.6.1-preview.1_x64-setup.exe',
      },
    ],
    hashes: [{ platform: 'windows-x86_64', value: 'c'.repeat(64) }],
  };
  const { manifest } = buildManifest(options);
  assert.equal(manifest.platforms['windows-x86_64'].sha256, 'c'.repeat(64));
  assert.equal(manifest.platforms['windows-x86_64'].signature, '');
  assert.equal(manifest.signed, false);

  assert.throws(
    () =>
      buildManifest({
        ...options,
        hashes: [],
      }),
    /需要 --sha256/,
  );
  assert.throws(
    () =>
      buildManifest({
        ...options,
        hashes: [{ platform: 'windows-x86_64', value: 'not-a-hash' }],
      }),
    /64 位十六进制/,
  );
});

test('产物归类与参数解析', () => {
  assert.equal(
    classifyArtifact('Lite Code_0.6.1-preview.1_x64-setup.exe'),
    'windows-x86_64',
  );
  assert.equal(
    classifyArtifact('Lite Code_0.6.1-preview.1_aarch64.AppImage'),
    'linux-aarch64',
  );
  assert.equal(classifyArtifact('lite-code_0.6.1-preview.1_arm64.deb'), 'linux-aarch64');
  assert.equal(classifyArtifact('lite-code_0.6.1-preview.1_amd64.deb'), 'linux-x86_64');
  assert.equal(classifyArtifact('Lite Code_0.6.1-preview.1_x64.msi'), null);
  assert.equal(classifyArtifact('random.exe'), null);

  const parsed = parseArgs([
    '--version',
    VERSION,
    '--repo=OpenALC/lite-code',
    '--asset',
    'linux-aarch64=/tmp/a.AppImage',
    '--require-signatures',
  ]);
  assert.equal(parsed.version, VERSION);
  assert.equal(parsed.repo, 'OpenALC/lite-code');
  assert.deepEqual(parsed.assets, [
    { platform: 'linux-aarch64', value: '/tmp/a.AppImage' },
  ]);
  assert.equal(parsed.requireSignatures, true);

  assert.throws(
    () => parseArgs(['--asset', 'darwin-aarch64=/tmp/a.dmg']),
    /不支持/,
  );
  assert.throws(() => buildManifest({ version: '1.0.0' }), /非法 semver|--repo/);
  assert.throws(() => buildManifest({ version: '0.6.1' }), /preview/);
  assert.throws(
    () => buildManifest({ version: '0.6.1-preview.01', repo: 'OpenALC/lite-code' }),
    /前导零/,
  );

  // 目录不存在必须报错，避免 CI 静默产出空清单。
  const dir = tempDir();
  mkdirSync(join(dir, 'nested'));
  assert.throws(
    () => buildManifest({ ...baseOptions(join(dir, 'missing')) }),
    /目录不存在/,
  );
});
