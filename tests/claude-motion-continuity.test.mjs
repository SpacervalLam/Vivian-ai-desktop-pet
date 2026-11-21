import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { resolve } from 'node:path';
import sharp from 'sharp';
import { build } from 'esbuild';
import { assertBottomDockFrame, assertSpriteSeam, clearTransparentRgb } from './helpers/character-assets.mjs';

const root = resolve('character-packs/claude');
const manifest = JSON.parse(await readFile(`${root}/character.json`, 'utf8'));
const result = await build({
  stdin: { contents: `import {initializeCharacterPackages} from './src/characterPackages';await initializeCharacterPackages();export * from './src/chibi/motionRegistry';export {leanContactX} from './src/chibi/wallLean';`, resolveDir: process.cwd() },
  bundle: true, write: false, format: 'esm', platform: 'node', logLevel: 'silent',
  plugins: [{ name: 'installed-claude', setup(b) {
    b.onResolve({ filter: /^@tauri-apps\/api\/core$/ }, () => ({ path: 'native', namespace: 'native' }));
    b.onLoad({ filter: /.*/, namespace: 'native' }, () => ({ contents: `export const invoke=async()=>(${JSON.stringify({ active: [{ manifest, root }] })});export const convertFileSrc=p=>p;` }));
  } }],
});
const registry = await import('data:text/javascript;base64,' + Buffer.from(result.outputFiles[0].text).toString('base64'));
const hash = pixels => createHash('sha256').update(pixels).digest('hex');
const decoded = new Map();
async function frames(name, direction = 'left') {
  const spec = registry.animation(name, 'claude');
  const file = registry.sheetUrl(spec, 'claude', direction);
  if (!decoded.has(file)) {
    const meta = await sharp(file).metadata();
    assert.deepEqual([meta.width, meta.height], [spec.cols * 512, spec.rows * 512], `${name}: physical and runtime grid agree`);
    const cells = [];
    for (let i = 0; i < spec.frames; i++) cells.push(clearTransparentRgb(await sharp(file).extract({ left: i % spec.cols * 512, top: Math.floor(i / spec.cols) * 512, width: 512, height: 512 }).ensureAlpha().raw().toBuffer()));
    decoded.set(file, cells);
  }
  return decoded.get(file);
}
const idle = clearTransparentRgb(await sharp(`${root}/assets/chibi/motion/claude/claude-idle-sheet.webp`).ensureAlpha().raw().toBuffer());
const names = ['blink','look','happy','angry','smug','think','thinking','cast','music','tired','float','remind','busy-in','busy-loop','edge','lean','edge-bottom','sleep','wake','turn','walk'];
let checked = 0;
for (const name of names) {
  const spec = registry.animation(name, 'claude');
  for (const resolved of [registry.getMotion(name, 'claude'), registry.resolveMotion(name, 'claude')]) {
    assert.equal(resolved.frames, spec.frames, `${name}: automatic and explicit playback agree`);
    assert.equal(resolved.rows, spec.rows, `${name}: renderer uses the character grid`);
  }
  assert.equal(spec.durations.length, spec.frames);
  assert(spec.durations.every(ms => ms > 0));
  for (const direction of spec.directions ?? ['left']) {
    const cells = await frames(name, direction), hashes = cells.map(hash);
    if (spec.loop && name !== 'thinking') assert.notEqual(hashes[0], hashes.at(-1), `${name}: no duplicated hold at the loop seam`);
    assert(new Set(hashes).size >= cells.length - 1, `${name}: no duplicated exposure padding`);
    for (let i = 0; i < cells.length; i++) {
      if (i) assert.notEqual(hashes[i], hashes[i - 1], `${name}/${i}: no frozen adjacent frame`);
      checked++;
    }
  }
}
for (const name of ['blink','happy','angry','smug','think','cast','remind']) {
  const cells = await frames(name);
  assertSpriteSeam(cells[0], idle, `${name}: idle entry`);
  // Blink returns to the idle pose in the renderer after its last unique frame.
  if (name !== 'blink') assertSpriteSeam(cells.at(-1), idle, `${name}: idle exit`);
}
const bottomDock = await frames('edge-bottom');
assertSpriteSeam(bottomDock[0], idle, 'bottom dock: idle entry');
let previousDockHeight = 512;
for (const [i, frame] of bottomDock.entries()) {
  previousDockHeight = assertBottomDockFrame(frame, idle, previousDockHeight, `bottom dock/${i + 1}`);
}
assert.deepEqual((await frames('busy-in')).at(-1), (await frames('busy-loop'))[0], 'open-book entry joins reading');
const lean = registry.animation('lean', 'claude');
assert.deepEqual(lean.directions, ['left', 'right']);
assert.notEqual(registry.sheetUrl(lean, 'claude', 'left'), registry.sheetUrl(lean, 'claude', 'right'));
for (const direction of ['left','right']) {
  const turn = registry.animation('turn', 'claude');
  const edge = registry.animation('edge', 'claude');
  assert.deepEqual((await frames('turn', direction)).at(-1), (await frames('walk', direction))[0], `${direction}: turn joins gait`);
  assert.notEqual(registry.sheetUrl(edge, 'claude', direction), registry.sheetUrl(turn, 'claude', direction),
    `${direction}: docking has its own reach and grip drawings`);
  assertSpriteSeam((await frames('edge', direction))[0], idle, `${direction}: docking starts in the original idle pose`);
  const leaning = await frames('lean', direction);
  assertSpriteSeam(leaning[0], idle, `${direction}: the reading lean starts in the original idle pose`);
  assertSpriteSeam(leaning[5], (await frames('busy-in')).at(-1), `${direction}: the book is open before leaning`);
  const [x, y] = lean.wallContact[direction];
  const alphaAt = column => leaning.at(-1)[(y * 512 + column) * 4 + 3];
  const inside = direction === 'left' ? x : x - 1;
  const outside = direction === 'left' ? x - 1 : x;
  assert(alphaAt(inside) > 200 && alphaAt(outside) <= 200, `${direction}: contact is on the wall-facing sleeve`);
  const expected = 50 + (x * 100 / 512 - 50) * manifest.presentation.defaultSize.scale;
  assert(Math.abs(registry.leanContactX(lean, 'claude', direction) - expected) < 1e-6, `${direction}: project the actual body contact`);
  for (const spec of [edge, lean]) for (let i = 0; i < spec.frames; i++) {
    const style = registry.frameStyle(spec, i, 'claude', direction);
    assert(Number.parseFloat(style.scale ?? '1') > 0, `${spec.name}/${direction}/${i}: preserve the accessory side`);
  }
}
assert.notDeepEqual((await frames('lean', 'left')).at(-1), (await frames('lean', 'right')).at(-1), 'each wall has a distinct reading pose');
const sleep = await frames('sleep'), wake = await frames('wake');
assert.deepEqual(sleep[0], idle);
assert.notDeepEqual(sleep.at(-1), idle, 'sleep remains asleep');
assert.equal(sleep.length, wake.length);
for (let i = 0; i < sleep.length; i++) assert.deepEqual(sleep[i], wake[sleep.length - 1 - i], 'wake retraces the same authored poses');
assert.deepEqual(wake.at(-1), idle);

