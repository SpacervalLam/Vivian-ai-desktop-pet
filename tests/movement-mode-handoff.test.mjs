import assert from 'node:assert/strict';
import {build} from 'esbuild';
const effects=[],timers=new Map(),dom=new Map(),calls=[],coordinator={physicsEnabled:true};let timerId=0,gate=null;
const oldWindow=globalThis.window;
globalThis.window={setTimeout:(fn)=>{const id=++timerId;timers.set(id,fn);return id;},clearTimeout:id=>timers.delete(id),addEventListener:(name,fn)=>dom.set(name,fn),removeEventListener:name=>dom.delete(name)};
globalThis.__handoff={effects,coordinator,async invoke(name,args){calls.push({name,args});if(name==='find_safe_position'){if(gate)await gate;return{unchanged:false,region:{x:400,y:200,width:100,height:100}};}},win:{isVisible:async()=>true,outerPosition:async()=>({x:0,y:100}),outerSize:async()=>({width:100,height:100}),onFocusChanged:async()=>()=>{}}};
const flush=async()=>{for(let i=0;i<8;i++)await new Promise(resolve=>setImmediate(resolve));};
try{
 const bundle=await build({entryPoints:['src/hooks/useSmartPositioning.ts'],bundle:true,write:false,platform:'node',format:'esm',plugins:[{name:'handoff-fixture',setup(b){
  b.onResolve({filter:/^react$|^@tauri-apps\/api\/|\/positioningCoordinator$|\/characterContext$|\/slideTrack$/},({path})=>({path,namespace:'fixture'}));
  b.onLoad({filter:/.*/,namespace:'fixture'},({path})=>({contents:path==='react'?`export const useEffect=fn=>globalThis.__handoff.effects.push(fn),useRef=value=>({current:value});`:path.endsWith('positioningCoordinator')?`export const positioningCoordinator=globalThis.__handoff.coordinator;`:path.endsWith('characterContext')?`export const getCharacterId=()=> 'vivian';`:path.endsWith('slideTrack')?`export const runSlide=async o=>{if(o.shouldAbort())return false;await o.apply(o.toX,o.toY);return true;};`:path.endsWith('/window')?`export const getCurrentWindow=()=>globalThis.__handoff.win;`:`export const convertFileSrc=p=>p;export const invoke=(...a)=>globalThis.__handoff.invoke(...a);`}));
 }}]});
 const {useSmartPositioning}=await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
 useSmartPositioning({current:{playTurn:async()=>false,resetExpression:()=>{}}},true,true);const cleanups=effects.map(fn=>fn());await flush();
 coordinator.triggerSmartCheck();await flush();assert.equal(calls.length,0,'physics blocks even a forced smart check');
 coordinator.physicsEnabled=false;let resume;gate=new Promise(resolve=>resume=resolve);coordinator.triggerSmartCheck();await flush();assert.equal(calls.at(-1).name,'find_safe_position');
 coordinator.physicsEnabled=true;resume();await flush();assert.equal(calls.filter(c=>c.name==='set_window_position').length,0,'switching while analysis awaits prevents a stale move');
 coordinator.physicsEnabled=false;gate=null;coordinator.triggerSmartCheck();await flush();assert.equal(calls.filter(c=>c.name==='set_window_position').length,1,'smart positioning resumes after physics is disabled');
 cleanups.forEach(fn=>fn?.());assert.equal(timers.size,0);assert.equal(dom.size,0);assert.equal(coordinator.triggerSmartCheck,null);
 console.log('Movement handoff: forced checks, physics during pending analysis, resume, cleanup passed');
}finally{globalThis.window=oldWindow;delete globalThis.__handoff;}
