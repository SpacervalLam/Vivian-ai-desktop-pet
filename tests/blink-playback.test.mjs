import assert from 'node:assert/strict';
import sharp from 'sharp';
import {readFile} from 'node:fs/promises';
import {build} from 'esbuild';
import {createHash} from 'node:crypto';

const eyeRegions = {
 chatgpt: [[193, 170, 242, 205], [266, 159, 321, 196]],
};

function visibleBounds(pixels) {
 let left = 512, top = 512, right = 0, bottom = 0;
 for (let y = 0; y < 512; y++) {
  for (let x = 0; x < 512; x++) {
   if (pixels[(y * 512 + x) * 4 + 3] <= 32) continue;
   left = Math.min(left, x);
   top = Math.min(top, y);
   right = Math.max(right, x);
   bottom = Math.max(bottom, y);
  }
 }
 return [left, top, right, bottom];
}

function assertEyeOnlyBlink(pixels, idle, character, frame) {
 const eyeChanges = [0, 0];
 for (let y = 0; y < 512; y++) {
  for (let x = 0; x < 512; x++) {
   const i = (y * 512 + x) * 4;
   if (pixels[i + 3] !== idle[i + 3]) assert.fail(`${character} blink ${frame}: silhouette changed at ${x},${y}`);
   if (!pixels[i + 3] || (pixels[i] === idle[i] && pixels[i + 1] === idle[i + 1] && pixels[i + 2] === idle[i + 2])) continue;
   const eye = eyeRegions[character].findIndex(([left, top, right, bottom]) =>
    x >= left && x < right && y >= top && y < bottom);
   if (eye === -1) assert.fail(`${character} blink ${frame}: artwork outside the eyes changed at ${x},${y}`);
   eyeChanges[eye]++;
  }
 }
 if (frame === 0) assert.deepEqual(eyeChanges, [0, 0], `${character}: blink starts from the idle portrait`);
 else assert(eyeChanges.every(count => count > 0), `${character} blink ${frame}: both eyelids move`);
}

const manifests=await Promise.all(['deepseek','claude','chatgpt'].map(async id=>JSON.parse(await readFile(`character-packs/${id}/character.json`,'utf8'))));
const core=JSON.parse(await readFile('src/chibi/animations.json','utf8'));
manifests.push({...manifests[0],id:'custom',animations:{blink:{...manifests[0].animations.blink,cols:2,rows:1,frames:2,durations:[70,90]}}});
for(const spec of core.animations)for(const id of manifests.map(m=>m.id))assert(!spec.characterOverrides?.[id],`${id}: no imported animation configuration in the application`);
const built=await build({stdin:{contents:"import {initializeCharacterPackages} from './src/characterPackages'; await initializeCharacterPackages(); export {animation,frameStyle} from './src/chibi/motionRegistry';",resolveDir:process.cwd()},bundle:true,write:false,platform:'node',format:'esm',plugins:[{name:'package-status',setup(b){
 b.onResolve({filter:/^@tauri-apps\/api\/core$/},()=>({path:'native',namespace:'native'}));
 b.onLoad({filter:/.*/,namespace:'native'},()=>({contents:`export const invoke=async()=>({active:${JSON.stringify(manifests.map(manifest=>({manifest,root:manifest.id})))}});export const convertFileSrc=p=>p;`}));
}}]});
const {animation,frameStyle}=await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString('base64')}`);
assert.equal(animation('blink','custom').cols,2,'an unknown imported character supplies its own grid');
assert.equal(animation('blink','custom').frames,2);
assert.deepEqual(animation('blink','custom').durations,[70,90]);
for(const character of ['vivian','nana','deepseek','claude','chatgpt']){
 const spec=animation('blink',character);
 const path=spec.sheet.replaceAll('{character}',character);
 const root=['vivian','nana'].includes(character)?'public':`character-packs/${character}/assets`;
 const metadata=await sharp(await readFile(root+path)).metadata();
 const builtin=['vivian','nana'].includes(character);
 const manifest=builtin?null:JSON.parse(await readFile(`character-packs/${character}/character.json`,'utf8'));
 const idlePath=`/chibi/motion/${character}/${character}-idle-sheet.webp`;
 const idle=(eyeRegions[character] || character === 'deepseek')
  ? await sharp(root+idlePath).ensureAlpha().raw().toBuffer()
  : null;
 if(manifest)assert.equal(createHash('sha256').update(await readFile(root+path)).digest('hex'),manifest.files['assets'+path],`${character}: updated package checksum`);
 for(const [kind,file] of [['anchors','frameAnchors'],['scales','frameScales']]){
  const data=builtin?JSON.parse(await readFile(`src/chibi/${file}.json`,'utf8')):manifest[kind];
  assert.equal(data[path].length,spec.frames,`${character}: ${kind} follow compacted frame indices`);
 }
 assert.deepEqual([metadata.width,metadata.height],[spec.cols*512,spec.rows*512],`${character}: blink grid matches physical 512px cells`);
 assert(spec.frames<=spec.cols*spec.rows,`${character}: blink frames fit their grid`);
 assert(spec.frames>(spec.rows-1)*spec.cols,`${character}: no unused rows`);
 assert.equal(spec.durations.length,spec.frames,`${character}: each frame has a duration`);
 const seen=new Set();
 for(let frame=0;frame<spec.frames;frame++){
  const pixels=await sharp(root+path).extract({left:frame%spec.cols*512,top:Math.floor(frame/spec.cols)*512,width:512,height:512}).ensureAlpha().raw().toBuffer();
  for(let p=0;p<pixels.length;p+=4)if(!pixels[p+3])pixels.fill(0,p,p+3);
  if(idle){
   if (character === 'deepseek') {
    if (frame === 0) {
     for(let p=0;p<idle.length;p+=4)if(!idle[p+3])idle.fill(0,p,p+3);
     assert(pixels.equals(idle), 'DeepSeek blink starts on the idle portrait');
    }
    const bounds = visibleBounds(idle);
    assert(visibleBounds(pixels).every((edge, i) => Math.abs(edge - bounds[i]) <= 1),
     `DeepSeek blink ${frame}: full portrait stays registered to idle`);
   } else {
    assertEyeOnlyBlink(pixels,idle,character,frame);
   }
   for(const kind of ['anchors','scales'])assert.deepEqual(manifest[kind][path][frame],manifest[kind][idlePath][0]);
  }
  const hash=createHash('sha256').update(pixels).digest('hex');
  assert(!seen.has(hash),`${character}: frame ${frame} must not repeat an earlier frame`);
  seen.add(hash);
  const style=frameStyle(spec,frame,character);
  assert.equal(style.backgroundSize,`${metadata.width/512*100}% ${metadata.height/512*100}%`);
  const [x,y]=style.backgroundPosition.split(' ').map(parseFloat);
  assert(Math.abs(x/100*(metadata.width-512)-(frame%spec.cols)*512)<1e-6);
  assert(Math.abs(y/100*(metadata.height-512)-Math.floor(frame/spec.cols)*512)<1e-6);
 }
}
console.log('Blink grids, timing, registration and package hashes verified.');
