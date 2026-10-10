import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import sharp from 'sharp';
import { build } from 'esbuild';
const manifest = JSON.parse(await readFile('src/chibi/animations.json', 'utf8'));
const groups = [['nana', 'sleep',24], ['nana','wake',8], ['vivian','remind',12], ['nana','remind',12], ['vivian','tired',8], ['nana','tired',8], ['vivian','umbrella',8], ['nana','umbrella',8], ['vivian','music',8], ['nana','music',8], ['vivian','clipboard',1], ['nana','clipboard',1]];
for (const character of ['vivian', 'nana']) for (const hand of ['rock', 'scissors', 'paper']) groups.push([character, `rps-${hand}`, 1]);
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
assert.equal(resolveMotion('happy', 'nana').name, 'idle');
assert.equal(resolveMotion('happy', 'vivian').name, 'happy');
for (const character of ['nana', 'vivian']) {
  for (const name of ['music', 'clipboard', 'umbrella', 'rps-rock', 'rps-scissors', 'rps-paper']) assert.equal(resolveMotion(name, character).name, name);
  for (const [alias, name] of [['reach-out','rps-paper'],['伸手','rps-paper'],['cheer','rps-rock'],['加油打气','rps-rock'],['victory','rps-scissors'],['比耶','rps-scissors']]) {
    assert.equal(resolveMotion(alias, character).name, name);
    assert.equal(resolveMotion(alias, character).promptable, true);
  }
}
const provenance = JSON.parse(await readFile('assets/chibi/rps/generation.json', 'utf8'));
assert.equal(provenance.records.length, 6);
for (const record of provenance.records) {
  const png = record.sheet.replace(`${record.character}-rps-${record.hand}-sheet.webp`, `rps-${record.hand}/001.png`);
  const sourceMetadata = await sharp(record.source).metadata();
  assert.equal(sourceMetadata.hasAlpha, true, `${record.character}/${record.hand}: recorded source exists`);
  // Generated intermediate frames are optional, just like the source directories above.
  const frame = await readFile(png).catch(error => { if (error.code === 'ENOENT') return null; throw error; });
  if (frame !== null) {
    const original = await sharp(frame).flatten({ background: '#ffffff' }).raw().toBuffer();
    const decoded = await sharp(record.sheet).flatten({ background: '#ffffff' }).raw().toBuffer();
    assert.deepEqual(decoded, original, `${record.character}/${record.hand}: lossless visible pixels`);
  }
  const { data, info } = await sharp(record.sheet).ensureAlpha().raw().toBuffer({ resolveWithObject: true });
  let bottom = -1;
  for (let y = 0; y < info.height; y++) for (let x = 0; x < info.width; x++) if (data[(y * info.width + x) * 4 + 3] >= 16) bottom = y;
  assert.ok(Math.abs(bottom - record.foot_baseline) <= 2, `${record.character}/${record.hand}: baseline`);
}
console.log('18 companion animation groups: 112 transparent cells, source frames, playback, character availability and RPS baselines passed');
