import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { build } from 'esbuild';
import sharp from 'sharp';

const active = await Promise.all(['chatgpt', 'deepseek', 'claude'].map(async id => {
  const root = `character-packs/${id}`;
  return { root, manifest: JSON.parse(await readFile(`${root}/character.json`, 'utf8')) };
}));
const testManifest = active.find(({ manifest }) => manifest.id === 'deepseek').manifest;
assert.equal(testManifest.animations.lean, undefined, 'the current DeepSeek pack has no lean motion');
// A legacy installed pack may still advertise a lean. The current runtime must ignore it.
testManifest.animations.lean = active.find(({ manifest }) => manifest.id === 'chatgpt').manifest.animations.lean;
for (const name of ['edge', 'edge-bottom']) {
  const motion = testManifest.animations[name];
  testManifest.scales[motion.sheet][motion.frames - 1] = [0.8, 90];
}
const bundle = await build({ stdin: { contents: `export * from './src/chibi/edgeDock';
  export { initializeCharacterPackages } from './src/characterPackages';
  export { leanContactX, wallLeanOffsets } from './src/chibi/wallLean';
  export { defaultSizeTransform } from './src/chibi/defaultSize';
  export { resolveMotion, frameStyle } from './src/chibi/motionRegistry';
  export { speechDisplayMotion } from './src/chibi/speechPose';`, resolveDir: process.cwd() },
  bundle: true, write: false, format: 'esm', platform: 'node',
  plugins: [{ name: 'installed-characters', setup(b) {
    b.onResolve({ filter: /^@tauri-apps\/api\/core$/ }, () => ({ path: 'native', namespace: 'native' }));
    b.onLoad({ filter: /.*/, namespace: 'native' }, () => ({ contents:
      `export const invoke = async () => (${JSON.stringify({ active })}); export const convertFileSrc = path => path;`,
    }));
  } }],
});
const { planEdgeDock, planScreenDock, dockMotion, dockClip, dockEntryPlayback, resolveMotion, frameStyle, speechDisplayMotion, leanContactX, wallLeanOffsets, defaultSizeTransform, initializeCharacterPackages } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
for (const [left, right, scale] of [[0,1920,1],[-2560,0,2],[1920,4480,1.5]]) {
  const width = 300 * scale;
  for (const side of ['left','right']) {
    const plan = planEdgeDock(left + 700, width, left, right, scale, 6, 288, side);
    const boundary = side === 'left' ? left : right;
    const grip = 6 + 288 * .5;
    assert(Math.abs(plan.dockX + grip * scale - boundary) <= 1, 'authored grip meets the physical screen edge');
    assert.equal(plan.walkX, side === 'left' ? left : right - width, 'walk to the visible edge first');
    assert.equal(dockClip(plan.walkX, plan), 'inset(0px 0px 0px 0px)');
    const margins = dockClip(plan.dockX, plan).match(/[\d.]+(?=px)/g).map(Number);
    assert(Math.abs(margins[side === 'left' ? 3 : 1] - 150) < 1);
  }
}
assert.equal(planEdgeDock(50,300,0,1920,1,0,300).side,'left');
assert.equal(planEdgeDock(1500,300,0,1920,1,0,300).side,'right');
for (const scale of [1,1.5,2]) {
  const screen = {left:-1920,right:0,top:-1080,bottom:0,scale};
  const size = {width:300*scale,height:320*scale};
  const sprite = {left:6,top:20,width:288,height:288};
  for (const side of ['bottom']) {
    const plan = planScreenDock({x:-1000,y:-600},size,screen,sprite,side);
    const boundary = screen.bottom;
    assert(Math.abs(plan.dockY + (20+288*.5)*scale-boundary)<=1);
    assert.equal(plan.dockX,plan.walkX,'vertical art stays upright without faking a horizontal rotation');
    const clip = dockClip(plan.dockX,plan,plan.dockY).match(/[\d.]+(?=px)/g).map(Number);
    assert(clip[2]>0);
    assert.equal(clip[1],0);assert.equal(clip[3],0);
    const spec = resolveMotion(dockMotion(side),'vivian');
    assert.match(spec.sheet,new RegExp(`edge-${side}-sheet`));
    assert.equal(frameStyle(spec,3,'vivian','right').scale,undefined);
    assert.equal(speechDisplayMotion(spec.name,true,false),spec.name);
  }
}
const screen = {left:0,right:1920,top:0,bottom:1080,scale:1};
const size = {width:300,height:300}, sprite = {left:0,top:0,width:300,height:300};
assert.notEqual(planScreenDock({x:800,y:0},size,screen,sprite).side,'top');
assert.notEqual(planScreenDock({x:800,y:0},size,screen,sprite,'top').side,'top','legacy explicit top requests cannot trigger top docking');
assert.equal(planScreenDock({x:800,y:780},size,screen,sprite).side,'bottom');
const edge = resolveMotion('edge','vivian'), lean = resolveMotion('lean','nana');
assert.match(edge.sheet,/edge-peek-sheet/);
assert.match(lean.sheet,/nana-lean-\{direction\}-sheet/,'Nana uses the repurposed independent left and right drawings');
assert.equal(lean.hold,false,'random leaning returns to idle');
assert.equal(frameStyle(edge,7,'vivian','left').scale,'-1 1');
assert.equal(speechDisplayMotion('edge',true,false,'talk'),'edge','speech keeps the clinging pose');

