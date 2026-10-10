import assert from 'node:assert/strict';
import sharp from 'sharp';
import {readFile} from 'node:fs/promises';
import {lightPalette} from '../scripts/audit-chibi-palette.mjs';
const report=JSON.parse(await readFile('assets/chibi/color-correction/build-report.json','utf8'));
const luminance=rgb=>rgb[0]*.2126+rgb[1]*.7152+rgb[2]*.0722;
const reference=(await lightPalette('public/chibi/nana-atlas.webp',3,2,1))[0].rgb;
function tones(data,mask=data){
 const channels=[[],[],[]];for(let k=0;k<data.length;k+=4)if(mask[k+3]>230&&mask[k]>120&&mask[k+1]>80&&mask[k+2]>=mask[k]&&mask[k+2]-mask[k+1]<90)for(let c=0;c<3;c++)channels[c].push(data[k+c]);
 return channels.map(v=>{v.sort((a,b)=>a-b);return [.25,.5,.75].map(q=>v[Math.floor(v.length*q)]);});
}
const defaultMeta=await sharp('public/chibi/nana-atlas.webp').metadata();
const defaultTones=tones(await sharp('public/chibi/nana-atlas.webp').extract({left:0,top:0,width:defaultMeta.width/3,height:defaultMeta.height/2}).ensureAlpha().raw().toBuffer());
for(const item of report){
 const file=`public/chibi/${item.path}`,old=`assets/chibi/color-correction/nana-${item.name}-original.webp`;
 const {data:before,info}=await sharp(old).ensureAlpha().raw().toBuffer({resolveWithObject:true}),after=await sharp(file).ensureAlpha().raw().toBuffer();
 assert.equal(after.length,before.length);const meta=await sharp(file).metadata();assert.equal(meta.width,item.cols*512);assert.equal(meta.height,item.rows*512);
 let alphaErrors=0,outlineErrors=0,changed=0;
 for(let k=0;k<before.length;k+=4){
  if(after[k+3]!==before[k+3])alphaErrors++;
  for(let c=0;c<3;c++){
   if(before[k+3]&&before[k+c]<=96&&after[k+c]!==before[k+c])outlineErrors++;
   if(after[k+c]!==before[k+c])changed++;
  }
 }
 assert.equal(alphaErrors,0,`${item.name}: original alpha and silhouette, including empty cells`);assert.equal(outlineErrors,0,'visible dark outline channels unchanged');assert.ok(changed>100000,'colour correction is present');
 if(item.toneMatch)for(let frame=0;frame<item.frames;frame++){
  const a=Buffer.alloc(512*512*4),b=Buffer.alloc(a.length);
  for(let y=0;y<512;y++){const start=((Math.floor(frame/item.cols)*512+y)*info.width+frame%item.cols*512)*4;before.copy(a,y*512*4,start,start+512*4);after.copy(b,y*512*4,start,start+512*4);}
  const measured=tones(b,a); // Same material pixels before/after; no shifting colour-selection bias.
  for(let c=0;c<3;c++)for(let q=0;q<3;q++)assert.ok(Math.abs(measured[c][q]-defaultTones[c][q])<=1,`${item.name}/${frame}: shadows, midtones and highlights match default`);
 }
 for(const frame of item.preserve)for(let y=0;y<512;y++){
  const start=((Math.floor(frame/item.cols)*512+y)*info.width+frame%item.cols*512)*4;
  const a=Buffer.from(after.subarray(start,start+512*4)),b=Buffer.from(before.subarray(start,start+512*4));
  // WebP can discard invisible RGB; compare the full visible RGBA and alpha everywhere.
  for(let k=0;k<a.length;k+=4)if(!a[k+3]){a[k]=a[k+1]=a[k+2]=0;b[k]=b[k+1]=b[k+2]=0;}
  assert.deepEqual(a,b,`${item.name}/${frame}: original visible endpoint pixels`);
 }
 const frames=await lightPalette(file,item.cols,item.rows,item.frames),values=frames.map(f=>luminance(f.rgb));
 assert.ok(Math.max(...values)-Math.min(...values)<8,`${item.name}: frame palette continuity`);
 for(const frame of frames)assert.ok(Math.abs(luminance(frame.rgb)-luminance(reference))<6,`${item.name}/${frame.frame}: default-state colour match`);
}
assert.equal(report.length,12);
for(const name of ['wake','busy-in','busy-loop','cast','tend-out','tend','tired'])assert.ok(report.some(r=>r.name===name),'all requested sheets are covered');
console.log('Nana palette: 12 sheets match default colours; original alpha, silhouettes, dimensions, endpoint pixels and dark outlines preserved.');
