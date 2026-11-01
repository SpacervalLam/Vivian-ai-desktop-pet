// Browser automation runtime is an on-demand pack: trimmed, hashed and shipped separately.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { access, readFile, readdir, stat } from 'node:fs/promises';
import { join } from 'node:path';

const packs = JSON.parse(await readFile('src/optional-resources.json', 'utf8'));
const manifest = packs.find(pack => pack.id === 'browser-runtime');
assert(manifest, 'optional resource manifest must declare browser-runtime');
assert(manifest.download, 'the runtime is published separately instead of bundled by Setup');
const playwright = JSON.parse(await readFile('node_modules/playwright-core/package.json', 'utf8'));
assert.equal(manifest.version, playwright.version, 'runtime version must match playwright-core');
assert(Array.isArray(manifest.files) && manifest.files.length > 3, 'required files are declared');
assert(manifest.files.includes('node.exe'), 'Node is shipped with the runtime');
assert(manifest.files.includes('host.mjs') && manifest.files.includes('task.mjs'));
assert.match(manifest.download.url, /^https:\/\//);
assert.equal(manifest.download.filename, `Vivian-browser-runtime-${manifest.version}.zip`);
assert.equal(manifest.dev_root, 'src-tauri/browser-runtime');

const sha256 = async path => createHash('sha256').update(await readFile(path)).digest('hex');
const runtime = manifest.dev_root;
const exists = await access(runtime).then(() => true, () => false);
if (exists) {
  for (const forbidden of [
    'node_modules/playwright-core/types',
    'node_modules/playwright-core/lib/vite',
    'node_modules/playwright-core/lib/tools',
    'node_modules/playwright/types',
    'node_modules/playwright/lib/transform',
    'node_modules/playwright/lib/runner',
    'node_modules/playwright/cli.js',
  ]) {
    assert.equal(await access(join(runtime, forbidden)).then(() => true, () => false), false, `trimmed entry: ${forbidden}`);
  }
  for (const file of manifest.files) {
    const info = await stat(join(runtime, file)).catch(() => null);
    assert(info?.isFile() && info.size > 0, `required runtime file missing: ${file}`);
  }
  const pack = JSON.parse(await readFile(join(runtime, 'pack.json'), 'utf8'));
  assert.equal(pack.id, manifest.id);
  assert.equal(pack.version, manifest.version);
  if (process.platform === 'win32') {
    assert.equal(pack.payloads?.['node.exe'], 'node.exe.lz', 'Node is compressed inside the published archive');
  }
  const actual = (await readdir(runtime, { recursive: true, withFileTypes: true }))
    .filter(entry => entry.isFile())
    .map(entry => join(entry.parentPath ?? entry.path, entry.name).slice(runtime.length + 1).replace(/\\/g, '/'));
  const declared = Object.keys(pack.files).sort();
  assert.deepEqual(actual.filter(file => file !== 'pack.json').sort(), declared, 'pack manifest must describe exactly the shipped files');
  for (const file of declared) assert.equal(await sha256(join(runtime, file)), pack.files[file], `checksum: ${file}`);
  const releasePath = `release/${manifest.download.filename}`;
  if (await access(releasePath).then(() => true, () => false)) {
    const published = JSON.parse(await readFile('release/resource-packs.json', 'utf8'))
      .find(pack => pack.id === manifest.id);
    assert(published, 'published metadata must include the runtime');
    assert.equal(published.filename, manifest.download.filename);
    assert.equal(published.bytes, (await stat(releasePath)).size);
    assert.equal(published.sha256, await sha256(releasePath));
    assert.equal(published.version, manifest.version);
  }
  console.log(`[browser-runtime] verified ${declared.length} trimmed files`);
} else {
  console.log('[browser-runtime] generated runtime absent; run npm run package:browser-runtime to verify the archive');
}
