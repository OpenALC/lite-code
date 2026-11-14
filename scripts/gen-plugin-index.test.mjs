// node --test scripts/gen-plugin-index.test.mjs
// 覆盖：基本生成（sha256/URL/排序/签名透传）、--require-signatures、
// id/version/manifest 一致性、缺包、URL 白名单、幂等与 CLI 输出。

import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import {
  allowedBaseUrl,
  buildIndex,
  joinUrl,
  listVersions,
  parseArgs,
  validPluginId,
  validSemver,
} from './gen-plugin-index.mjs';

const BASE = 'https://raw.githubusercontent.com/OpenALC/lite-code-marketplace/main/plugins';

function tempDir() {
  return mkdtempSync(join(tmpdir(), 'lc-plugin-index-'));
}

function writeVersion(root, id, version, { manifest, pkg = 'zip-bytes', sig } = {}) {
  const dir = join(root, id, version);
  mkdirSync(dir, { recursive: true });
  const pluginJson = manifest ?? { id, version, name: id, runtime: 'declarative' };
  writeFileSync(join(dir, 'plugin.json'), `${JSON.stringify(pluginJson, null, 2)}\n`);
  writeFileSync(join(dir, 'package.zip'), pkg, 'binary');
  if (sig !== undefined) writeFileSync(join(dir, 'package.zip.sig'), sig);
  return dir;
}

function sha256(content) {
  return createHash('sha256').update(content).digest('hex');
}

test('基本生成：sha256/URL/排序/签名透传/engines+deps 冗余', () => {
  const root = tempDir();
  writeVersion(root, 'demo.alpha', '0.2.0', {
    pkg: 'alpha-020',
    sig: 'untrusted comment: sig\nRWQ...\n',
    manifest: {
      id: 'demo.alpha',
      version: '0.2.0',
      name: 'Alpha',
      runtime: 'node',
      engines: { 'lite-code': '>=0.6.1' },
      extensionDependencies: ['demo.beta'],
    },
  });
  writeVersion(root, 'demo.alpha', '0.1.0', { pkg: 'alpha-010' });
  writeVersion(root, 'demo.beta', '1.0.0', { pkg: 'beta-100' });

  const { index, warnings } = buildIndex({ plugins: root, baseUrl: BASE });
  assert.equal(index.version, 1);
  assert.deepEqual(
    index.plugins.map((p) => `${p.id}@${p.version}`),
    ['demo.alpha@0.1.0', 'demo.alpha@0.2.0', 'demo.beta@1.0.0'],
  );

  const [oldAlpha, newAlpha, beta] = index.plugins;
  assert.equal(oldAlpha.dist.sha256, sha256('alpha-010'));
  assert.equal(newAlpha.dist.sha256, sha256('alpha-020'));
  assert.equal(beta.dist.sha256, sha256('beta-100'));
  assert.equal(oldAlpha.dist.url, joinUrl(BASE, 'demo.alpha', '0.1.0'));
  assert.equal(newAlpha.dist.url, `${BASE}/demo.alpha/0.2.0/package.zip`);
  assert.equal(newAlpha.signature, 'untrusted comment: sig\nRWQ...');
  assert.equal(oldAlpha.signature, undefined);
  assert.deepEqual(newAlpha.engines, { 'lite-code': '>=0.6.1' });
  assert.deepEqual(newAlpha.deps, ['demo.beta']);
  // 幂等：同输入两次输出一致。
  assert.deepEqual(buildIndex({ plugins: root, baseUrl: BASE }).index, index);
  // 未签名 → warning（不改变输出）。
  assert.equal(warnings.filter((w) => w.includes('未签名')).length, 2);
});

test('--require-signatures：缺 .sig 即失败；空签名文件也失败', () => {
  const root = tempDir();
  writeVersion(root, 'demo.unsigned', '0.1.0');
  assert.throws(
    () => buildIndex({ plugins: root, baseUrl: BASE, requireSignatures: true }),
    /--require-signatures/,
  );

  const signed = tempDir();
  writeVersion(signed, 'demo.signed', '0.1.0', { sig: 'sig-text\n' });
  assert.equal(
    buildIndex({ plugins: signed, baseUrl: BASE, requireSignatures: true }).index.plugins.length,
    1,
  );

  const emptySig = tempDir();
  writeVersion(emptySig, 'demo.empty', '0.1.0', { sig: '   \n' });
  assert.throws(
    () => buildIndex({ plugins: emptySig, baseUrl: BASE }),
    /签名文件为空/,
  );
});

