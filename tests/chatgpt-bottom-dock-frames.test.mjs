import assert from 'node:assert/strict';
import {build} from 'esbuild';
import sharp from 'sharp';
import {readFile} from 'node:fs/promises';
import { bounds } from '../scripts/chibi/frame-sheets/registration.mjs';
const manifests=await Promise.all(['deepseek','claude','chatgpt'].map(async id=>JSON.parse(await readFile(`character-packs/${id}/character.json`,'utf8'))));
const bundle=await build({stdin:{contents:"import {initializeCharacterPackages} from './src/characterPackages';await initializeCharacterPackages();export * from './src/chibi/motionRegistry';export {bottomDockStyle, BOTTOM_DOCK_ORIGIN_PERCENT} from './src/chibi/dockSize';",resolveDir:process.cwd()},bundle:true,write:false,format:'esm',platform:'node',logLevel:'silent',plugins:[{name:'installed',setup(b){b.onResolve({filter:/^@tauri-apps\/api\/core$/},()=>({path:'native',namespace:'native'}));b.onLoad({filter:/.*/,namespace:'native'},()=>({contents:`export const invoke=async()=>({active:${JSON.stringify(manifests.map(manifest=>({manifest,root:manifest.id})))}});export const convertFileSrc=p=>p;`}));}}]});
const registry=await import('data:text/javascript;base64,'+Buffer.from(bundle.outputFiles[0].text).toString('base64'));
const spec=registry.animation('edge-bottom','chatgpt');
assert.equal(spec.frames,13);
assert.equal(spec.durations.length,13);
assert(spec.durations.at(-1) > Math.max(...spec.durations.slice(0, -1)), 'the final pose holds longer than the approach frames');
assert.equal(spec.durations.reduce((total, ms) => total + ms, 0), 1180, 'the shortened sequence preserves the original beat');
assert.equal(registry.resolveMotion('edge-bottom','chatgpt').frames,13);
for (const id of ['vivian', 'nana']) {
  const motion = registry.resolveMotion('edge-bottom', id);
  assert.equal(motion.frames, 13, `${id}: hold the full-head drawing on the last frame`);
  assert.equal(motion.durations.length, motion.frames);
  assert.equal(motion.durations.reduce((total, ms) => total + ms, 0), 1180);
}
assert.equal(registry.animation('edge-bottom','deepseek').frames,16);
const {data}=await sharp('character-packs/chatgpt/assets/chibi/motion/chatgpt/chatgpt-edge-bottom-sheet.webp').ensureAlpha().raw().toBuffer({resolveWithObject:true});
let finalVisible=0;
for(let y=1536;y<2048;y++)for(let x=0;x<2048;x++){
 const alpha=data[(y*2048+x)*4+3];
 if(x<512)finalVisible+=alpha>0?1:0;else assert.equal(alpha,0);
}
assert(finalVisible>1000);

function finalCropRow(id) {
  const motion = registry.resolveMotion('edge-bottom', id);
  const frame = registry.frameStyle(motion, motion.frames - 1, id);
  const wrapper = registry.bottomDockStyle(id);
  const origin = registry.BOTTOM_DOCK_ORIGIN_PERCENT;
  const yOffset = style => Number.parseFloat(style.translate?.split(' ')[1] ?? '0');
  const insideWrapper = origin + (50 - origin - yOffset(wrapper)) / Number(wrapper.scale);
  const frameOrigin = Number.parseFloat(frame.transformOrigin?.split(' ')[1] ?? '90');
  return (frameOrigin + (insideWrapper - frameOrigin - yOffset(frame)) / Number(frame.scale ?? 1)) * 5.12;
}

const deepseekCrop = finalCropRow('deepseek');
assert(deepseekCrop >= 478 && deepseekCrop <= 484, 'DeepSeek: the screen edge meets the base of both painted hands');
const cell = 512;
async function packageHoldFrame(id) {
  const motion = registry.resolveMotion('edge-bottom', id);
  const last = motion.frames - 1;
  return sharp(`character-packs/${id}/assets${motion.sheet}`)
    .extract({ left: last % motion.cols * cell, top: Math.floor(last / motion.cols) * cell, width: cell, height: cell })
    .ensureAlpha().raw().toBuffer();
}

const deepseekHold = await packageHoldFrame('deepseek');
const deepseekTop = bounds(deepseekHold, cell, cell).y0;
assert(deepseekCrop > deepseekTop, 'DeepSeek: the halo stays inside the crop');
for (const { id, presentation } of manifests) {
  assert.equal(Number(registry.bottomDockStyle(id).scale), presentation.defaultSize.scale,
    `${id}: bottom docking keeps the standing scale`);
}
const claudeCrop = finalCropRow('claude');
const claudeHold = await packageHoldFrame('claude');
const claudeAlpha = (x, y) => claudeHold[(y * cell + x) * 4 + 3];
const lastVisibleRow = Math.ceil(claudeCrop) - 1;
for (const x of [185, 256, 325]) {
  assert(claudeAlpha(x, lastVisibleRow) > 200, 'Claude: both hands and the face reach the screen edge');
}
assert(claudeCrop >= 249 && claudeCrop <= 253, 'Claude: the crop keeps the drawn fingers visible');
assert(claudeAlpha(256, lastVisibleRow + 8) < 8, 'Claude: the final drawing ends at the grip pose');
console.log('Bottom dock: ChatGPT hold, DeepSeek hand visibility and Claude redraw verified.');
