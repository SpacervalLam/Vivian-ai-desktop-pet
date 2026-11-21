import sharp from 'sharp';
import { readFile, writeFile, mkdir, access } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve, dirname } from 'node:path';
import { createHash } from 'node:crypto';
import { bounds, faceBounds, cleanMatte, headCenter } from './registration.mjs';
import { registerEntryFrames } from './register-entry-frames.mjs';

const root=fileURLToPath(new URL('../../../',import.meta.url));
const vocab=JSON.parse(await readFile(resolve(root,'src/chibi/animations.json'),'utf8'));
const anchorFile=resolve(root,'src/chibi/frameAnchors.json');
const anchors=JSON.parse(await readFile(anchorFile,'utf8'));
const flags=process.argv.slice(2),only=flags.find(s=>s.startsWith('--only='))?.slice(7);
const ids=flags.filter(s=>!s.startsWith('--'));
const references=new Map(),report=[];
const neutralEntries=new Set(['float','turn','cast','happy','angry','thinking','think','smug','sleep','busy-in','remind','tired','music','look']);
// Claude's authored sleep rows shrink as she lies down; they are not an
// equal-height grid. Boundaries sit in the transparent gutters between poses.
const sourceRowEdges={'claude/sleep':[0,288,569,852,1115,1344,1536]};

async function sourceCells(id,name,spec){
 let file=resolve(root,`assets/chibi/frame-sheets/${id}/${name}-v5.png`);
 try{await access(file);}catch{
  file=resolve(root,`assets/chibi/frame-sheets/${id}/${name}-v4.png`);
  if(['clipboard','atlas'].includes(name))try{await access(file);}catch{
   const original=name==='atlas'?`public/chibi/${id}-atlas.webp`:`public/chibi/motion/${id}/${id}-clipboard-sheet.webp`;
   await sharp(await readFile(resolve(root,original))).png().toFile(file);
  }
 }
 const{data,info}=await sharp(file).ensureAlpha().raw().toBuffer({resolveWithObject:true});
 const width=Math.floor(info.width/spec.cols),cells=[];
 const rowEdges=sourceRowEdges[`${id}/${name}`]??Array.from({length:spec.rows+1},(_,i)=>Math.round(i*info.height/spec.rows));
 if(rowEdges.length!==spec.rows+1||rowEdges.at(-1)!==info.height)throw new Error(`${id}/${name}: source row layout does not match image`);
 for(let i=0;i<spec.frames;i++){
  const row=Math.floor(i/spec.cols),top=rowEdges[row],height=rowEdges[row+1]-top;
  const pixels=Buffer.alloc(width*height*4),left=Math.round(i%spec.cols*info.width/spec.cols);
  for(let y=0;y<height;y++)data.copy(pixels,y*width*4,((top+y)*info.width+left)*4,((top+y)*info.width+left+width)*4);
  cleanMatte(pixels,width,height);
  const box=bounds(pixels,width,height);
  if(box.height<20)throw new Error(`${id}/${name}/${i}: empty source frame`);
  const previous=name==='wake'&&i===spec.frames-1?undefined:cells.at(-1)?.face;
  const face=faceBounds(pixels,width,height,box,previous);
  cells.push({pixels,width,height,box,face,headX:headCenter(pixels,width,height,face,box)});
 }
 return cells;
}

async function reference(id){
 if(!references.has(id)){
  const spec=vocab.animations.find(s=>s.name==='blink'),first=(await sourceCells(id,'blink',spec))[0];
  const scale=420/first.box.height;
  references.set(id,{faceWidth:first.face.width*scale,chin:480-(first.box.y1-first.face.y1)*scale});
 }
 return references.get(id);
}

