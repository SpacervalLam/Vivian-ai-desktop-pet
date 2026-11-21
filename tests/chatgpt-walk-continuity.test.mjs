import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import sharp from 'sharp';
import {bounds} from '../scripts/chibi/frame-sheets/registration.mjs';

const root='character-packs/chatgpt',manifest=JSON.parse(await readFile(root+'/character.json','utf8'));
const portraits={right:'c47aa7fc2d683fd984ee07c41fd62edf1c3520cd4c11a6f91216382aa2fe2e75',left:'560093dd8d68acaf55074cfcade6372484708497a943875308ead6c7d77b08fc'};
const sha=b=>createHash('sha256').update(b).digest('hex');
function visibleHash(bytes){const p=Buffer.from(bytes);for(let k=0;k<p.length;k+=4)if(!p[k+3])p.fill(0,k,k+4);return sha(p);}
function hands(p){
 const mask=new Uint8Array(512*512),parts=[];
 for(let y=280;y<365;y++)for(let x=75;x<440;x++){const k=(y*512+x)*4,r=p[k],g=p[k+1],b=p[k+2];if(p[k+3]>200&&r>190&&g>130&&b>110&&r>g+10&&r>b+10)mask[y*512+x]=1;}
 for(let start=0;start<mask.length;start++)if(mask[start]){const q=[start];mask[start]=0;let x=0,y=0;for(let j=0;j<q.length;j++){const k=q[j];x+=k%512;y+=Math.floor(k/512);for(const n of[k-1,k+1,k-512,k+512])if(n>=0&&n<mask.length&&mask[n]){mask[n]=0;q.push(n);}}if(q.length>=25)parts.push({x:x/q.length,y:y/q.length});}
 return parts;
}
for(const direction of ['right','left']){
 const url=`/chibi/walk/chatgpt/chatgpt-walk-${direction}-sheet.webp`,turn=`/chibi/motion/chatgpt/chatgpt-turn-${direction}-sheet.webp`;
 const bytes=await readFile(root+'/assets'+url),tp=await readFile(root+'/assets'+turn);
 assert.equal(sha(bytes),manifest.files['assets'+url]);assert.equal(sha(tp),manifest.files['assets'+turn]);
 const image=await sharp(bytes).ensureAlpha().raw().toBuffer(),near=[],parts=[],spreads=[],legs=new Set();let first;
 for(let i=0;i<14;i++){
  const p=Buffer.alloc(512*512*4);for(let y=0;y<512;y++){const at=((Math.floor(i/4)*512+y)*2048+i%4*512)*4;image.copy(p,y*2048,at,at+2048);}
  first??=p;
  assert.equal(visibleHash(p.subarray(0,255*512*4)),portraits[direction],`${direction}/${i}: approved head and ornament stay unchanged`);
  legs.add(visibleHash(p.subarray(389*512*4)));
  const b=bounds(p,512,512),[s,o]=manifest.scales[url][i],[dx,dy]=manifest.anchors[url][i];
  assert(b.x0>0&&b.x1<511&&b.y0>0&&b.y1<511,`${direction}/${i}: no clipped artwork`);
  const sole=o*5.12+(b.y1-o*5.12)*s+dy*5.12;
  const rest=bounds(await sharp(root+'/assets/chibi/chatgpt-atlas.webp').extract({left:0,top:0,width:512,height:512}).ensureAlpha().raw().toBuffer(),512,512);
  assert(Math.abs(sole-rest.y1)<.01,`${direction}/${i}: grounded support`);
  let l=512,r=-1;for(let y=b.y1-24;y<=b.y1;y++)for(let x=0;x<512;x++)if(p[(y*512+x)*4+3]>200){l=Math.min(l,x);r=Math.max(r,x);}spreads.push(r-l);
  const all=hands(p),hint=280-45*Math.cos(i*2*Math.PI/14),target=direction==='right'?hint:519-hint;
  const hand=all.filter(h=>h.y>285).sort((a,b)=>Math.hypot(a.x-target,a.y-330)-Math.hypot(b.x-target,b.y-330))[0];
  assert(hand,`${direction}/${i}: visible near hand`);near.push(hand);parts.push(all);
 }
 assert.equal(legs.size,14,`${direction}: complete gait rather than repeated or standing bookend poses`);
 assert(Math.max(...spreads)<=145&&Math.max(...spreads)>=110,`${direction}: modest visible steps`);
 assert(Math.abs(spreads[0]-spreads[7])<15,`${direction}: comparable opposite-leg contacts`);
 assert.equal(new Set(manifest.scales[url].map(([s])=>s)).size,1,'fixed camera scale');
 for(let i=0;i<14;i++){
  const a=near[i],b=near[(i+1)%14],sign=direction==='right'?1:-1;
  assert(Math.hypot(b.x-a.x,b.y-a.y)<40,`${direction}/${i}: no abrupt wrist jumps`);
  if(i<7)assert(sign*(b.x-a.x)>=-1,`${direction}/${i}: forward arm phase`);
  if(i>=7&&i<13)assert(sign*(b.x-a.x)<=1,`${direction}/${i}: returning arm phase`);
 }
 assert((direction==='right'?1:-1)*(near[7].x-near[0].x)>70,'same-side arm opposes the planted leg');
 assert(parts[0].length>parts[7].length,'far hand retracts behind the torso instead of staying forward');
 const turnLast=await sharp(tp).extract({left:1536,top:512,width:512,height:512}).ensureAlpha().raw().toBuffer();
 assert.equal(visibleHash(turnLast),visibleHash(first),'turn and walk share their visible endpoint');
 assert.deepEqual(manifest.anchors[turn][7],manifest.anchors[url][0]);assert.deepEqual(manifest.scales[turn][7],manifest.scales[url][0]);
}
console.log('ChatGPT: both independent directions, locked portraits,14 unique gait frames, counter-swinging hands, stable floor, turn endpoints and package hashes passed.');