// The first frame is the drawing master. Expressions may not repaint clothes,
// hair, the book or the body while changing a small facial feature.
async function lockedOutside(name, rect, reference = idle, direction = 'left') {
  const [x0,y0,x1,y1] = rect;
  for (const [i, pixels] of (await frames(name, direction)).entries()) {
    for (let y = 0; y < 512; y++) {
      const start = y * 512 * 4;
      const end = start + 2048;
      const check = (from, to) => assertSpriteSeam(pixels.subarray(from, to), reference.subarray(from, to), `${name}/${i}: static region on row ${y}`);
      if (y < y0 || y >= y1) check(start, end);
      else {
        check(start, start + x0 * 4);
        check(start + x1 * 4, end);
      }
    }
  }
}
for (const name of ['blink','look','happy','angry','smug','think','tired']) await lockedOutside(name, [190,146,342,225]);
await lockedOutside('remind', [180,230,345,340]);
await lockedOutside('cast', [149,227,342,338]);
await lockedOutside('busy-in', [188,235,342,338]);
await lockedOutside('busy-loop', [186,234,343,340], (await frames('busy-loop'))[0]);
for (const direction of ['left','right']) await lockedOutside('walk', [0,328,512,512], (await frames('walk', direction))[0], direction);
assert.equal(registry.animation('blink', 'vivian').frames, 12, 'other characters retain their timelines');
assert.equal(registry.animation('walk', 'vivian').frames, 14);
console.log(`Claude: ${checked} frames verified; no repeated padding, visually continuous seams, static regions, character-specific grids and timings.`);