for (const side of ['left', 'right']) {
  const {approach, settled, slideDuration} = dockEntryPlayback(edge, side);
  assert.deepEqual(approach.frames, [0, 1, 2, 3, 4, 5, 6, 7], `Vivian ${side}: approach uses the complete drawings`);
  assert.equal(slideDuration, 660, `Vivian ${side}: reach the edge before the first cropped drawing`);
  assert.deepEqual(settled.frames, [8, 9, 10, 11, 12, 13, 14, 15]);
  assert.deepEqual([...approach.durations, ...settled.durations], edge.durations);
}

for (const id of ['chatgpt', 'claude']) {
  assert.equal(leanContactX(resolveMotion('lean', id), id, 'right'), null, `${id}: missing package metadata does not get an app-provided mark`);
}
await initializeCharacterPackages();
for (const id of ['vivian', 'deepseek']) {
  assert.equal(resolveMotion('lean', id).name, 'idle', `${id} does not lean`);
}
const cell = 512;
const deepseekPackage = active.find(({ manifest }) => manifest.id === 'deepseek');
const deepseekEdge = resolveMotion(dockMotion('left'), 'deepseek');
const edgeHold = deepseekEdge.frames - 1;
const edgeFrame = await sharp(`${deepseekPackage.root}/assets${deepseekEdge.sheet}`).extract({
  left: edgeHold % deepseekEdge.cols * cell, top: Math.floor(edgeHold / deepseekEdge.cols) * cell,
  width: cell, height: cell,
}).ensureAlpha().raw().toBuffer();
const gripX = 142;
const gripY = 305;
const edgeAlpha = (x, y) => edgeFrame[(y * cell + x) * 4 + 3];
assert.ok(edgeAlpha(gripX - 1, gripY) <= 200 && edgeAlpha(gripX, gripY) > 200,
  'DeepSeek edge: the authored drawing holds a wall on its left');
for (const [side, sign] of [['left', 1], ['right', -1]]) {
  const style = frameStyle(deepseekEdge, edgeHold, 'deepseek', side);
  const scale = Number.parseFloat(style.scale ?? '1');
  const offset = Number.parseFloat(style.translate);
  assert.equal(scale, sign, `DeepSeek ${side}: docking reflects the frame at its original scale`);
  const baseScale = deepseekPackage.manifest.presentation.defaultSize.scale;
  const project = x => 50 + (offset + (x * 100 / cell - 50) * scale) * baseScale;
  assert.ok(Math.abs(project(gripX) - 50) <= 0.2, `DeepSeek ${side}: the palms meet the crop line`);
  assert.ok((project(240) - 50) * sign > 15, `DeepSeek ${side}: the face stays inside the monitor`);
}
const bottom = resolveMotion('edge-bottom', 'deepseek');
assert.equal(frameStyle(bottom, bottom.frames - 1, 'deepseek', 'right').scale, undefined,
  'bottom docking leaves the scale of the frame unchanged');

