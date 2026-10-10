import assert from 'node:assert/strict';
import {build} from 'esbuild';
const originals=Object.fromEntries(['window','performance'].map(key=>[key,globalThis[key]]));
const effects=[],events=new Map(),dom=new Map(),timers=new Map();
let clock=0,timerId=0,position={x:200,y:30},writes=[],inFlight=0,maxInFlight=0,terrainGate=null;
const coordinator={physicsEnabled:false,physicsInFlight:false,physicsLane:null,dragInFlight:false,fullscreenHidden:false,fullscreenInFlight:false,fleeInFlight:false,smartPositioningInFlight:false,ambientMoveInFlight:false,abortSmartMove:()=>{}};
const world={windows:[],floors:[{id:'floor',x:0,y:0,width:1200,height:800}]};
globalThis.__physicsFixture={effects,coordinator,
 async invoke(name,args){if(name==='get_desktop_terrain'){if(terrainGate)await terrainGate;return world;}if(name==='set_window_position'){maxInFlight=Math.max(maxInFlight,++inFlight);await Promise.resolve();writes.push(args);position={...args};inFlight--; }},
 listen:async(name,callback)=>{events.set(name,callback);return()=>events.delete(name);},
 win:{label:'nana',isVisible:async()=>true,outerPosition:async()=>position,outerSize:async()=>({width:300,height:300}),scaleFactor:async()=>1},
};
globalThis.performance={now:()=>clock};
globalThis.window={setTimeout:(fn,ms)=>{const id=++timerId;timers.set(id,{fn,ms});return id;},clearTimeout:id=>timers.delete(id),
 addEventListener:(name,fn)=>dom.set(name,fn),removeEventListener:(name,fn)=>{if(dom.get(name)===fn)dom.delete(name);},matchMedia:()=>({matches:false})};
const flush=async()=>{for(let i=0;i<6;i++)await new Promise(resolve=>setImmediate(resolve));};
const tick=async()=>{const entry=timers.entries().next().value;assert.ok(entry,'physics schedules next tick');const[id,timer]=entry;timers.delete(id);clock+=timer.ms;timer.fn();await flush();};
try{
 const result=await build({entryPoints:['src/hooks/useDesktopPhysics.ts'],bundle:true,write:false,platform:'node',format:'esm',plugins:[{name:'physics-fixture',setup(b){
  b.onResolve({filter:/^react$|^@tauri-apps\/api\/|\/positioningCoordinator$/},({path})=>({path,namespace:'fixture'}));
  b.onLoad({filter:/.*/,namespace:'fixture'},({path})=>({contents:
   path==='react'?`export const useEffect=fn=>globalThis.__physicsFixture.effects.push(fn),useRef=value=>({current:value}),useState=value=>[value,()=>{}];`:
   path.endsWith('positioningCoordinator')?`export const positioningCoordinator=globalThis.__physicsFixture.coordinator;`:
   path.endsWith('/window')?`export const getCurrentWindow=()=>globalThis.__physicsFixture.win;`:
   path.endsWith('/event')?`export const listen=(...args)=>globalThis.__physicsFixture.listen(...args);`:
   `export const invoke=(...args)=>globalThis.__physicsFixture.invoke(...args);`
  }));
 }}]});
 const {useDesktopPhysics}=await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
 useDesktopPhysics(true,{current:null});const cleanup=effects.pop()();await flush();
 assert.equal(coordinator.physicsEnabled,true);assert.equal(events.size,1);
 for(let i=0;i<180;i++)await tick();
 assert.equal(coordinator.physicsInFlight,false);assert.ok(coordinator.physicsLane);assert.equal(maxInFlight,1,'only one native positioning request is pending');
 const restingWrites=writes.length;for(let i=0;i<20;i++)await tick();assert.equal(writes.length,restingWrites,'resting pet does not issue redundant native moves');
 dom.get('mousedown')();const before=writes.length;for(let i=0;i<4;i++)await tick();assert.equal(writes.length,before,'pointer input suspends physics');
 coordinator.dragInFlight=true;dom.get('mouseup')();await tick();assert.equal(writes.length,before,'native drag keeps ownership after WebView mouseup');
 coordinator.dragInFlight=false;position={x:300,y:200};events.get('drag:released')({payload:{vx:400,vy:-400}});await tick();await tick();assert.ok(position.x>300&&position.y<200,'release momentum reaches the actual native movement loop');
 coordinator.fullscreenHidden=true;const hiddenWrites=writes.length;await tick();await tick();assert.equal(writes.length,hiddenWrites,'fullscreen hiding wins');coordinator.fullscreenHidden=false;
 coordinator.fleeInFlight=true;await tick();assert.equal(writes.length,hiddenWrites,'fleeing wins');coordinator.fleeInFlight=false;
 coordinator.ambientMoveInFlight=true;await tick();assert.equal(writes.length,hiddenWrites,'frontend walking receives no competing physics moves');coordinator.ambientMoveInFlight=false;
 let releaseTerrain;terrainGate=new Promise(resolve=>{releaseTerrain=resolve;});clock+=200;const entry=timers.entries().next().value;timers.delete(entry[0]);entry[1].fn();await flush();
 const disposedWrites=writes.length;cleanup();releaseTerrain();await flush();assert.equal(writes.length,disposedWrites,'disposed terrain read cannot restart movement');
 assert.equal(events.size,0);assert.equal(dom.size,0);assert.equal(timers.size,0);assert.equal(coordinator.physicsEnabled,false);assert.equal(coordinator.physicsLane,null);
 useDesktopPhysics(false,{current:null});effects.pop()();assert.equal(timers.size,0,'disabled mode creates no polling loop');
 console.log('Desktop physics lifecycle: serialized real movement, resting budget, native release, input/hide/flee/walk ownership, pending-read cancellation and complete listener/timer cleanup passed');
}finally{
 for(const[key,value]of Object.entries(originals)){if(value===undefined)delete globalThis[key];else globalThis[key]=value;}
 delete globalThis.__physicsFixture;
}
