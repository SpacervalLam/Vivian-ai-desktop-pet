import sharp from 'sharp';
import {readFile,writeFile,rename,rm} from 'node:fs/promises';
import {resolve} from 'node:path';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {bounds,faceBounds} from './registration.mjs';

const root=fileURLToPath(new URL('../../../',import.meta.url));
const readJson=async path=>JSON.parse(await readFile(resolve(root,path),'utf8'));
async function save(path,data){
 const temporary=`${path}.${process.pid}.tmp`;
 try{
  await writeFile(temporary,data);
  for(let attempt=0;;attempt++)try{await rename(temporary,path);break;}catch(error){
   if(attempt===4)throw error;
   await new Promise(done=>setTimeout(done,100*(attempt+1)));
  }
 }finally{await rm(temporary,{force:true});}
}

// Copy both the drawing and its display calibration. Copying only pixels leaves
// Nana's per-frame scale/anchor on the replacement and still causes a size jump.
export async function registerEntryFrames(ids){
 const vocab=await readJson('src/chibi/animations.json'),entries=await readJson('src/chibi/entryStates.json');
 const anchors=await readJson('src/chibi/frameAnchors.json'),scales=await readJson('src/chibi/frameScales.json');
 const sheets=new Map();
 async function sheet(url,cols,rows,frames){
  if(!sheets.has(url))sheets.set(url,{url,cols,rows,frames,input:await readFile(resolve(root,'public'+url)),replacements:new Map(),cells:new Map()});
  return sheets.get(url);
 }
 async function cell(s,i){
  if(s.replacements.has(i))return s.replacements.get(i);
  if(!s.cells.has(i))s.cells.set(i,await sharp(s.input).extract({left:i%s.cols*512,top:Math.floor(i/s.cols)*512,width:512,height:512}).png().toBuffer());
  return {input:s.cells.get(i),anchor:anchors[s.url]?.[i]??[0,0],scale:scales[s.url]?.[i]??[1,90]};
 }
 async function sameDrawing(a,b){
  if(a.equals(b))return true;
  const [x,y]=await Promise.all([sharp(a).ensureAlpha().raw().toBuffer(),sharp(b).ensureAlpha().raw().toBuffer()]);
  for(let k=0;k<x.length;k+=4){
   if(x[k+3]!==y[k+3])return false;
   // libvips' alpha compositing can round a visible channel by one unit.
   if(x[k+3]&&[0,1,2].some(c=>Math.abs(x[k+c]-y[k+c])>1))return false;
  }
  return true;
 }
 async function replace(target,index,source,sourceIndex){
  const next=await cell(source,sourceIndex),current=await cell(target,index);
  if(JSON.stringify(current.anchor)===JSON.stringify(next.anchor)&&JSON.stringify(current.scale)===JSON.stringify(next.scale)&&await sameDrawing(current.input,next.input))return;
  target.replacements.set(index,next);
 }
 for(const id of ids??vocab.characters){
  const available=vocab.animations.filter(s=>!s.characters||s.characters.includes(id));
  const get=async(spec,direction='left')=>sheet(spec.sheet.replaceAll('{character}',id).replaceAll('{direction}',direction),spec.cols,spec.rows,spec.frames);
  const atlas=await sheet(`/chibi/${id}-atlas.webp`,3,2,6);
  const idle=id==='nana'?atlas:await sheet(`/chibi/motion/${id}/${id}-idle-sheet.webp`,1,1,1);
  async function landmarks(s){
   const {input,anchor,scale}=await cell(s,0);
   const pixels=await sharp(input).ensureAlpha().raw().toBuffer(),box=bounds(pixels,512,512),face=faceBounds(pixels,512,512,box);
   const x=value=>(value-256)*scale[0]+256+anchor[0]*5.12;
   const y=value=>(value-scale[1]*5.12)*scale[0]+scale[1]*5.12+anchor[1]*5.12;
   return {faceX:x((face.x0+face.x1)/2),chin:y(face.y1),foot:y(box.y1)};
  }
  // Umbrellas must not be included in the body centre/height. Align the face
  // and feet, then apply one similarity transform to the entire authored loop.
  // This also preserves the clipboard instead of replacing its only frame.
  const rest=await landmarks(idle);
  for(const spec of available.filter(s=>entries.props.includes(s.name))){
   const target=await get(spec),first=await landmarks(target);
   const factor=(rest.foot-rest.chin)/(first.foot-first.chin);
   const dx=rest.faceX-((first.faceX-256)*factor+256);
   anchors[target.url]??=Array.from({length:spec.frames},()=>[0,0]);
   scales[target.url]??=Array.from({length:spec.frames},()=>[1,90]);
   for(let i=0;i<spec.frames;i++){
    const [ax,ay]=anchors[target.url][i]??[0,0],[scale,origin]=scales[target.url][i]??[1,90];
    const round=value=>+value.toFixed(6);
    anchors[target.url][i]=[round(factor*ax+dx/5.12),round(factor*ay+(factor-1)*origin+(rest.foot-factor*first.foot)/5.12)];
    scales[target.url][i]=[round(factor*scale),origin];
   }
  }
  if(id!=='nana')await replace(atlas,0,idle,0);
  for(const spec of available.filter(s=>entries.idle.includes(s.name)))for(const direction of spec.directions??['left'])await replace(await get(spec,direction),0,idle,0);
  for(const chain of entries.chains){
   const from=available.find(s=>s.name===chain.from),to=available.find(s=>s.name===chain.to);
   if(!from||!to)continue;
   for(const direction of to.directions??['left']){
    const target=await get(to,direction),source=await get(from,direction);
    // These wake drawings depict a different bed and body scale. Reuse evenly
    // sampled sleep keyframes backwards so the entire wake, not just frame 0,
    // retains the same character, bedding and registration.
    if(chain.reverseSourceFor?.includes(id))for(let i=0;i<to.frames;i++)await replace(target,i,source,Math.round((1-i/(to.frames-1))*(from.frames-1)));
    await replace(target,0,source,from.frames-1);
    if(chain.returnToIdle)await replace(target,to.frames-1,idle,0);
   }
  }
  // The blink's return is the same rest drawing as its entry.
  const blink=available.find(s=>s.name==='blink');
  if(blink)await replace(await get(blink),blink.frames-1,idle,0);
 }
 for(const s of sheets.values()){
  if(!s.replacements.size)continue;
  const frames=[];
  for(let i=0;i<s.frames;i++)frames.push((await cell(s,i)).input);
  const encoded=await sharp({create:{width:s.cols*512,height:s.rows*512,channels:4,background:'#00000000'}})
   .composite(frames.map((input,i)=>({input,left:i%s.cols*512,top:Math.floor(i/s.cols)*512}))).webp({lossless:true,effort:4}).toBuffer();
  await save(resolve(root,'public'+s.url),encoded);
  console.log(`Entry frames: ${s.url}`);
  for(const [i,cell] of s.replacements){
   anchors[s.url]??=Array.from({length:s.frames},()=>[0,0]);
   scales[s.url]??=Array.from({length:s.frames},()=>[1,90]);
   anchors[s.url][i]=cell.anchor;scales[s.url][i]=cell.scale;
  }
 }
 await save(resolve(root,'src/chibi/frameAnchors.json'),JSON.stringify(anchors,null,2)+'\n');
 await save(resolve(root,'src/chibi/frameScales.json'),JSON.stringify(scales,null,2)+'\n');
 console.log(`Registered entry states for ${(ids??vocab.characters).join(', ')}.`);
}

if(process.argv[1]&&import.meta.url===pathToFileURL(resolve(process.argv[1])).href)await registerEntryFrames(process.argv.slice(2).length?process.argv.slice(2):undefined);