for (const { root, manifest } of active) {
  const { id } = manifest;
  if (id !== 'chatgpt') continue;
  const spec = resolveMotion('lean', id);
  const last = spec.frames - 1;
  const [palm, palmRow] = spec.wallContact.right;
  assert.deepEqual(spec.wallContact.left, spec.wallContact.right, `${id}: mirrored directions use the same source point`);
  const frame = await sharp(`${root}/assets${spec.sheet}`).extract({
    left: last % spec.cols * cell, top: Math.floor(last / spec.cols) * cell, width: cell, height: cell,
  }).ensureAlpha().raw().toBuffer();
  const alpha = (x, y) => frame[(y * cell + x) * 4 + 3];
  assert.ok(alpha(palm - 1, palmRow) > 200 && alpha(palm, palmRow) <= 200, `${id}: the package mark is on the painted palm edge`);

  const [anchorX] = manifest.anchors[spec.sheet][last];
  const [frameScale] = manifest.scales[spec.sheet][last];
  for (const [side, sign] of [['left', -1], ['right', 1]]) {
    const expected = 50 + (anchorX + (palm * 100 / cell - 50) * frameScale * sign) * manifest.presentation.defaultSize.scale;
    assert.equal(leanContactX(spec, id, side), expected, `${id}/${side}: package registration and mirroring are applied`);
  }
}
const chatGptDock = resolveMotion('edge', 'chatgpt');
for (const side of ['left', 'right']) {
  const playback = dockEntryPlayback(chatGptDock, side);
  assert.deepEqual(playback.approach.frames, [0, 1, 2], 'ChatGPT moves with an intact drawing');
  assert.equal(playback.settled.frames[0], 3, 'the first cutout waits for the edge');
  assert.equal(playback.slideDuration, 520);
}
for (const id of ['nana', 'chatgpt']) {
  const spec = resolveMotion('lean', id);
  const {from} = spec.wallContactFrames;
  for (const side of ['left', 'right']) {
    const target = side === 'left' ? 0 : 100;
    const offsets = wallLeanOffsets(spec, id, side, target);
    assert.equal(Math.abs(offsets[0]), 0);
    const marks = spec.wallContactFrames[side] ?? spec.wallContactFrames.right;
    for (const [index, x] of marks.entries()) {
      const {scale, translate} = frameStyle(spec, from + index, id, side);
      const projected = 50 + (parseFloat(translate ?? '0') + (x * 100 / cell - 50) * parseFloat(scale ?? '1'))
        * defaultSizeTransform(id).scale;
      assert(Math.abs(projected + offsets[from + index] - target) < 1e-6,
        `${id}/${side}: contact stays on the screen edge from its first marked frame`);
    }
  }
}
for (const id of ['nana', 'claude']) {
  const spec = resolveMotion('edge', id);
  const root = id === 'nana' ? 'public' : 'character-packs/claude/assets';
  assert.deepEqual(spec.directions, ['left', 'right']);
  assert.equal(spec.hold, true);
  for (const side of spec.directions) {
    const {settled, slideDuration} = dockEntryPlayback(spec, side);
    const style = frameStyle(spec, spec.frames - 1, id, side);
    assert.equal(style.scale, undefined, `${id}/${side}: keep the drawn scale and direction`);
    const path = root + spec.sheet.replace('{direction}', side);
    const meta = await sharp(path).metadata();
    assert.deepEqual([meta.width, meta.height], [spec.cols * cell, spec.rows * cell]);
    const [x, y] = spec.wallContact[side];
    if (id === 'nana') {
      assert.equal(slideDuration, dockEntryPlayback(edge, side).slideDuration,
        `Nana/${side}: the peek starts on the same beat as Vivian's`);
    }
    assert(settled.frames.length > 0);
    for (const frame of settled.frames) {
      const pixels = await sharp(path).extract({ left: (frame % spec.cols) * cell + x,
        top: Math.floor(frame / spec.cols) * cell + y, width: 1, height: 1 }).ensureAlpha().raw().toBuffer();
      assert(pixels[3] > 200, `${id}/${side}: the settled drawing grips at the marked point`);
      if (id === 'nana') {
        const belowSleeves = 355;
        const visibleLowerBody = await sharp(path).extract({left: frame % spec.cols * cell + (side === 'left' ? x : 0),
          top: Math.floor(frame / spec.cols) * cell + belowSleeves, width: side === 'left' ? cell - x : x,
          height: cell - belowSleeves})
          .extractChannel('alpha').raw().toBuffer();
        assert(visibleLowerBody.every(alpha => alpha < 16), `Nana/${side}: no dress or feet below the sleeves`);
      }
    }
    for (const scale of [1, 1.5, 2]) {
      const point = leanContactX(spec, id, side);
      const layout = { left: 6, top: 12, width: 288, height: 288 };
      const plan = planScreenDock({ x: 700, y: 0 }, { width: 300 * scale, height: 300 * scale },
        { ...screen, scale }, layout, side, direction => leanContactX(spec, id, direction));
      const boundary = screen[side];
      assert(Math.abs(plan.dockX + (layout.left + layout.width * point / 100) * scale - boundary) <= 1,
        `${id}/${side} @${scale}: palm reaches the monitor while the window keeps its original width`);
    }
  }
}
const custom = { ...lean, sheet: '/chibi/motion/custom/custom-lean-{direction}-sheet.webp', mirrorLeft: false,
  wallContact: undefined, palmContact: { left: [120, 254] } };
assert.equal(leanContactX(custom, 'custom', 'left'), 120 * 100 / cell, 'character packages can mark an independent left drawing');
assert.equal(leanContactX(custom, 'custom', 'right'), null, 'a mark only enables its authored direction');
console.log('Edge docking: monitor geometry, clipping, speech and character-package palm contact passed.');
