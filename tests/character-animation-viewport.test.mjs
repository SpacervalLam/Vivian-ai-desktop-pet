import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import sharp from 'sharp';
import {bounds,faceBounds} from '../scripts/chibi/frame-sheets/registration.mjs';
const json=async p=>JSON.parse(await readFile(p,'utf8'));
const vocab=await json('src/chibi/animations.json');
const selectedCharacter=process.argv.find(arg=>arg.startsWith('--character='))?.slice('--character='.length);
for(const id of ['deepseek','claude','chatgpt'].filter(id=>!selectedCharacter||id===selectedCharacter)){
 const m=await json(`character-packs/${id}/character.json`);
 const idle=await sharp(`character-packs/${id}/assets/chibi/${id}-atlas.webp`).extract({left:0,top:0,width:512,height:512}).ensureAlpha().raw().toBuffer();
 const rest=bounds(idle,512,512);
 for(const name of ['turn','walk',...(['claude','deepseek'].includes(id)?['happy']:[])]){
  const spec=m.animations[name];
  for(const direction of spec.directions??['left']){
  const url=spec.sheet.replaceAll('{character}',id).replaceAll('{direction}',direction);
  const faces=[];
  for(let i=0;i<spec.frames;i++){
   const p=await sharp(`character-packs/${id}/assets${url}`).extract({left:i%spec.cols*512,top:Math.floor(i/spec.cols)*512,width:512,height:512}).ensureAlpha().raw().toBuffer();
   const b=bounds(p,512,512),[k,o]=m.scales[url][i],[dx,dy]=m.anchors[url][i];
   faces.push(faceBounds(p,512,512,b).width);
   const x=v=>(v-256)*k+256+dx*5.12,y=v=>(v-o*5.12)*k+o*5.12+dy*5.12;
   assert(x(b.x0)>=1&&x(b.x1)<=510&&y(b.y0)>=1&&y(b.y1)<=510,`${id}/${name}/${i}: entire transformed sprite fits viewport`);
   assert(Math.abs(y(b.y1)-rest.y1)<.01,`${id}/${name}/${i}: stable baseline`);
   const height=y(b.y1)-y(b.y0);
   if(name==='walk'||name==='turn')assert(Math.abs(height-(rest.y1-rest.y0))<rest.height*.07,`${id}/${name}/${i}: authored bob stays within the common camera`);
   else assert(Math.abs(height-(rest.y1-rest.y0))<.01,`${id}/${name}/${i}: no scale jump`);
  }
  if(name==='walk'){
   assert.equal(new Set(m.scales[url].map(([scale])=>scale)).size,1,`${id}: walking never rescales the head from frame to frame`);
   const median=[...faces].sort((a,b)=>a-b)[Math.floor(faces.length/2)];
   assert(faces[0]/median>.8&&faces[0]/median<1.2,`${id}: gait opening keeps its authored viewing angle rather than a foreign turn pose`);
  }
  }
 }
}
console.log('Independent side turns, all three walk cycles and Claude happy: transformed viewport, size and baseline verified.');
