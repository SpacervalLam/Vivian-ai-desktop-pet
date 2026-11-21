import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import sharp from 'sharp';
import {createHash} from 'node:crypto';
import {bounds,faceBounds} from '../scripts/chibi/frame-sheets/registration.mjs';

const anchors=JSON.parse(await readFile('src/chibi/frameAnchors.json','utf8'));
const scales=JSON.parse(await readFile('src/chibi/frameScales.json','utf8'));
const walk='/chibi/walk/nana/nana-walk-right-sheet.webp';
const turn='/chibi/motion/nana/nana-turn-right-sheet.webp';
const pixels=await sharp(await readFile('public'+walk)).ensureAlpha().raw().toBuffer();
const idle=await sharp('public/chibi/nana-atlas.webp').extract({left:0,top:0,width:512,height:512}).ensureAlpha().raw().toBuffer();
const floor=bounds(idle,512,512).y1;
const faces=[],footSpreads=[];
const handFrames=[];
const protectedLegHashes=JSON.parse(await readFile(new URL('./fixtures/nana-walk-leg-hashes.json',import.meta.url),'utf8'));
// Arm correction must preserve the approved head and existing leg animation.
const approvedHeadHashes=[
 'b0502183699690fb5f65e07b834c3f6d9dfe71c0eb39c748e9d683a936ec8734',
 '479c23beea801b76eb5a3c0b312a8cd074bd6456f015ccbad6e886ebdfb0325d',
];
function visibleHash(bytes){
 const normalized=Buffer.from(bytes);
 for(let p=0;p<normalized.length;p+=4)if(!normalized[p+3])normalized.fill(0,p,p+4);
 return createHash('sha256').update(normalized).digest('hex');
}
function handParts(cell){
 const skin=new Uint8Array(512*512),parts=[];
 for(let y=285;y<365;y++)for(let x=225;x<435;x++){
  const k=(y*512+x)*4,r=cell[k],g=cell[k+1],b=cell[k+2];
  if(cell[k+3]>200&&r>190&&g>130&&b>110&&r>g+10&&r>b+10)skin[y*512+x]=1;
 }
 for(let start=0;start<skin.length;start++)if(skin[start]){
  const queue=[start];skin[start]=0;let x=0,y=0;
  for(let j=0;j<queue.length;j++){
   const p=queue[j];x+=p%512;y+=Math.floor(p/512);
   for(const n of [p-1,p+1,p-512,p+512])if(n>=0&&n<skin.length&&skin[n]){skin[n]=0;queue.push(n);}
  }
  if(queue.length>=35)parts.push({x:x/queue.length,y:y/queue.length});
 }
 return parts;
}
for(let i=0;i<14;i++){
 const cell=Buffer.alloc(512*512*4);
 for(let y=0;y<512;y++){
  const at=((Math.floor(i/4)*512+y)*2048+i%4*512)*4;
  pixels.copy(cell,y*2048,at,at+2048);
 }
 assert.equal(visibleHash(cell.subarray(0,275*512*4)),approvedHeadHashes[i%2],
   `Nana ${i}: the user-drawn head remains pixel-identical`);
 assert.equal(visibleHash(cell.subarray(375*512*4)),protectedLegHashes[i],
   `Nana ${i}: arm correction cannot change the legs`);
 const parts=handParts(cell),targetX=300-43*Math.cos(i*2*Math.PI/14);
 const near=parts.filter(p=>p.y>=300).sort((a,b)=>Math.abs(a.x-targetX)-Math.abs(b.x-targetX))[0];
 assert(near,`Nana ${i}: visible near hand`);handFrames.push({near,parts});
 const box=bounds(cell,512,512),f=faceBounds(cell,512,512,box);
 const [scale,origin]=scales[walk][i],[dx,dy]=anchors[walk][i];
 const sole=origin*5.12+(box.y1-origin*5.12)*scale+dy*5.12;
 assert(Math.abs(sole-floor)<=3,`Nana ${i}: supporting shoe stays at floor level`);
 let footLeft=512,footRight=-1;
 for(let y=box.y1-24;y<=box.y1;y++)for(let x=0;x<512;x++)if(cell[(y*512+x)*4+3]>200){
  footLeft=Math.min(footLeft,x);footRight=Math.max(footRight,x);
 }
 const spread=(footRight-footLeft)*scale;
 footSpreads.push(spread);
 assert(spread<=175,`Nana ${i}: leg opening stays within the corrected artwork's stride`);
 faces.push({x:256+((f.x0+f.x1)/2-256)*scale+dx*5.12,
   y:origin*5.12+((f.y0+f.y1)/2-origin*5.12)*scale+dy*5.12});
}
assert(Math.max(...footSpreads)>=140,'retain a visible stride rather than tiny shuffling steps');
assert(Math.abs(footSpreads[0]-footSpreads[7])<12,
 'opposite-leg contact poses have comparable openings in the guided cycle');
for(let i=0;i<faces.length;i++){
 const next=(i+1)%faces.length;
 assert(Math.abs(faces[next].x-faces[i].x)<6,`Nana ${i}->${next}: tail swing must not shift the face sideways`);
 assert(Math.abs(faces[next].y-faces[i].y)<6,`Nana ${i}->${next}: a raised foot must not pull the head down`);
}
assert.equal(new Set(scales[walk].map(([s])=>s)).size,1,'fixed camera scale');
assert.deepEqual(anchors[turn][7],anchors[walk][0],'turn and walk meet at the same position');
assert.deepEqual(scales[turn][7],scales[walk][0],'turn and walk meet at the same scale');
assert(handFrames[0].near.x<280&&handFrames[7].near.x>330,'opposite-leg contacts use opposite arm extremes');
for(let i=0;i<14;i++){
 const p=handFrames[i].near,n=handFrames[(i+1)%14].near;
 assert(Math.hypot(n.x-p.x,n.y-p.y)<33,`Nana ${i}: no two-frame arm flicker or wrist teleport`);
 if(i<7)assert(n.x>=p.x-1,'near arm sweeps forward during the first half-cycle');
 if(i>=7&&i<13)assert(n.x<=p.x+1,'near arm sweeps back during the second half-cycle');
}
assert(handFrames[0].parts.some(p=>p.x>385),'far hand is forward at the first contact');
assert(!handFrames[7].parts.some(p=>p.x>385),'far hand retracts rather than staying forward through the entire walk');
console.log('Nana right walk: face stays continuous over all 14 frames, loop seam and turn handoff.');
