import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { readFile, access } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import packs from '../src/optional-resources.json' with { type: 'json' };

assert.deepEqual(packs.map(pack => pack.id), ['fonts', 'stickers']);
const stickerCatalog = JSON.parse(await readFile('public/stickers/catalog.json', 'utf8'));
assert.deepEqual(new Set(packs.find(pack => pack.id === 'stickers').files), new Set(stickerCatalog.map(row => `stickers/${row.filename}`)));
assert.equal(new Set(packs.flatMap(pack => pack.files)).size, packs.reduce((total, pack) => total + pack.files.length, 0));

// Production resource resolution: absent packs never request omitted core URLs.
let statuses = [];
globalThis.__resourceStatus = () => statuses;
const output = await build({ entryPoints: ['src/utils/optionalResources.ts'], bundle: true, write: false,
  platform: 'node', format: 'esm', define: { 'import.meta.env.DEV': 'false' }, plugins: [{ name: 'resource-bridge', setup(b) {
    b.onResolve({ filter: /^@tauri-apps\/api\/core$/ }, () => ({ path: 'native', namespace: 'resource' }));
    b.onLoad({ filter: /.*/, namespace: 'resource' }, () => ({ contents: 'export const invoke=async()=>globalThis.__resourceStatus();export const convertFileSrc=p=>"asset:"+p;' }));
  }}] });
const resources = await import('data:text/javascript;base64,' + Buffer.from(output.outputFiles[0].text).toString('base64'));
await resources.initializeOptionalResources();
assert.equal(resources.optionalAssetSource('/fonts/ma-shan-zheng.woff2'), null);
assert.equal(resources.optionalAssetSource('/stickers/vivian_cheer_01-v2.webp'), null);
assert.equal(resources.optionalAssetSource('/chibi/vivian-atlas.webp'), '/chibi/vivian-atlas.webp');
statuses = [{ id: 'stickers', installed: true, root: 'C:/Pets/optional/stickers' }];
await resources.initializeOptionalResources();
assert.equal(resources.optionalAssetSource('/stickers/vivian_cheer_01-v2.webp'), 'asset:C:/Pets/optional/stickers/stickers/vivian_cheer_01-v2.webp');
assert.equal(resources.resourcePackInstalled('stickers'), true);
assert.equal(resources.resourcePackInstalled('fonts'), false);
delete globalThis.__resourceStatus;

if (process.argv.includes('--dist')) {
  const artifacts = JSON.parse(await readFile('release/resource-packs.json', 'utf8'));
  assert.equal(artifacts.length, 2);
  for (const pack of packs) {
    for (const file of pack.files) await assert.rejects(access(`dist/${file}`), { code: 'ENOENT' });
    const artifact = artifacts.find(item => item.id === pack.id);
    assert.ok(artifact);
    assert.equal(createHash('sha256').update(await readFile(`release/${artifact.filename}`)).digest('hex'), artifact.sha256);
    const result = spawnSync('python', ['-c',
      'import sys,json,hashlib\nfrom zipfile import ZipFile\nwith ZipFile(sys.argv[1]) as z:\n m=json.loads(z.read("pack.json"))\n assert m["id"]==sys.argv[2]\n assert set(z.namelist())==set(json.loads(sys.argv[3]))|{"pack.json"}\n for n,h in m["files"].items(): assert hashlib.sha256(z.read(n)).hexdigest()==h\n',
      `release/${artifact.filename}`, pack.id, JSON.stringify(pack.files)], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
  }
}
console.log('Optional resources: missing-pack fallback, installed URLs, core artwork, package boundaries and archive integrity passed.');
