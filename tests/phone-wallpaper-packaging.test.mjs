import assert from 'node:assert/strict';
import { loadConfigFromFile } from 'vite';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

const root = process.cwd();
const config = await loadConfigFromFile({ command: 'build', mode: 'production' }, path.join(root, 'vite.config.ts'));
const plugin = config.config.plugins.find(item => item?.name === 'copy-public-assets');
assert.equal(typeof plugin.closeBundle, 'function');
const assets = ['riverside-original-enhanced.webp', 'manhattan-dusk.webp'];
const bytes = await Promise.all(assets.map(name => readFile(path.join(root, 'public/wallpapers', name))));
const dir = await mkdtemp(path.join(tmpdir(), 'vivian-phone-assets-'));
const warn = console.warn;
try {
  await mkdir(path.join(dir, 'public/wallpapers'), { recursive: true });
  for (let i = 0; i < assets.length; i++) await writeFile(path.join(dir, 'public/wallpapers', assets[i]), bytes[i]);
  plugin.configResolved({ root: dir, publicDir: path.join(dir, 'public'), build: { outDir: 'release-ui' } });
  console.warn = () => {};
  plugin.closeBundle();
  for (let i = 0; i < assets.length; i++) {
    assert.deepEqual(await readFile(path.join(dir, 'release-ui/wallpapers', assets[i])), bytes[i], `${assets[i]} reaches the configured release directory unchanged`);
  }
  await rm(path.join(dir, 'public/wallpapers/manhattan-dusk.webp'));
  assert.throws(() => plugin.closeBundle(), /必需的手机图片资源/, 'missing runtime images must fail the build');
  console.log('Phone release assets: wallpaper and photo copied, missing required asset rejects build.');
} finally {
  console.warn = warn;
  assert.ok(path.resolve(dir).startsWith(path.resolve(tmpdir()) + path.sep));
  await rm(dir, { recursive: true, force: true });
}
