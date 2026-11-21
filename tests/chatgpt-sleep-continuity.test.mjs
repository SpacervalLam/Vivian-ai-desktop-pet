import assert from 'node:assert/strict';
import {build} from 'esbuild';
import sharp from 'sharp';
import {readFile} from 'node:fs/promises';
const manifest=JSON.parse(await readFile('character-packs/chatgpt/character.json','utf8'));
const result=await build({stdin:{contents:"import {initializeCharacterPackages} from './src/characterPackages';await initializeCharacterPackages();export * from './src/chibi/motionRegistry';",resolveDir:process.cwd()},bundle:true,write:false,format:'esm',platform:'node',logLevel:'silent',plugins:[{name:'installed',setup(b){b.onResolve({filter:/^@tauri-apps\/api\/core$/},()=>({path:'native',namespace:'native'}));b.onLoad({filter:/.*/,namespace:'native'},()=>({contents:'export const invoke=async()=>('+JSON.stringify({active:[{manifest,root:'character-packs/chatgpt'}]})+');export const convertFileSrc=p=>p;'}));}}]});
const registry=await import('data:text/javascript;base64,'+Buffer.from(result.outputFiles[0].text).toString('base64'));
const cell=async(name,i)=>sharp(`character-packs/chatgpt/assets/chibi/motion/chatgpt/chatgpt-${name}-sheet.webp`).extract({left:i%4*512,top:Math.floor(i/4)*512,width:512,height:512}).ensureAlpha().raw().toBuffer();
for(const name of ['sleep','wake']){
 for(const spec of [registry.animation(name,'chatgpt'),registry.resolveMotion(name,'chatgpt')]){
  assert.equal(spec.frames,12);assert.equal(spec.rows,3);assert.equal(spec.durations.length,12);
 }
 assert.equal(registry.animation(name,'vivian').frames,name==='sleep'?24:16);
}
const idle=await sharp('character-packs/chatgpt/assets/chibi/chatgpt-atlas.webp').extract({left:0,top:0,width:512,height:512}).ensureAlpha().raw().toBuffer();
assert.deepEqual(await cell('sleep',0),idle);
for(let i=0;i<12;i++){
 const a=await cell('sleep',i);assert.deepEqual(a,await cell('wake',11-i));
 for(let k=0;k<a.length;k+=4)if(a[k+3]===0)assert.equal(a[k]|a[k+1]|a[k+2],0);
 if(i>0)assert.notDeepEqual(a,await cell('sleep',i-1),'sleep has no duplicated adjacent holds');
}
console.log('ChatGPT sleep: compact grid, distinct frames, clean transparency, exact idle and wake continuity passed.');
