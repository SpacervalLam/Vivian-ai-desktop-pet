import assert from 'node:assert/strict';
import sharp from 'sharp';
import {readFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
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
// Wake is new artwork, not a colour-only edit of the archived calibration input.
// Pin both replacement sheets and validate their full runtime frame grids separately.
const replacementSheets={
 'motion/nana/nana-wake-sheet.webp':'a88bb06433dba273f3bf33c30ba2457733d03d3691db85508f5f03921c392962',
 'motion/vivian/vivian-wake-sheet.webp':'de3e3fdfc33e4d5311c81856129e1e816b94f0403f8a48800e8b30a2a8a78efc',
};
for(const [relative,expected] of Object.entries(replacementSheets)){
 const bytes=await readFile(`public/chibi/${relative}`);
 assert.equal(createHash('sha256').update(bytes).digest('hex'),expected,`${relative}: reviewed replacement source`);
 const metadata=await sharp(bytes).metadata();
 assert.equal(metadata.width,2048);assert.equal(metadata.height,1024);assert.equal(metadata.hasAlpha,true);
 for(let frame=0;frame<8;frame++){
  const stats=await sharp(bytes).extract({left:frame%4*512,top:Math.floor(frame/4)*512,width:512,height:512}).stats();
  assert.equal(stats.channels[3].min,0,`${relative}/${frame}: transparent background`);
  assert.ok(stats.channels[3].max>0,`${relative}/${frame}: nonempty frame`);
 }
}
for(const item of report){
 if(item.path in replacementSheets)continue;
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
 assert.equal(alphaErrors,0,`${item.name}: original alpha and silhouette, including empty cells`);assert.equal(outlineErrors,0,'visible dark outline channels unchanged');
 // 已经与默认配色对齐的 sheet 再跑一次构建是幂等的（改动为 0），不能因此判失败；
 // 是否真的对齐由后面的 default-state colour match 逐帧断言。
 if(changed<=100000){
  const already=await lightPalette(file,item.cols,item.rows,item.frames);
  assert.ok(already.some(f=>f.count>1000),`${item.name}: sheet holds no correctable material`);
  for(const frame of already)assert.ok(Math.abs(luminance(frame.rgb)-luminance(reference))<6,`${item.name}/${frame.frame}: unchanged sheet must already match default colours`);
 }
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
console.log('Nana palette: 11 calibrated sheets preserve original alpha, outlines and colours; both replacement wake sheets retain reviewed sources and 8 transparent, nonempty frames.');
