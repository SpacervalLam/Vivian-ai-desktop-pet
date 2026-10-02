import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import sharp from 'sharp';

const dir = 'public/stickers';
const rows = JSON.parse(await fs.readFile(path.join(dir, 'catalog.json'), 'utf8'));
assert.equal(rows.length, 24);
assert.equal(new Set(rows.map(row => row.id)).size, rows.length);
let totalBytes = 0;
for (const row of rows) {
  assert.equal(row.filename, `${row.id}-v${row.version}.webp`);
  assert.ok(row.id.startsWith(`${row.character_id}_`));
  const file = path.join(dir, row.filename);
  const {data, info} = await sharp(file).ensureAlpha().raw().toBuffer({resolveWithObject:true});
  assert.deepEqual([info.width, info.height, info.channels], [512, 512, 4]);
  let clearPixels = 0;
  for (let y=0; y<512; y++) for (let x=0; x<512; x++) {
    const alpha = data[(y*512+x)*4+3];
    if (alpha===0) clearPixels++;
    if (x<16 || x>=496 || y<16 || y>=496) assert.equal(alpha, 0, `${row.id}: transparent margin`);
  }
  assert.ok(clearPixels>512*512*0.2, `${row.id}: transparent canvas`);
  totalBytes += (await fs.stat(file)).size;
  if (process.argv.includes('--dist')) {
    assert.deepEqual(await fs.readFile(path.join('dist/stickers', row.filename)), await fs.readFile(file));
  }
}
assert.ok(totalBytes<3*1024*1024, 'Builtin sticker artwork must fit a 3 MB package budget');
assert.ok((await fs.readdir(dir)).every(file => file==='catalog.json' || /-v2\.webp$/.test(file)), 'No source sheets or legacy images');
if (process.argv.includes('--dist')) assert.equal((await fs.readdir('dist/stickers')).length, rows.length);
console.log(`Sticker assets: ${rows.length} transparent WebPs, ${(totalBytes/1024/1024).toFixed(2)} MB; ${process.argv.includes('--dist')?'production assets verified':'source assets verified'}.`);
