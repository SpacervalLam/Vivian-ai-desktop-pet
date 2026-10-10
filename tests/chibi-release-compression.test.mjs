import assert from 'node:assert/strict';
import sharp from 'sharp';
import { readFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { releaseSheets, calibratedSheets, validateReleaseImage } from '../scripts/compress-chibi.mjs';

// Catch a regression that changes silhouettes or silently scales the atlas.
const pixels = Buffer.from([220, 204, 237, 255, 242, 232, 245, 127, 0, 0, 0, 0, 45, 30, 60, 255]);
const source = await sharp(pixels, { raw: { width: 2, height: 2, channels: 4 } }).png().toBuffer();
await validateReleaseImage(source, source);
const alphaChanged = Buffer.from(pixels); alphaChanged[7] = 128;
await assert.rejects(validateReleaseImage(source, await sharp(alphaChanged, { raw: { width: 2, height: 2, channels: 4 } }).png().toBuffer()), /alpha changed/);
await assert.rejects(validateReleaseImage(source, await sharp(source).resize(4, 4).png().toBuffer()), /dimensions changed/);
const darker = Buffer.from(pixels); darker[0] -= 20; darker[1] -= 20; darker[2] -= 20;
await assert.rejects(validateReleaseImage(source, await sharp(darker, { raw: { width: 2, height: 2, channels: 4 } }).png().toBuffer()), /colour error/);
assert.ok(releaseSheets.length > 12);
assert.equal(new Set(releaseSheets).size, releaseSheets.length);
assert.ok(calibratedSheets.every(path => releaseSheets.includes(path)));

if (process.argv.includes('--dist')) {
  let before = 0, after = 0, calibratedBefore = 0, calibratedAfter = 0;
  for (const path of releaseSheets) {
    const original = await readFile(join('public/chibi', path));
    const compressed = await readFile(join('dist/chibi', path));
    await validateReleaseImage(original, compressed);
    assert.ok(compressed.length <= original.length, `${path}: no size increase`);
    before += original.length; after += compressed.length;
    if (calibratedSheets.includes(path)) { calibratedBefore += original.length; calibratedAfter += compressed.length; }
  }
  assert.ok(calibratedAfter < calibratedBefore * .35, 'Calibrated release sheets must save at least 65%');
  assert.ok(after < before * .53, 'Expanded compression must save at least 47% across animation sheets');
  async function verifyOtherFiles(directory, relative = '') {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = [relative, entry.name].filter(Boolean).join('/');
      if (entry.isDirectory()) await verifyOtherFiles(join(directory, entry.name), path);
      else if (!releaseSheets.includes(path)) assert.deepEqual(await readFile(join('dist/chibi', path)), await readFile(join('public/chibi', path)), `${path}: other artwork preserved`);
    }
  }
  await verifyOtherFiles('dist/chibi');
  console.log(`Release sheets: ${(before/1048576).toFixed(2)} -> ${(after/1048576).toFixed(2)} MiB; dimensions, alpha, colour budget and other artwork verified.`);
}
console.log('Chibi release compression guards passed.');
