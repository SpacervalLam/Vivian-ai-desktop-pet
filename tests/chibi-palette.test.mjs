import assert from 'node:assert/strict';
import {readFile,readdir} from 'node:fs/promises';
import sharp from 'sharp';
import {lightPalette} from '../scripts/audit-chibi-palette.mjs';
const manifest=JSON.parse(await readFile('src/chibi/animations.json','utf8'));
const specs=new Map();
for(const character of manifest.characters){
 specs.set(`${character}-atlas.webp`,{character,name:'atlas',cols:3,rows:2,frames:6});
 specs.set(`motion/${character}/${character}-idle-sheet.webp`,{character,name:'idle',cols:1,rows:1,frames:1});
 for(const spec of manifest.animations){
  if(spec.characters&&!spec.characters.includes(character))continue;
  for(const direction of spec.directions??['left'])specs.set(spec.sheet.replaceAll('{character}',character).replaceAll('{direction}',direction).replace('/chibi/',''),{...spec,character});
 }
}
specs.set('motion/vivian/vivian-busy-out-sheet.webp',{character:'vivian',name:'busy-out',cols:4,rows:3,frames:12});
async function files(dir,relative=''){
 const result=[];
 for(const entry of await readdir(dir,{withFileTypes:true})){
  const file=[relative,entry.name].filter(Boolean).join('/');
  if(entry.isDirectory())result.push(...await files(`${dir}/${entry.name}`,file));
  else if(entry.name.endsWith('.webp'))result.push(file);
 }
 return result;
}
const actual=await files('public/chibi');
assert.equal(actual.length,54,'complete official resource set');
assert.deepEqual(actual.sort(),[...specs.keys()].sort(),'every shipped sheet covered by a grid contract');
const references={};
for(const character of manifest.characters)references[character]=(await lightPalette(`public/chibi/${character}-atlas.webp`,3,2,1))[0].rgb;
function detachedAlphaPixels(alpha){
 const d=new Uint16Array(alpha.length);d.fill(60000);
 for(let k=0;k<d.length;k++)if(alpha[k]>=200)d[k]=0;
 for(let y=0;y<512;y++)for(let x=0;x<512;x++){const k=y*512+x;if(x)d[k]=Math.min(d[k],d[k-1]+1);if(y)d[k]=Math.min(d[k],d[k-512]+1);}
 for(let y=511;y>=0;y--)for(let x=511;x>=0;x--){const k=y*512+x;if(x<511)d[k]=Math.min(d[k],d[k+1]+1);if(y<511)d[k]=Math.min(d[k],d[k+512]+1);}
 let count=0;for(let k=0;k<d.length;k++)if(alpha[k]>5&&alpha[k]<200&&d[k]>6)count++;
 return count;
}
let used=0,unused=0;
for(const [file,spec]of specs){
 const image=sharp(`public/chibi/${file}`),meta=await image.metadata();
 assert.deepEqual([meta.width,meta.height,meta.hasAlpha],[spec.cols*512,spec.rows*512,true],`${file}: runtime grid and alpha`);
 const {data,info}=await image.ensureAlpha().raw().toBuffer({resolveWithObject:true});
 for(let i=0;i<spec.cols*spec.rows;i++){
  let visible=0,opaque=0;const walkAlpha=spec.name==='walk'?new Uint8Array(512*512):null;
  for(let y=0;y<512;y++)for(let x=0;x<512;x++){
   const a=data[((Math.floor(i/spec.cols)*512+y)*info.width+i%spec.cols*512+x)*4+3];
   if(a)visible++;if(a>128)opaque++;if(walkAlpha)walkAlpha[y*512+x]=a;
   if(x<12||x>=500||y<12||y>=500)assert.equal(a,0,`${file}/${i}: transparent safety margin`);
  }
  if(i<spec.frames&&walkAlpha)assert.ok(detachedAlphaPixels(walkAlpha)<1500,`${file}/${i}: no detached translucent background matte`);
  if(i<spec.frames){assert.ok(opaque>3000,`${file}/${i}: populated frame`);assert.ok(visible<512*512*.8,`${file}/${i}: transparent backdrop`);used++;}
  else{assert.equal(visible,0,`${file}/${i}: unused cell fully empty`);unused++;}
 }
 const palettes=await lightPalette(`public/chibi/${file}`,spec.cols,spec.rows,spec.frames);
 for(const frame of palettes){
  assert.ok(frame.count>250,`${file}/${frame.frame}: original light costume material retained`);
  // Furniture and umbrellas change the sampled material mix; their colours are
  // reviewed visually, while unoccluded character sheets share the atlas palette.
  if(!['sleep','wake','tend','tend-out','umbrella'].includes(spec.name))for(let c=0;c<3;c++)assert.ok(Math.abs(frame.rgb[c]-references[spec.character][c])<25,`${file}/${frame.frame}: character material palette`);
 }
}
console.log(`Chibi assets: 54 grids, ${used} populated frames, ${unused} empty cells, transparent margins and character material palettes verified.`);
