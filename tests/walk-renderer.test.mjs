import assert from 'node:assert/strict';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { build } from 'esbuild';
import { chromium } from 'playwright';

const packages=await Promise.all(['deepseek','claude','chatgpt'].map(async id=>({manifest:JSON.parse(await readFile(`character-packs/${id}/character.json`,'utf8')),root:id})));
const built=await build({stdin:{contents:`
import React,{createRef} from 'react';import {createRoot} from 'react-dom/client';
import {initializeCharacterPackages} from './src/characterPackages';import {setCharacterId} from './src/characterContext';
import {ChibiPetCanvas} from './src/components/ChibiPetCanvas';
await initializeCharacterPackages();setCharacterId(new URL(location.href).searchParams.get('id'));
const ref=createRef();window.pet=ref;
createRoot(document.getElementById('root')).render(<ChibiPetCanvas ref={ref} previewMode ambientMotionEnabled={false} mouseFollowMode='off' onReady={()=>window.ready=true}/>);
`,loader:'tsx',resolveDir:process.cwd()},bundle:true,write:false,format:'esm',outfile:'renderer.js',external:['/chibi/*'],plugins:[{name:'native-walk-fixture',setup(b){
 b.onResolve({filter:/^@tauri-apps\/api\//},args=>({path:args.path,namespace:'native'}));
 b.onLoad({filter:/.*/,namespace:'native'},()=>({contents:`export const invoke=async name=>name==='character_package_status'?${JSON.stringify({active:packages,installed:packages.map(p=>p.manifest)})}:null;export const convertFileSrc=path=>'http://walk-render.test/'+path;export const listen=async()=>()=>{};export const getCurrentWindow=()=>({label:'test'});export const getAllWindows=async()=>[];export const currentMonitor=async()=>null;`}));
 b.onResolve({filter:/\/stores\/useAppStore$/},()=>({path:'store',namespace:'state'}));
 b.onLoad({filter:/.*/,namespace:'state'},()=>({contents:`const state={presenceState:'online',dockState:'Off',settledBubbles:[]};export const useAppStore=Object.assign(fn=>fn(state),{getState:()=>state});`}));
 b.onResolve({filter:/\/hooks\/useDesktopPhysics$/},()=>({path:'physics',namespace:'physics'}));
 b.onLoad({filter:/.*/,namespace:'physics'},()=>({contents:`export const useDesktopPhysics=()=>({airborne:false,error:null});`}));
}}]});
const js=built.outputFiles.find(f=>f.path.endsWith('.js')).text;
const css=built.outputFiles.find(f=>f.path.endsWith('.css')).text;
const browser=await chromium.launch({channel:'chrome',headless:true});
try{
 const page=await browser.newPage({viewport:{width:512,height:512}});const errors=[];
 page.on('pageerror',e=>errors.push(e.message));
 await page.route('http://walk-render.test/**',async route=>{
  const url=new URL(route.request().url());
  const match=url.pathname.match(/^\/(deepseek|claude|chatgpt)\/assets\/(.+)$/);
  if(match)return route.fulfill({contentType:'image/webp',body:await readFile(resolve('character-packs',match[1],'assets',match[2]))});
  if(url.pathname.startsWith('/chibi/'))return route.fulfill({contentType:'image/webp',body:await readFile('public'+url.pathname)});
  return route.fulfill({contentType:'text/html',body:`<style>body{margin:0;background:#efedf4}${css}</style><div id="root"></div><script type="module">${js}</script>`});
 });
 const builtin = JSON.parse(await readFile('src/chibi/animations.json', 'utf8')).animations.find(spec => spec.name === 'walk');
 const traces=[];
 for(const id of ['vivian','nana','deepseek','claude','chatgpt'])for(const direction of ['left','right']){
  await page.goto(`http://walk-render.test/?id=${id}`);await page.waitForFunction(()=>window.ready&&window.pet.current);
  const trace=await page.evaluate(async direction=>{
   const samples=[];let recording=true;
   const sprite=document.querySelector('.chibi-pet-sprite');
   const sample=()=>{samples.push({at:performance.now(),sheet:sprite.style.backgroundImage,position:sprite.style.backgroundPosition,scale:sprite.style.scale});if(recording)requestAnimationFrame(sample);};
   requestAnimationFrame(sample);
   const start=performance.now();const turned=await window.pet.current.playTurn(direction);const turnEnd=performance.now();
   await new Promise(requestAnimationFrame);
   const tail={sheet:sprite.style.backgroundImage,position:sprite.style.backgroundPosition};
   const walking=window.pet.current.playWalk(direction,28,78);
   await new Promise(requestAnimationFrame);
   const entry={sheet:sprite.style.backgroundImage,position:sprite.style.backgroundPosition};
   const walked=await walking;const walkEnd=performance.now();
   const returned=await window.pet.current.playTurnBack();
   await new Promise(requestAnimationFrame);recording=false;
   return {turned,walked,returned,tail,entry,turnMs:turnEnd-start,walkMs:walkEnd-turnEnd,samples};
  },direction);
  assert(trace.turned&&trace.walked&&trace.returned);
  assert(trace.turnMs<450,`${id}/${direction}: turn stays brisk including first load`);
  assert(Math.abs(trace.walkMs-2184)<120,`${id}/${direction}: walking shares the requested movement duration`);
  assert.equal(trace.tail.position,'100% 100%');assert.equal(trace.entry.position,'0% 0%');
  assert(trace.tail.sheet.includes(`-turn-${direction}-sheet`),'turn loads the requested physical side');
  assert(trace.entry.sheet.includes(`-walk-${direction}-sheet`),'walking loads the requested physical side');
  const walk=trace.samples.filter(s=>s.sheet.includes(`-walk-${direction}-sheet`));
  assert(walk.every(s=>Number(s.scale)>0),'right-facing drawing is never reflected');
  assert(walk.length>50);
  const changes=walk.filter((s,i)=>i===0||s.position!==walk[i-1].position);
  const { cols, rows, frames } = packages.find(p => p.manifest.id === id)?.manifest.animations.walk ?? builtin;
  const indices = changes.map(({ position }) => {
   const [x, y] = position.split(' ').map(parseFloat);
   return Math.round(y / 100 * (rows - 1)) * cols + Math.round(x / 100 * (cols - 1));
  });
  assert(indices.every(index => index >= 0 && index < frames), 'every visible cell belongs to the authored cycle');
  assert.equal(indices[0],0);
  for(let i=1;i<indices.length;i++){
   const advanced=(indices[i]-indices[i-1]+frames)%frames;
   const elapsed=changes[i].at-changes[i-1].at;
   assert(advanced>=1&&advanced<=Math.ceil(elapsed/78)+1,`${id}/${direction}: frame progress follows elapsed time without stale resets`);
  }
  if(direction==='right'||!['vivian','nana'].includes(id))assert(new Set(walk.map(s=>s.scale)).size===1,'new walking art keeps a fixed head scale');
  traces.push({id,direction,...trace});
 }
 assert.deepEqual(errors,[]);await mkdir('output/directional-walk-workflow',{recursive:true});
 await writeFile('output/directional-walk-workflow/renderer-trace.json',JSON.stringify(traces,null,2)+'\n');
 console.log('Actual React renderer: all five characters, independent physical directions, turn/entry frames, continuous cycles and shared clock passed.');
}finally{await browser.close();}
