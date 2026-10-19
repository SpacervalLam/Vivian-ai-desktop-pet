import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, utimesSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

const root = mkdtempSync(join(tmpdir(), 'vivian-packaging-test-'));
const script = resolve('scripts/package-small.mjs');
const put = (name, contents) => {
  const path = join(root, name);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, contents);
  return path;
};
try {
  put('dist/index.html', 'current frontend');
  put('dist/assets/MemoryWindow-current.js', 'current room');
  put('dist/assets/MemoryWindow-current.css', 'current styles');
  put('src-tauri/windows/resource-packages.nsh', 'current resources');
  put('plugins/3d-apartment/plugin.json', JSON.stringify({ version: '1.1.0' }));
  put('release/resource-packs.json', '[]');
  const published = put('release/Vivian_1.0.1_x64-small-setup.exe', 'keep published installer');
  const setup = put('src-tauri/target/release/bundle/nsis/Vivian_1.0.1_x64-setup.exe', 'stale installer');
  const old = new Date(Date.now() - 60000);
  utimesSync(setup, old, old);
  put('src-tauri/target/release/vivian.exe', 'MemoryWindow-current.js MemoryWindow-current.css');
  let result = spawnSync(process.execPath, [script, '--collect-only'], { cwd: root, encoding: 'utf8' });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Installer is older/);
  assert.equal(readFileSync(published, 'utf8'), 'keep published installer');

  put('src-tauri/target/release/vivian.exe', 'MemoryWindow-previous.js MemoryWindow-previous.css');
  result = spawnSync(process.execPath, [script, '--collect-only'], { cwd: root, encoding: 'utf8' });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /does not embed the current room assets/);
  assert.equal(readFileSync(published, 'utf8'), 'keep published installer');
  console.log('Packaging: stale installers and stale embedded room assets cannot overwrite a published installer.');
} finally {
  assert.equal(dirname(resolve(root)), resolve(tmpdir()));
  assert.ok(root.includes('vivian-packaging-test-'));
  rmSync(root, { recursive: true, force: true });
}