async function exportSheet(id,spec,direction){
 const name=spec.directions?`${spec.name}-${direction}`:spec.name==='thinking'?'think':spec.name;
 const cells=await sourceCells(id,name,spec),ref=await reference(id);
 const from=cells[spec.name==='wake'?cells.length-1:0];
 const contextual=['sleep','wake'].includes(spec.name),profile=spec.name==='walk';
 // Calibrate against the same resting face and foot line, rather than each
 // sheet's changing outline (which includes umbrellas, hair and spell effects).
 const scale=profile||(id==='claude'&&spec.name==='sleep')?420/from.box.height:ref.faceWidth/from.face.width;
 const bodyScale=contextual||profile?scale:(480-ref.chin)/Math.max(1,from.box.y1-from.face.y1);
 const frames=[],hashes=new Set();
 for(const [i,cell]of cells.entries()){
  const{pixels,width,height,box,face}=cell;
  const split=Math.min(box.y1,face.y1);
  const upperHeight=split-box.y0+1,lowerHeight=box.y1-split;
  const outWidth=Math.round(box.width*scale),headHeight=Math.round(upperHeight*scale),bodyHeight=Math.round(lowerHeight*bodyScale);
  const center=contextual&&box.height<from.box.height*.8?(box.x0+box.x1)/2:cell.headX;
  const projectedLeft=Math.round(256+(box.x0-center)*scale);
  const left=Math.max(12,Math.min(500-outWidth,projectedLeft));
  const chin=spec.name==='float'?ref.chin+(split-from.face.y1)*scale:480-bodyHeight;
  const top=Math.max(12,Math.min(500-headHeight-bodyHeight,Math.round(chin-headHeight+1)));
  if(left<12||left+outWidth>500||top<12||top+headHeight+bodyHeight>500)throw new Error(`${id}/${name}/${i}: registered drawing exceeds safe viewport (${left},${top},${outWidth},${headHeight+bodyHeight})`);
  const raw={raw:{width,height,channels:4}};
  const layers=[{input:await sharp(pixels,raw).extract({left:box.x0,top:box.y0,width:box.width,height:upperHeight}).resize(outWidth,headHeight).png().toBuffer(),left,top}];
  if(lowerHeight>0)layers.push({input:await sharp(pixels,raw).extract({left:box.x0,top:split+1,width:box.width,height:lowerHeight}).resize(outWidth,bodyHeight).png().toBuffer(),left,top:top+headHeight});
  const output=await sharp({create:{width:512,height:512,channels:4,background:'#00000000'}}).composite(layers).png().toBuffer();
  hashes.add(createHash('sha256').update(output).digest('hex'));frames.push(output);
  report.push({id,name,frame:i,faceWidth:face.width*scale,chin,foot:top+headHeight+bodyHeight-1,headX:256+(cell.headX-center)*scale+left-projectedLeft,scale,bodyScale});
 }
 if(hashes.size!==spec.frames)throw new Error(`${id}/${name}: duplicated playback frames`);
 // Shared rest is a real entry keyframe, not repeated exposure padding. Keep
 // loop-only work/umbrella and waking sequences in their own starting states.
 if(neutralEntries.has(spec.name))frames[0]=await readFile(resolve(root,`public/chibi/motion/${id}/${id}-idle-sheet.webp`));
 if(spec.name==='blink')frames[frames.length-1]=frames[0];
 const url=spec.sheet.replaceAll('{character}',id).replaceAll('{direction}',direction),target=resolve(root,`public${url}`);
 await mkdir(dirname(target),{recursive:true});
 if(name==='atlas')frames[0]=await readFile(resolve(root,`public/chibi/motion/${id}/${id}-idle-sheet.webp`));
 const encoded=await sharp({create:{width:spec.cols*512,height:spec.rows*512,channels:4,background:'#00000000'}})
  .composite(frames.map((input,i)=>({input,left:i%spec.cols*512,top:Math.floor(i/spec.cols)*512}))).webp({lossless:true,effort:4}).toBuffer();
 await writeFile(target,encoded);
 anchors[url]=frames.map(()=>[0,0]);
 if(spec.name==='blink'){
  const idle=`/chibi/motion/${id}/${id}-idle-sheet.webp`;
  await sharp(frames[0]).webp({lossless:true,effort:4}).toFile(resolve(root,`public${idle}`));anchors[idle]=[[0,0]];
 }
 console.log(`${id}/${name}: registered ${frames.length} playback frames against idle.`);
}

for(const id of ids.length?ids:['deepseek','claude']){
 if(!['deepseek','claude'].includes(id))throw new Error(`Unknown character: ${id}`);
 const seen=new Set();
 if(!only){await exportSheet(id,vocab.animations.find(s=>s.name==='blink'),'left');seen.add('blink');}
 for(const spec of vocab.animations.filter(s=>!s.characters||s.characters.includes(id)))for(const direction of spec.directions??['left']){
  const name=spec.directions?`${spec.name}-${direction}`:spec.name==='thinking'?'think':spec.name;
  if(seen.has(name)||(only&&name!==only))continue;seen.add(name);await exportSheet(id,spec,direction);
 }
 if(!only||only==='atlas')await exportSheet(id,{name:'atlas',cols:3,rows:2,frames:6,sheet:'/chibi/{character}-atlas.webp'},'left');
}
await writeFile(anchorFile,JSON.stringify(anchors,null,2)+'\n');
await registerEntryFrames(ids.length?ids:['deepseek','claude']);
await mkdir(resolve(root,'tmp/chibi-registration'),{recursive:true});
await writeFile(resolve(root,'tmp/chibi-registration/metrics.json'),JSON.stringify(report,null,2)+'\n');
