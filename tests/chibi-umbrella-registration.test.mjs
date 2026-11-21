import assert from 'node:assert/strict';
import { access, readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { build } from 'esbuild';
import sharp from 'sharp';
import { bounds, faceBounds } from '../scripts/chibi/frame-sheets/registration.mjs';
import { frameDifference } from '../scripts/chibi/audit-temporal-continuity.mjs';

const packaged = ['deepseek', 'claude', 'chatgpt'];
// Opaque bounds from the umbrella redraws before atlas packing.
const artworkAspect = { nana: 1035 / 1417, claude: 894 / 1361, chatgpt: 882 / 1325 };
const packs = await Promise.all(packaged.map(async id => {
  const root = resolve('character-packs', id);
  const manifest = JSON.parse(await readFile(`${root}/character.json`, 'utf8'));
  return { root, manifest };
}));
const bundle = await build({
  stdin: {
    contents: `import { initializeCharacterPackages } from './src/characterPackages';
      await initializeCharacterPackages();
      export { resolveMotion, frameStyle, sheetUrl } from './src/chibi/motionRegistry';
      export { defaultSizeStyle } from './src/chibi/defaultSize';`,
    resolveDir: process.cwd(),
  },
  bundle: true, write: false, platform: 'node', format: 'esm',
  plugins: [{ name: 'resident-packages', setup(builder) {
    builder.onResolve({ filter: /^@tauri-apps\/api\/core$/ }, () => ({ path: 'native', namespace: 'native' }));
    builder.onLoad({ filter: /.*/, namespace: 'native' }, () => ({
      contents: `export const invoke = async () => ({ active: ${JSON.stringify(packs)} }); export const convertFileSrc = path => path;`,
    }));
  } }],
});
const { resolveMotion, frameStyle, sheetUrl, defaultSizeStyle } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`
);
const cell = 512;
const readFrame = (file, cols, i) => sharp(file)
  .extract({ left: i % cols * cell, top: Math.floor(i / cols) * cell, width: cell, height: cell })
  .ensureAlpha().raw().toBuffer();
const percent = value => parseFloat(value) * cell / 100;

function transformed(style, origin = '50% 90%') {
  const scale = Number(style.scale ?? 1);
  const [dx, dy] = (style.translate ?? '0% 0%').split(' ').map(percent);
  const [ox, oy] = (style.transformOrigin ?? origin).split(' ').map(percent);
  return { scale, x: x => (x - ox) * scale + ox + dx, y: y => (y - oy) * scale + oy + dy };
}

const deepseek = packs.find(entry => entry.manifest.id === 'deepseek');
const retiredSheet = '/chibi/motion/deepseek/deepseek-umbrella-sheet.webp';
assert.equal(resolveMotion('umbrella', 'deepseek').name, 'idle');
assert(!('umbrella' in deepseek.manifest.animations));
assert(!(`assets${retiredSheet}` in deepseek.manifest.files));
assert(!(retiredSheet in deepseek.manifest.anchors));
assert(!(retiredSheet in deepseek.manifest.scales));
await assert.rejects(access(`${deepseek.root}/assets${retiredSheet}`), { code: 'ENOENT' });

for (const id of ['vivian', 'nana', 'claude', 'chatgpt']) {
  const spec = resolveMotion('umbrella', id);
  assert.equal(spec.kind, 'animation');
  assert(spec.loop, `${id}: holds the umbrella in a loop`);
  const pack = packs.find(entry => entry.manifest.id === id);
  const root = pack ? `${pack.root}/assets` : 'public';
  const idlePath = id === 'nana' ? `/chibi/${id}-atlas.webp` : `/chibi/motion/${id}/${id}-idle-sheet.webp`;
  const idle = await readFrame(root + idlePath, id === 'nana' ? 3 : 1, 0);
  const idleBox = bounds(idle, cell, cell);
  const idleFace = id === 'vivian' ? faceBounds(idle, cell, cell, idleBox) : null;
  const outer = transformed(defaultSizeStyle(id));
  const url = sheetUrl(spec, id, 'left');
  const sheet = pack ? url : `public${url}`;
  const frames = [];
  const frameScales = new Set();

  for (let i = 0; i < spec.frames; i++) {
    const label = `${id}/umbrella/${i}`;
    const pixels = await readFrame(sheet, spec.cols, i);
    const box = bounds(pixels, cell, cell);
    const inner = transformed(frameStyle(spec, i, id));
    frameScales.add(inner.scale);
    assert(Math.abs(outer.y(inner.y(box.y1)) - outer.y(idleBox.y1)) < 1.25, `${label}: same foot baseline as idle`);
    if (idleFace) {
      // Vivian's restored animation moves the umbrella independently of the body.
      const face = faceBounds(pixels, cell, cell, box);
      assert(Math.abs(inner.y(face.y1) - idleFace.y1) <= 2, `${label}: chin stays at idle height`);
      assert(Math.abs(face.width * inner.scale / idleFace.width - 1) < .05, `${label}: face keeps the idle size`);
    } else {
      assert(Math.abs(box.width / box.height / artworkAspect[id] - 1) < .025, `${label}: retains the original drawing's aspect ratio`);
    }
    // The stage is at most 96vw wide and 94vh tall, centered horizontally and bottom-aligned.
    const horizontalMargin = cell * .02 / .96;
    const topMargin = cell * .06 / .94;
    assert(outer.x(inner.x(box.x0)) > -horizontalMargin && outer.x(inner.x(box.x1)) < cell + horizontalMargin &&
      outer.y(inner.y(box.y0)) > -topMargin && outer.y(inner.y(box.y1)) < cell, `${label}: full parasol fits the desktop window`);
    frames.push(pixels);
  }

  assert.equal(frameScales.size, 1, `${id}: the artwork uses one scale throughout the loop`);
  if (id !== 'vivian') {
    for (const [i, pixels] of frames.entries()) {
      assert(frameDifference(frames[0], pixels) < 14, `${id}/umbrella/${i}: consistent parasol and grip`);
      assert(frameDifference(pixels, frames[(i + 1) % frames.length]) < 8, `${id}/umbrella/${i}: smooth transition including the loop seam`);
    }
  }
}
console.log('DeepSeek falls back to idle without an umbrella sheet; the other four umbrella loops retain their registration.');
