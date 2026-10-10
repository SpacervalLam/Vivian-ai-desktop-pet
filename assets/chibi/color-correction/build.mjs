// Pack tool-generated colour edits. Preserve original endpoint poses and unused cells exactly.
import sharp from 'sharp';
import {readFile,writeFile,mkdir,copyFile} from 'node:fs/promises';
import {constants} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {lightPalette} from '../../../scripts/audit-chibi-palette.mjs';
const root=new URL('./',import.meta.url),base=new URL('../../../',import.meta.url);
const vocabulary=JSON.parse(await readFile(new URL('src/chibi/animations.json',base),'utf8'));
const additional=['wake','busy-in','busy-loop','cast','tend-out','tend','tired'];
const targets=[
 {name:'walk-left',cols:4,rows:4,frames:14,preserve:[0,14,15],path:'walk/nana/nana-walk-left-sheet.webp'},
 {name:'walk-right',cols:4,rows:4,frames:14,preserve:[0,14,15],path:'walk/nana/nana-walk-right-sheet.webp'},
 ...['angry','think','smug'].map(name=>({name,cols:4,rows:3,frames:12,preserve:[0,11],path:`motion/nana/nana-${name}-sheet.webp`})),
 ...additional.map(name=>{
  const spec=vocabulary.animations.find(s=>s.name===name);
  return {name,cols:spec.cols,rows:spec.rows,frames:spec.frames,preserve:Array.from({length:spec.cols*spec.rows-spec.frames},(_,i)=>i+spec.frames),path:`motion/nana/nana-${name}-sheet.webp`,toneMatch:true};
 }),
];
const report=[];
const reference=(await lightPalette(fileURLToPath(new URL('public/chibi/nana-atlas.webp',base)),3,2,1))[0].rgb;
function materialTones(data,mask=data){
 const values=[[],[],[]];
 for(let k=0;k<data.length;k+=4)if(mask[k+3]>230&&mask[k]>120&&mask[k+1]>80&&mask[k+2]>=mask[k]&&mask[k+2]-mask[k+1]<90)for(let c=0;c<3;c++)values[c].push(data[k+c]);
 return values.map(v=>{if(v.length<100)throw new Error('Insufficient opaque palette samples');v.sort((a,b)=>a-b);return [.25,.5,.75].map(q=>v[Math.floor(v.length*q)]);});
}
const defaultMeta=await sharp(fileURLToPath(new URL('public/chibi/nana-atlas.webp',base))).metadata();
const defaultPixels=await sharp(fileURLToPath(new URL('public/chibi/nana-atlas.webp',base))).extract({left:0,top:0,width:defaultMeta.width/3,height:defaultMeta.height/2}).ensureAlpha().raw().toBuffer();
const referenceTones=materialTones(defaultPixels);
function transferTones(original){
 const sourceTones=materialTones(original),result=Buffer.from(original);
 const tables=sourceTones.map((tones,c)=>{
  const points=[[0,0],[96,96],...tones.map((v,i)=>[v,referenceTones[c][i]]),[255,255]];
  // Duplicate source quantiles can occur on flat shading; combine rather than divide by zero.
  const unique=[];for(const point of points){if(unique.at(-1)?.[0]===point[0])unique[unique.length-1]=point;else unique.push(point);}
  return Array.from({length:256},(_,v)=>{
   if(v<=96)return v;
   const index=unique.findIndex(p=>p[0]>=v),b=unique[index],a=unique[index-1];
   return Math.round(a[1]+(v-a[0])*(b[1]-a[1])/(b[0]-a[0]));
  });
 });
 for(let k=0;k<result.length;k+=4)if(result[k+3])for(let c=0;c<3;c++)result[k+c]=tables[c][original[k+c]];
 return {data:result,sourceTones,referenceTones,actualTones:materialTones(result,original),actual:meanLight(result)};
}
function meanLight(data){
 const sums=[0,0,0];let count=0;
 for(let k=0;k<data.length;k+=4){const r=data[k],g=data[k+1],b=data[k+2];if(data[k+3]>230&&r>140&&g>125&&b>=r&&b-g<65){count++;sums[0]+=r;sums[1]+=g;sums[2]+=b;}}
 return sums.map(v=>v/count);
}
function transfer(original,target){
 const source=meanLight(original),anchors=[...target];
 const transform=()=>{
  const tables=source.map((mid,c)=>Array.from({length:256},(_,v)=>Math.round(v<=96?v:v<=mid?96+(v-96)*(anchors[c]-96)/(mid-96):anchors[c]+(v-mid)*(255-anchors[c])/(255-mid))));
  const result=Buffer.from(original);
  for(let k=0;k<result.length;k+=4)if(result[k+3])for(let c=0;c<3;c++)result[k+c]=tables[c][original[k+c]];
  return result;
 };
 let result;
 for(let iteration=0;iteration<12;iteration++){
  result=transform();const actual=meanLight(result);
  if(actual.every((v,c)=>Math.abs(v-target[c])<.4))break;
  for(let c=0;c<3;c++)anchors[c]=Math.max(source[c],Math.min(254,target[c]+12,anchors[c]+(target[c]-actual[c])*1.2));
 }
 return {data:result,source,anchors,actual:meanLight(result)};
}
for(const t of targets){
 if(process.argv.includes('--walk-only')&&!t.name.startsWith('walk'))continue;
 const destination=fileURLToPath(new URL(`public/chibi/${t.path}`,base));
 const original=fileURLToPath(new URL(`nana-${t.name}-original.webp`,root)),source=fileURLToPath(new URL(`nana-${t.toneMatch?'walk-left':t.name}-color-source.png`,root));
 await copyFile(destination,original,constants.COPYFILE_EXCL).catch(error=>{if(error.code!=='EEXIST')throw error;});
 const meta=await sharp(source).metadata(),width=t.cols*512,height=t.rows*512;
 const guideGrid=t.toneMatch?{cols:4,rows:4,frames:14}:t;
 if(Math.abs(meta.width/meta.height-guideGrid.cols/guideGrid.rows)>.04)throw new Error(`Generated canvas ratio changed: ${t.name}`);
 // Generated edits supply a palette reference only. Transfer its palette onto the original
 // pixel geometry rather than using redrawn silhouettes. Clamp to the default-state palette.
 const guide=await lightPalette(source,guideGrid.cols,guideGrid.rows,guideGrid.frames).catch(async()=>{
  const buffer=await sharp(source).resize(guideGrid.cols*512,guideGrid.rows*512).png().toBuffer();
  return lightPalette(buffer,guideGrid.cols,guideGrid.rows,guideGrid.frames);
 });
 const guideMean=reference.map((_,c)=>guide.reduce((sum,f)=>sum+f.rgb[c],0)/guide.length);
 const target=reference.map((v,c)=>Math.max(v-1,Math.min(v+1,guideMean[c])));
 const before=await sharp(original).ensureAlpha().raw().toBuffer(),resized=Buffer.from(before),metrics=[];
 for(let i=0;i<t.cols*t.rows;i++){
  if(t.preserve.includes(i)){
   for(let y=0;y<512;y++){const start=((Math.floor(i/t.cols)*512+y)*width+i%t.cols*512)*4;before.copy(resized,start,start,start+512*4);}
   continue;
  }
  const cell=Buffer.alloc(512*512*4);
  for(let y=0;y<512;y++){const start=((Math.floor(i/t.cols)*512+y)*width+i%t.cols*512)*4;before.copy(cell,y*512*4,start,start+512*4);}
  const corrected=t.toneMatch?transferTones(cell):transfer(cell,target);
  for(let y=0;y<512;y++){const start=((Math.floor(i/t.cols)*512+y)*width+i%t.cols*512)*4;corrected.data.copy(resized,start,y*512*4,(y+1)*512*4);}
  metrics.push({frame:i,source:corrected.source,anchors:corrected.anchors,sourceTones:corrected.sourceTones,referenceTones:corrected.referenceTones,actualTones:corrected.actualTones,actual:corrected.actual,alphaUnchanged:true});
 }
 await sharp(resized,{raw:{width,height,channels:4}}).webp({lossless:true,effort:4}).toFile(destination);
 const palette=await lightPalette(destination,t.cols,t.rows,t.frames);
 report.push({...t,reference,guideMean,target,metrics,beforePalette:await lightPalette(original,t.cols,t.rows,t.frames),palette});
 console.log(`Packed ${t.name}: ${metrics.length} colour-corrected frames; endpoint cells preserved`);
}
await mkdir(new URL('previews/',root),{recursive:true});
const walk=targets[0],file=fileURLToPath(new URL(`public/chibi/${walk.path}`,base)),original=fileURLToPath(new URL('nana-walk-left-original.webp',root));
const cells=[];
for(let i=0;i<14;i++){
 const region={left:i%4*512,top:Math.floor(i/4)*512,width:512,height:512};
 cells.push(await sharp({create:{width:512,height:256,channels:4,background:'#f6f0e9'}}).composite(await Promise.all([original,file].map(async(p,j)=>({input:await sharp(p).extract(region).resize(256,256).png().toBuffer(),left:j*256,top:0})))).raw().toBuffer());
}
await sharp(Buffer.concat(cells),{raw:{width:512,height:256*14,channels:4,pageHeight:256}}).gif({loop:0,delay:[90,82,76,72,68,84,78,72,82,82,72,68,88,76]}).toFile(fileURLToPath(new URL('previews/nana-walk-before-after.gif',root)));
await writeFile(new URL(process.argv.includes('--walk-only')?'walk-build-report.json':'build-report.json',root),JSON.stringify(report,null,2)+'\n');
