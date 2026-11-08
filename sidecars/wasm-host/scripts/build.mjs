#!/usr/bin/env node
/**
 * Copies the wasm-host runner into crates/lc-core/assets/wasm-host.mjs.
 *
 * The asset is the single-file artifact embedded by Rust. No bundling or
 * dependencies are involved; the source is already a standalone ESM module.
 *
 *   node sidecars/wasm-host/scripts/build.mjs          # write asset
 *   node sidecars/wasm-host/scripts/build.mjs --check  # fail if stale
 */

import { copyFileSync, readFileSync } from 'node:fs';
import { dirname, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../../..');
const source = resolve(root, 'sidecars/wasm-host/src/wasm-host.mjs');
const target = resolve(root, 'crates/lc-core/assets/wasm-host.mjs');
const check = process.argv.includes('--check');

if (check) {
  const expected = readFileSync(source, 'utf8');
  let actual;
  try {
    actual = readFileSync(target, 'utf8');
  } catch {
    actual = null;
  }
  if (actual !== expected) {
    process.stderr.write(
      `${relative(root, target)} is stale; run node sidecars/wasm-host/scripts/build.mjs\n`,
    );
    process.exitCode = 1;
  }
} else {
  copyFileSync(source, target);
  process.stdout.write(`wrote ${relative(root, target)}\n`);
}
