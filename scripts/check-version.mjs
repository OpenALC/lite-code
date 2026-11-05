#!/usr/bin/env node
// 版本号守卫：CI、release、本地共用。
// 规则见 docs/VERSIONING.md。两种模式：
//   默认（PR/日常）：一致性 + 合法 semver + 主版本锁 0 + 0.x 必带 -preview.N
//   --release --tag vX：追加「manifest 必须等于 tag」「第二段必须等于当前里程碑」
// 退出码：0 通过，1 违规。

import { readFileSync, existsSync } from 'node:fs';

const args = process.argv.slice(2);
const has = (f) => args.includes(f);
const opt = (f) => { const i = args.indexOf(f); return i >= 0 ? args[i + 1] : null; };

const tag = opt('--tag');
const release = has('--release') || !!tag;
const allowV1 = has('--allow-v1') || process.env.ALLOW_V1 === 'true';
const fail = [];

const readJson = (p) => JSON.parse(readFileSync(p, 'utf8'));
const manifests = {};
if (existsSync('Cargo.toml')) {
  const m = /^version\s*=\s*"([^"]+)"/m.exec(readFileSync('Cargo.toml', 'utf8'));
  if (m) manifests['Cargo.toml'] = m[1];
}
if (existsSync('package.json')) manifests['package.json'] = readJson('package.json').version;
if (existsSync('apps/desktop/src-tauri/tauri.conf.json')) {
  manifests['tauri.conf.json'] = readJson('apps/desktop/src-tauri/tauri.conf.json').version;
}
const names = Object.keys(manifests);
if (!names.length) { console.log('check-version: 无 manifest，跳过'); process.exit(0); }

// 1) 三处一致
const uniq = [...new Set(Object.values(manifests))];
if (uniq.length > 1) {
  fail.push(`版本号不一致：\n${names.map(n => `    - ${n}: ${manifests[n]}`).join('\n')}`);
}
const v = uniq[0];

// 2) semver 与结构
const re = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?$/;
const m = re.exec(v);
if (!m) {
  fail.push(`'${v}' 不是合法 semver（不要写 1.0 / 1.2.3-preview1 这类形式）`);
} else {
  const [, major, minor, , pre] = m;

  // 3) 主版本锁
  if (major !== '0' && !allowV1) {
    fail.push(
      `主版本为 ${major}，未获授权。\n` +
      `    第一部分只有维护者明确声明时才增加，否则恒为 0（docs/VERSIONING.md §1）。\n` +
      `    执行者不得自行改动；如确为正式版，由维护者设置仓库变量 ALLOW_V1=true 并打 tag v${v}。`
    );
  }

  // 4) preview 后缀
  if (major === '0' && !/^preview\.\d+$/.test(pre || '')) {
    fail.push(
      `'${v}' 缺少 '-preview.N' 后缀（当前为 ${pre ? `'${pre}'` : '无后缀'}）。\n` +
      `    未声明正式版前，版本必须形如 0.6.1-preview.1（点号不可省）。`
    );
  }
  if (major !== '0' && pre) fail.push(`正式版不得带预发布后缀（收到 '${v}'）。`);

  // 5) release 模式：tag 与里程碑对齐
  if (release) {
    if (tag && tag.replace(/^v/, '') !== v) {
      fail.push(`tag ${tag} 与 manifest 版本 ${v} 不一致。`);
    }
    let milestone = opt('--milestone');
    if (!milestone && existsSync('.milestone')) milestone = readFileSync('.milestone', 'utf8').trim();
    if (milestone) {
      const want = Number(String(milestone).replace(/^P/i, ''));
      if (Number(minor) !== want) {
        fail.push(
          `第二段为 ${minor}，但当前里程碑是 P${want}（应为 0.${want}.x）。\n` +
          `    版本线：P0=0.0.x，P1=0.1.x …… P6=0.6.x（docs/VERSIONING.md §2）。`
        );
      }
    }
  }
}

if (fail.length) {
  console.error('✗ 版本号校验失败：\n');
  for (const f of fail) console.error('  • ' + f + '\n');
  process.exit(1);
}
console.log(`✓ ${v}${release ? '（release 校验）' : ''}`);
