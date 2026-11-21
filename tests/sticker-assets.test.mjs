import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import sharp from 'sharp';

const dir = 'public/stickers';
const coreRows = JSON.parse(await fs.readFile(path.join(dir, 'catalog.json'), 'utf8'));
const deepseekRows = JSON.parse(await fs.readFile('character-packs/deepseek/prompts/stickers.json', 'utf8'));
const claudeRows = JSON.parse(await fs.readFile('character-packs/claude/prompts/stickers.json', 'utf8'));
assert.ok(coreRows.every(row => ['vivian', 'nana'].includes(row.character_id)));
assert.ok(deepseekRows.every(row => row.character_id === 'deepseek'));
assert.equal(deepseekRows.length, 10);
assert.deepEqual(claudeRows, []);
const rows = [...coreRows, ...deepseekRows];
assert.ok(rows.length===39);
assert.equal(new Set(rows.map(row => row.id)).size, rows.length);
assert.equal(new Set(deepseekRows.map(row=>row.category)).size,10,'Exactly one new artwork per reaction type');
assert.ok(rows.every(row=>typeof row.category==='string'&&row.category.length>0));
let totalBytes = 0;
const marginViolations = [];
for (const row of rows) {
  assert.equal(row.filename, `${row.id}-v${row.version}.webp`);
  assert.ok(row.id.startsWith(`${row.character_id}_`));
  const packaged = row.character_id === 'deepseek';
  const file = path.join(packaged ? 'character-packs/deepseek/assets/stickers' : dir, row.filename);
  const {data, info} = await sharp(file).ensureAlpha().raw().toBuffer({resolveWithObject:true});
  assert.deepEqual([info.width, info.height, info.channels], [512, 512, 4]);
  let clearPixels = 0;
  let margin = 512;
  for (let y=0; y<512; y++) for (let x=0; x<512; x++) {
    const alpha = data[(y*512+x)*4+3];
    if (alpha===0) clearPixels++;
    else margin = Math.min(margin, x, y, 511 - x, 511 - y);
  }
  if (margin < 16) marginViolations.push(`${row.id} (${margin}px)`);
  assert.ok(clearPixels>512*512*0.2, `${row.id}: transparent canvas`);
  totalBytes += (await fs.stat(file)).size;
  if (process.argv.includes('--dist')) {
    const staged = path.join('tmp/optional-resources/stickers/stickers', row.filename);
    if (packaged) await assert.rejects(fs.access(staged), {code:'ENOENT'});
    else assert.deepEqual(await fs.readFile(staged), await fs.readFile(file));
    await assert.rejects(fs.access(path.join('dist/stickers', row.filename)), {code:'ENOENT'});
  }
}
for (const name of ['plead', 'peek']) {
  const file = path.join(dir, `vivian_ds_${name}_01-v2.webp`);
  const metadata = await sharp(file, {animated:true}).metadata();
  assert.ok(metadata.pages > 1, `${name}: animation preserved`);
  assert.equal(metadata.pageHeight, 512);
  assert.ok(metadata.delay.every(ms => ms > 0), `${name}: frame timing preserved`);
  const first = await sharp(file, {page:0}).raw().toBuffer();
  const later = await sharp(file, {page:Math.floor(metadata.pages / 2)}).raw().toBuffer();
  assert.notDeepEqual(first, later, `${name}: frames must visibly differ`);
}
assert.ok((await fs.readdir(dir)).every(file => file==='catalog.json' || /-v2\.webp$/.test(file)), 'No source sheets or legacy images');
if (process.argv.includes('--dist')) assert.equal((await fs.readdir('tmp/optional-resources/stickers/stickers')).length, coreRows.length);
const problems = [];
if (marginViolations.length) problems.push(`16px transparent margin required: ${marginViolations.join(', ')}`);
if (totalBytes >= 6 * 1024 * 1024) problems.push(`6 MiB artwork budget exceeded: ${(totalBytes / 1048576).toFixed(2)} MiB`);
assert.deepEqual(problems, [], 'Sticker artwork checks failed');
console.log(`Sticker assets: ${rows.length} transparent WebPs, ${(totalBytes/1024/1024).toFixed(2)} MB; ${process.argv.includes('--dist')?'production assets verified':'source assets verified'}.`);
