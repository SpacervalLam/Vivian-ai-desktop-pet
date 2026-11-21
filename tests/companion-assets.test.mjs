import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import sharp from 'sharp';
import { build } from 'esbuild';
const manifest = JSON.parse(await readFile('src/chibi/animations.json', 'utf8'));
const groups = [['nana', 'sleep',24], ['nana','wake',16], ['vivian','remind',12], ['nana','remind',12], ['vivian','tired',16], ['nana','tired',16], ['vivian','umbrella',16], ['nana','umbrella',16], ['vivian','music',16], ['nana','music',16], ['vivian','clipboard',1], ['nana','clipboard',1], ['vivian','look',16], ['nana','look',16]];
for (const [character, name, count] of groups) {
  const spec = manifest.animations.find(s => s.name === name);
  assert.ok(spec && (!spec.characters || spec.characters.includes(character)));
  assert.equal(spec.frames, count);
  assert.equal(spec.durations.length, count);
  assert.ok(spec.durations.every(ms => Number.isFinite(ms) && ms > 0));
  const sheet = `public${spec.sheet.replaceAll('{character}', character)}`;
  const metadata = await sharp(sheet).metadata();
  assert.equal(metadata.width, 512 * spec.cols);
  assert.equal(metadata.height, 512 * spec.rows);
  assert.equal(metadata.hasAlpha, true);
  assert.ok((await sharp(sheet).stats()).channels[3].min < 255);
  for (let index = 0; index < count; index++) {
    const stats = await sharp(sheet).extract({ left: index % spec.cols * 512, top: Math.floor(index / spec.cols) * 512, width: 512, height: 512 }).stats();
    assert.ok(stats.channels[3].min < 255 && stats.channels[3].max > 0, `${character}/${name}/${index}: transparent, nonempty cell`);
  }
  const dir = `public/chibi/motion/${character}/${name}`;
  const source = await readdir(dir).catch(error => { if (error.code === 'ENOENT') return null; throw error; });
  if (source === null) continue; // Raw source frames are optional; runtime always loads the sheet.
  const frames = source.filter(n => /\.png$/.test(n)).sort();
  assert.equal(frames.length, count);
  for (let index = 0; index < count; index++) {
    assert.equal(frames[index], `${String(index + 1).padStart(3,'0')}.png`);
    const frame = await sharp(`${dir}/${frames[index]}`).metadata();
    assert.equal(frame.width, 512); assert.equal(frame.height, 512); assert.equal(frame.hasAlpha, true);
  }
}
assert.equal(manifest.animations.find(s => s.name === 'sleep').hold, true);
assert.equal(manifest.animations.find(s => s.name === 'wake').loop, false);
assert.equal(manifest.animations.find(s => s.name === 'remind').loop, false);
assert.equal(manifest.animations.find(s => s.name === 'tired').loop, true);
assert.equal(manifest.animations.find(s => s.name === 'umbrella').loop, true);
assert.equal(manifest.animations.find(s => s.name === 'music').loop, true);
assert.equal(manifest.animations.find(s => s.name === 'clipboard').loop, false);
assert.equal(manifest.animations.find(s => s.name === 'gift'), undefined);
assert.equal(manifest.expression_aliases.sweat, 'dizzy');
const bundle = await build({ entryPoints: ['src/chibi/motionRegistry.ts'], bundle: true, write: false, platform: 'node', format: 'esm' });
const { resolveMotion } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
assert.equal(resolveMotion('gift', 'nana').name, 'idle');
assert.equal(resolveMotion('gift', 'vivian').name, 'idle');
assert.equal(resolveMotion('tend', 'vivian').name, 'idle');
// nana 没有 happy 帧序列，但图集里画过同名格位：角色门应回落到那张神态，而不是抹成 idle。
assert.equal(resolveMotion('happy', 'nana').name, 'happy');
assert.equal(resolveMotion('happy', 'vivian').name, 'happy');
for (const character of ['nana', 'vivian']) assert.equal(resolveMotion('look', character).name, 'look');
for (const character of ['nana', 'vivian']) {
  for (const name of ['music', 'clipboard', 'umbrella']) assert.equal(resolveMotion(name, character).name, name);
}
console.log('Companion animation groups: runtime grids, transparency, playback, character availability passed.');
