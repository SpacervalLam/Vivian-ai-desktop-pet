import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { createHash } from 'node:crypto';
import sharp from 'sharp';
import { build } from 'esbuild';
import { releaseArtwork } from '../scripts/compress-chibi.mjs';

const ids=['vivian','nana','deepseek','claude','chatgpt'];
const packs=await Promise.all(ids.slice(2).map(async id=>({manifest:JSON.parse(await readFile(`character-packs/${id}/character.json`,'utf8')),root:resolve('character-packs',id)})));
const bundle=await build({stdin:{contents:`import {initializeCharacterPackages} from './src/characterPackages';await initializeCharacterPackages();export * from './src/chibi/motionRegistry';`,resolveDir:process.cwd()},bundle:true,write:false,platform:'node',format:'esm',plugins:[{name:'packages',setup(b){
 b.onResolve({filter:/^@tauri-apps\/api\/core$/},()=>({path:'native',namespace:'native'}));
 b.onLoad({filter:/.*/,namespace:'native'},()=>({contents:`export const invoke=async()=>(${JSON.stringify({active:packs})});export const convertFileSrc=p=>p;`}));
}}]});
const registry=await import('data:text/javascript;base64,'+Buffer.from(bundle.outputFiles[0].text).toString('base64'));
const hash=bytes=>createHash('sha256').update(bytes).digest('hex');
for(const id of ids)for(const name of ['walk','turn']){
 const spec=registry.resolveMotion(name,id);
 assert.equal(spec.mirrorRight,false,'asymmetric characters must not be reflected');
 const left=registry.sheetUrl(spec,id,'left'),right=registry.sheetUrl(spec,id,'right');
 assert.notEqual(left,right);
 assert.match(left,/-left-sheet\.webp$/);assert.match(right,/-right-sheet\.webp$/);
 const source=direction=>ids.indexOf(id)<2?'public'+registry.sheetUrl(spec,id,direction):registry.sheetUrl(spec,id,direction);
 if(name==='turn'){
  assert.deepEqual([spec.cols,spec.rows,spec.frames],[4,2,8],`${id}: compact eight-frame turn`);
  assert.equal(spec.durations.length,8);
  assert(spec.durations.reduce((sum,ms)=>sum+ms,0)<=500,`${id}: turn completes within 0.5s`);
  for(const direction of ['left','right']){
   const meta=await sharp(source(direction)).metadata();
   assert.deepEqual([meta.width,meta.height],[2048,1024],`${id}/${direction}: eight physical cells`);
   const key=`/chibi/motion/${id}/${id}-turn-${direction}-sheet.webp`;
   if(ids.indexOf(id)>=2)for(const kind of ['anchors','scales'])assert.equal(packs.find(p=>p.manifest.id===id).manifest[kind][key].length,8);
  }
 }
 const leftPixels=await sharp(source('left')).ensureAlpha().raw().toBuffer();
 const rightPixels=await sharp(source('right')).ensureAlpha().raw().toBuffer();
 for(let frame=0;frame<spec.frames;frame++){
  if(name==='turn'&&frame===0)continue; // Both directions intentionally start at the same front idle.
  const a=Buffer.alloc(512*512*4),b=Buffer.alloc(512*512*4);
  for(let y=0;y<512;y++){
   const at=((Math.floor(frame/spec.cols)*512+y)*spec.cols*512+frame%spec.cols*512)*4;
   leftPixels.copy(a,y*2048,at,at+2048);rightPixels.copy(b,y*2048,at,at+2048);
  }
  const mirrored=await sharp(a,{raw:{width:512,height:512,channels:4}}).flop().raw().toBuffer();
  // Compare visible RGBA only; encoder noise beneath alpha zero is immaterial.
  for(const cell of [mirrored,b])for(let k=0;k<cell.length;k+=4)if(!cell[k+3])cell.fill(0,k,k+4);
  assert.notEqual(hash(mirrored),hash(b),`${id}/${name}/${frame}: independently drawn, not a mirrored left cell`);
 }
 for(const direction of ['left','right'])for(let frame=0;frame<spec.frames;frame++){
  const style=registry.frameStyle(spec,frame,id,direction);
  assert(Number(style.scale)>0,`${id}/${name}/${direction}: no negative horizontal scale`);
  assert(style.backgroundImage.includes(`-${direction}-sheet.webp`));
 }
 if(ids.indexOf(id)<2)for(const direction of ['left','right'])assert(releaseArtwork.includes(`${name==='walk'?'walk':'motion'}/${id}/${id}-${name}-${direction}-sheet.webp`));
 else assert(packs.find(p=>p.manifest.id===id).manifest.independent_directions);
}
console.log('Five characters: independent left/right turn and walk sheets, positive transforms, distinct art and release coverage passed.');
