// Run the Windows paint-reentrancy regression against the vendored tao patch.
//
//   node src-tauri/vendor/tao/run-regression.mjs           # patched tao must survive
//   node src-tauri/vendor/tao/run-regression.mjs --control # stock 0.35.3 must panic
//
// The control run is the point: it proves the probe still triggers the real
// assertion, so a green patched run cannot be vacuous.
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

if (process.platform !== 'win32') {
  console.log('Windows paint-reentrancy regression requires Windows.');
  process.exit(0);
}

const control = process.argv.includes('--control');
const vendor = dirname(fileURLToPath(import.meta.url));
const root = resolve(vendor, '../../..');
const testDir = resolve(root, 'tmp/tao-paint-reentry');
const rustPath = p => p.replaceAll('\\', '/');

mkdirSync(testDir, { recursive: true });
writeFileSync(
  resolve(testDir, 'Cargo.toml'),
  `[package]
name = "tao-paint-reentry"
version = "0.0.0"
edition = "2021"

[workspace]

[[bin]]
name = "tao-paint-reentry"
path = "${rustPath(resolve(vendor, 'regression/paint_reentry.rs'))}"

[dependencies]
${control ? 'tao = "=0.35.3"' : `tao = { path = "${rustPath(vendor)}" }`}
`,
);

const result = spawnSync(
  'cargo',
  [
    'run',
    '--manifest-path',
    resolve(testDir, 'Cargo.toml'),
    '--offline',
    '--target-dir',
    resolve(testDir, 'target'),
    '-j',
    '2',
  ],
  { cwd: root, encoding: 'utf8' },
);
if (result.error) throw result.error;

const output = `${result.stdout ?? ''}${result.stderr ?? ''}`;
const summary = output.split('\n').find(line => line.startsWith('RESULT:')) ?? '(no RESULT line)';
console.log(summary);
if (result.status !== 0) {
  // Keep the failing tail available without dumping the whole cargo log.
  console.log(output.split('\n').filter(line => /PANIC|assertion failed|panicked at/.test(line)).join('\n'));
}

if (control) {
  const panicked = /assertion failed: flush_paint_messages/.test(output);
  if (!panicked) {
    console.error('control run did not reproduce the assertion; the probe is not exercising the bug');
    process.exit(1);
  }
  console.log('control OK: stock tao panics on the nested paint dispatch');
  process.exit(0);
}

if (result.status !== 0) {
  console.error(`patched tao failed the regression (exit ${result.status})`);
  process.exit(result.status ?? 1);
}
console.log('patched OK: nested paint dispatch is handled without panicking');
