import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import sharp from 'sharp';
import {build} from 'esbuild';

// Exercise the actual hook movement routine with window IPC replaced by a trace.
const source=await fs.readFile('src/hooks/useSmartPositioning.ts','utf8');
const start=source.indexOf('    const animatePosition = async (');
const end=source.indexOf('    const runCheck =',start);
assert.ok(start>=0&&end>start);
const bundle=await build({stdin:{contents:`
import {planSmartMove} from './src/chibi/walkPlan';
export async function move(x,y,env){
 const cancelled=false,positioningCoordinator={physicsEnabled:false};
 const moveTokenRef={current:1},userInteractingRef={current:false},focusedRef=env.focused;
 const petRef={current:env.pet},invoke=async()=>{},runSlide=env.slide;
 ${source.slice(start,end)}
 await animatePosition(x,y,0,0,1);
}`,resolveDir:process.cwd(),loader:'ts'},bundle:true,write:false,platform:'node',format:'esm'});
const {move}=await import('data:text/javascript;base64,'+Buffer.from(bundle.outputFiles[0].text).toString('base64'));
function fixture(){
 const trace=[],focused={current:false};
 return {trace,focused,pet:{
  async startVerticalMove(){trace.push('float');return ()=>trace.push('stop-float');},
  async playTurn(){trace.push('turn');return true;},
  async playWalk(){trace.push('walk');return true;},
  async playTurnBack(){trace.push('turn-back');return true;},
  resetExpression(){trace.push('reset');},
 },async slide(options){trace.push('slide');assert.ok(options.durationMs>0);return true;}};
}
for(const [x,y] of [[0,-300],[0,300],[5,300],[-5,-300]]){
 const env=fixture();await move(x,y,env);
 assert.deepEqual(env.trace,['float','slide','stop-float'],`${x},${y}: hover follows the vertical move`);
}
const horizontal=fixture();await move(300,20,horizontal);
assert.deepEqual(horizontal.trace,['turn','walk','slide','turn-back'],'horizontal movement retains its walking animation');
const interrupted=fixture();interrupted.slide=async()=>{interrupted.trace.push('slide');return false;};
await move(0,300,interrupted);
assert.deepEqual(interrupted.trace,['float','slide','stop-float','reset']);
const failed=fixture();failed.slide=async()=>{throw new Error('IPC failed');};
await assert.rejects(move(0,-300,failed),/IPC failed/);
assert.deepEqual(failed.trace,['float','stop-float'],'IPC failure cannot leave the hover loop running');
for(const character of ['vivian','nana']){
 const path=`public/chibi/motion/${character}/${character}-float-sheet.webp`;
 const {data,info}=await sharp(path).ensureAlpha().raw().toBuffer({resolveWithObject:true});
 assert.deepEqual([info.width,info.height,info.channels],[2048,1024,4]);
 for(let frame=0;frame<8;frame++){
  let opaque=0;
  for(let y=0;y<512;y++)for(let x=0;x<512;x++){
   const alpha=data[((Math.floor(frame/4)*512+y)*2048+(frame%4)*512+x)*4+3];
   if(alpha)opaque++;
   if(x<12||x>=500||y<12||y>=500)assert.equal(alpha,0,`${character}: transparent cell margin`);
  }
  assert.ok(opaque>512*512*.15,`${character}: frame ${frame+1} has artwork`);
 }
}
console.log('Vertical movement: ascent/descent selection, horizontal walk, interruption cleanup and eight-frame transparent assets passed.');