test('目录与清单不一致 / 缺包 / 清单非法 → UsageError', () => {
  const mismatchId = tempDir();
  writeVersion(mismatchId, 'demo.one', '0.1.0', {
    manifest: { id: 'demo.two', version: '0.1.0', name: 'x' },
  });
  assert.throws(() => buildIndex({ plugins: mismatchId, baseUrl: BASE }), /id 与目录不一致/);

  const mismatchVersion = tempDir();
  writeVersion(mismatchVersion, 'demo.one', '0.1.0', {
    manifest: { id: 'demo.one', version: '0.2.0', name: 'x' },
  });
  assert.throws(() => buildIndex({ plugins: mismatchVersion, baseUrl: BASE }), /version 与目录不一致/);

  const badJson = tempDir();
  const dir = join(badJson, 'demo.bad', '0.1.0');
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, 'plugin.json'), '{not json');
  writeFileSync(join(dir, 'package.zip'), 'x');
  assert.throws(() => buildIndex({ plugins: badJson, baseUrl: BASE }), /不是合法 JSON/);

  const noPackage = tempDir();
  const ndir = join(noPackage, 'demo.nopkg', '0.1.0');
  mkdirSync(ndir, { recursive: true });
  writeFileSync(
    join(ndir, 'plugin.json'),
    JSON.stringify({ id: 'demo.nopkg', version: '0.1.0', name: 'x' }),
  );
  assert.throws(() => buildIndex({ plugins: noPackage, baseUrl: BASE }), /缺少安装包/);

  const badId = tempDir();
  writeVersion(badId, 'Bad Id', '0.1.0', {
    manifest: { id: 'Bad Id', version: '0.1.0', name: 'x' },
  });
  assert.throws(() => buildIndex({ plugins: badId, baseUrl: BASE }), /不是合法插件 id/);

  const badVersion = tempDir();
  writeVersion(badVersion, 'demo.ver', 'v1.0', {
    manifest: { id: 'demo.ver', version: 'v1.0', name: 'x' },
  });
  assert.throws(() => buildIndex({ plugins: badVersion, baseUrl: BASE }), /不是合法 semver/);
});

test('URL 前缀白名单：https / 回环 http / file；其余拒绝', () => {
  assert.equal(allowedBaseUrl('https://example.com/p'), true);
  assert.equal(allowedBaseUrl('http://127.0.0.1:8080/p'), true);
  assert.equal(allowedBaseUrl('http://localhost/p'), true);
  assert.equal(allowedBaseUrl('file:///tmp/plugins'), true);
  assert.equal(allowedBaseUrl('http://example.com/p'), false);
  assert.equal(allowedBaseUrl('ftp://example.com'), false);
  assert.equal(allowedBaseUrl('https://example.com/a b'), false);

  const root = tempDir();
  writeVersion(root, 'demo.one', '0.1.0');
  assert.throws(
    () => buildIndex({ plugins: root, baseUrl: 'http://example.com/p' }),
    /--base-url 必须是/,
  );
  // 尾部斜杠归一。
  const { index } = buildIndex({ plugins: root, baseUrl: 'https://example.com/p/' });
  assert.equal(index.plugins[0].dist.url, 'https://example.com/p/demo.one/0.1.0/package.zip');
});

test('参数与纯函数守卫', () => {
  assert.throws(() => buildIndex({ baseUrl: BASE }), /缺少 --plugins/);
  assert.throws(() => buildIndex({ plugins: tempDir() }), /缺少 --base-url/);
  assert.throws(() => buildIndex({ plugins: '/definitely/missing', baseUrl: BASE }), /目录不存在/);
  assert.throws(() => buildIndex({ plugins: tempDir(), baseUrl: BASE }), /没有可发布的插件版本/);

  assert.equal(validPluginId('demo.one'), true);
  assert.equal(validPluginId('demo/one'), false);
  assert.equal(validPluginId('.'), false);
  assert.equal(validPluginId(''), false);
  assert.equal(validSemver('0.6.1-preview.69'), true);
  assert.equal(validSemver('0.6.1'), true);
  assert.equal(validSemver('1.0'), false);
  assert.equal(validSemver('01.0.0'), false);

  assert.deepEqual(parseArgs(['--plugins', 'p', '--base-url', 'https://x', '--require-signatures']), {
    plugins: 'p',
    baseUrl: 'https://x',
    requireSignatures: true,
  });
  assert.throws(() => parseArgs(['--nope']), /未知参数/);

  // listVersions 只收两级目录结构，忽略普通文件。
  const root = tempDir();
  writeVersion(root, 'demo.one', '0.1.0');
  writeFileSync(join(root, 'README.md'), 'x');
  assert.deepEqual(listVersions(root), [{ id: 'demo.one', version: '0.1.0' }]);
});
