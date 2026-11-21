import assert from 'node:assert/strict';
import { build } from 'esbuild';
const states=[],refs=[],effects=[],listeners=new Map(),timers=new Map();let stateIndex=0,refIndex=0,initial=true,timerId=0;
const speechStore={currentBubble:null,settledBubbles:[],presenceState:'online',dockState:'Off'};
speechStore.setDockState=value=>{speechStore.dockState=value;};
speechStore.setPresenceState=value=>{speechStore.presenceState=value;};
const priorTimeout=globalThis.setTimeout,priorClearTimeout=globalThis.clearTimeout,priorWindow=globalThis.window,priorRandom=Math.random;
const priorPerformance=globalThis.performance;let clock=0;
globalThis.performance={now:()=>clock};
globalThis.__idlePet={
 calls:[],slides:[],position:{x:0,y:200},
 state(value){const index=stateIndex++;if(initial)states[index]=typeof value==='function'?value():value;return [states[index],value=>states[index]=typeof value==='function'?value(states[index]):value];},
 ref(value){const index=refIndex++;if(initial)refs[index]={current:value};return refs[index];},
 effect(fn){effects.push(fn);},store:speechStore,
 async listen(name,callback){listeners.set(name,callback);return ()=>listeners.delete(name);},
};
globalThis.setTimeout=(fn,ms)=>{const id=++timerId;timers.set(id,{fn,ms,deadline:clock+ms});return id;};globalThis.clearTimeout=id=>timers.delete(id);
globalThis.window={setTimeout:globalThis.setTimeout,clearTimeout:globalThis.clearTimeout,innerWidth:300,innerHeight:300,addEventListener:()=>{},removeEventListener:()=>{},matchMedia:()=>({matches:false})};
const flush=()=>new Promise(resolve=>setImmediate(resolve));
const find=(tree,predicate)=>{if(!tree||typeof tree!=='object')return null;if(predicate(tree))return tree;for(const child of [tree.props?.children].flat(Infinity)){const found=find(child,predicate);if(found)return found;}return null;};
const timerWith=ms=>[...timers.entries()].find(([,timer])=>timer.ms===ms);
const fire=id=>{const timer=timers.get(id);assert.ok(timer,'timer to fire exists');timers.delete(id);clock=Math.max(clock,timer.deadline);timer.fn();};
try{
 const bundle=await build({entryPoints:['src/components/ChibiPetCanvas.tsx'],bundle:true,write:false,platform:'node',format:'esm',loader:{'.css':'empty'},plugins:[{name:'idle-pet-fixtures',setup(b){
  b.onResolve({filter:/^react$|^react\/jsx-runtime$|^@tauri-apps\/api\/|\/characterContext$|\/useAppStore$|\/sheetLoader$|\/useDesktopPhysics$|\/slideTrack$/},({path})=>({path,namespace:'fixture'}));
  b.onLoad({filter:/.*/,namespace:'fixture'},({path})=>({contents:
   path==='react'?`export const forwardRef=fn=>fn,useState=v=>globalThis.__idlePet.state(v),useRef=v=>globalThis.__idlePet.ref(v),useCallback=f=>f,useEffect=f=>globalThis.__idlePet.effect(f),useImperativeHandle=()=>{};`:
   path==='react/jsx-runtime'?`export const jsx=(type,props)=>({type,props}),jsxs=jsx;`:
   path.endsWith('slideTrack')?`export const runSlide=async o=>{if(o.shouldAbort?.())return false;globalThis.__idlePet.slides.push(o.durationMs);await o.apply(o.toX,o.toY);return !o.shouldAbort?.()};`:
   path.endsWith('characterContext')?`export const getCharacterId=()=>"nana";`:
   path.endsWith('useAppStore')?`export const useAppStore=selector=>selector(globalThis.__idlePet.store);useAppStore.getState=()=>globalThis.__idlePet.store;`:
   path.endsWith('useDesktopPhysics')?`export const useDesktopPhysics=()=>({airborne:false,error:false});`:
   path.endsWith('sheetLoader')?`export class SheetLoader{load(){return Promise.resolve(true)}isReady(){return true}clear(){}}`:
   path.endsWith('/event')?`export const listen=(...args)=>globalThis.__idlePet.listen(...args);`:
   path.endsWith('/window')?`export const currentMonitor=async()=>({scaleFactor:1,position:{x:0,y:0},size:{width:1920,height:1080},workArea:{position:{x:0,y:0},size:{width:1920,height:1080}}}),getCurrentWindow=()=>({outerPosition:async()=>globalThis.__idlePet.position,outerSize:async()=>({width:300,height:300}),onMoved:async()=>()=>{}}),getAllWindows=async()=>[];`:
   `export const convertFileSrc=p=>p;export const invoke=async(command,args)=>{globalThis.__idlePet.calls.push({command,args});if(command==="set_pet_dock_position"||command==="set_window_position")globalThis.__idlePet.position={x:args.x,y:args.y};return null};`
  }));}}]});
 const {ChibiPetCanvas:Pet}=await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
 const render=(props={})=>{stateIndex=0;refIndex=0;effects.length=0;const tree=Pet({ambientMotionEnabled:true,...props},{});initial=false;return tree;};
 const stage=tree=>find(tree,node=>node.props?.className?.includes('chibi-pet-stage'));
 const sprite=tree=>find(tree,node=>node.props?.className==='chibi-pet-sprite');

 // 预览不启动自主张望。
 render({previewMode:true});
 const disabledAccent=effects.find(fn=>fn.toString().includes('intervalSecs'));
 assert.ok(disabledAccent,'accent driver is registered');
 disabledAccent();
 assert.equal(timers.size,0,'preview must not arm the idle accent');

 // 张望仍与漫步解耦，但使用随机冷却，不再每四秒播放。
 Math.random=()=>0;
 render({ambientMotionEnabled:false,gameMode:true});
 const accent=effects.find(fn=>fn.toString().includes('intervalSecs'));
 assert.ok(accent,'idle accent driver is registered');
 const cleanup=accent();
 const armed=timerWith(1000);
 assert.ok(armed,'idle accent arms the configured interval');
 assert.equal([...timers.values()].filter(timer=>timer.ms===1000).length,1,'only one accent timer is armed');
 assert.match(sprite(render()).props.style.backgroundImage,/nana-atlas\.webp/,'Nana idle uses her original atlas');
 assert.equal(sprite(render()).props.style.backgroundPosition,'0% 0%','Nana idle uses the first atlas cell');
 assert.equal(sprite(render()).props.style.backgroundSize,'300% 200%');

 // 说话 / 被占用时顺延：不改姿态，改用短重试间隔。
 speechStore.currentBubble='正在说话';
 render();
 clock=25_000;
 fire(armed[0]);
 const retry=timerWith(1000);
 assert.ok(retry,'a blocked accent retries on the short interval');
 let tree=render();
 assert.doesNotMatch(stage(tree).props.className,/pose-look/,'speaking defers the idle accent');

 // 冷却已到，也必须先安静两秒，避免刚结束说话就立刻张望。
 speechStore.currentBubble=null;
 render();
 clock=26_000;
 fire(retry[0]);
 assert.doesNotMatch(stage(render()).props.className,/pose-look/);
 clock=28_000;
 fire(timerWith(1000)[0]);
 tree=render();
 assert.match(stage(tree).props.className,/pose-look/,'idle accent plays the look loop');
 assert.match(sprite(tree).props.style.backgroundImage,/nana-look-sheet\.webp/, 'idle accent loads the look sheet for the active character');
 assert.ok(timerWith(2600),'accent is held for the configured duration');
 const next=timerWith(1000);
 fire(next[0]);
 assert.ok(timerWith(1000),'an active animation is not restarted or interrupted');
 cleanup();
 assert.equal(timerWith(1000),undefined,'unmount cancels the pending retry');
 // Explicit resting owns a full entry sequence and has no automatic wake timer.
 speechStore.presenceState='rest'; render();
 const presenceEffect=()=>effects.find(fn=>fn.toString().includes('enterEdge'));
 const playTransition=async()=>{
  for(let i=0;i<70;i++){
   await flush();
   const next=[...timers.entries()].find(([,timer])=>timer.ms<1000);
   if(!next)break;
   fire(next[0]);
  }
  await flush();
 };
 const restCleanup=presenceEffect()();
 await playTransition();
 assert.match(stage(render()).props.className,/pose-sleep/);
 assert.equal(sprite(render()).props.style.backgroundPosition,'100% 100%');
 assert.equal(timerWith(30000),undefined,'rest stays asleep without the optional-nap timer');
 restCleanup();
 speechStore.presenceState='online';render();
 const wakeCleanup=presenceEffect()();
 await playTransition();
 assert.match(stage(render()).props.className,/pose-idle/,'leaving rest wakes the pet');
 wakeCleanup();

 render();
 const eventCleanup=effects.find(fn=>fn.toString().includes('dock:changed'))();
 await flush();
 const dockChanged=(payload)=>listeners.get('dock:changed')({payload:{character_id:'nana',origin:'drag',...payload}});
 dockChanged({character_id:'vivian',side:'right',mode:'Docked',epoch:1});
 assert.equal(speechStore.dockState,'Off','other characters cannot dock this pet');
 let epoch=1;
 for(const test of [
  {side:'left',visible:{x:0,y:200},docked:{x:-150,y:200},clip:'inset(0px 0px 0px 150px)'},
  {side:'right',visible:{x:1620,y:200},docked:{x:1770,y:200},clip:'inset(0px 150px 0px 0px)'},
  {side:'bottom',visible:{x:800,y:780},docked:{x:800,y:930},clip:'inset(0px 0px 150px 0px)'},
 ]){
  const {side,visible,docked,clip}=test;
  globalThis.__idlePet.position=visible;
  dockChanged({side,mode:'Docked',epoch:++epoch});
  render();const cleanupDock=presenceEffect()();
  await playTransition();
  const tree=render();
  assert.match(stage(tree).props.className,side==='bottom'?/pose-edge-bottom/:/pose-edge/);
  assert.match(sprite(tree).props.style.backgroundImage,side==='bottom'?/edge-bottom-sheet/:/edge-peek(?:-left|-right)?-sheet/);
  if (side === 'bottom') assert.equal(globalThis.__idlePet.slides.at(-1), 900, 'bottom slide lands before the final hold');
  else assert.equal(globalThis.__idlePet.slides.at(-1), 660, 'Nana slides until the same beat as Vivian');
  assert.deepEqual(globalThis.__idlePet.position,docked);
  assert.equal(find(tree,node=>node.props?.className?.startsWith('chibi-pet-canvas')).props.style.clipPath,clip);
  assert(globalThis.__idlePet.calls.some(c=>c.command==='settle_pet_dock'&&c.args.epoch===epoch));
  cleanupDock();
  globalThis.__idlePet.position=visible;
  dockChanged({mode:'Off',epoch:++epoch});
  render();const restore=presenceEffect()();
  await playTransition();
  assert.deepEqual(globalThis.__idlePet.position,visible,'the frontend respects the native restore position');
  assert.equal(find(render(),node=>node.props?.className?.startsWith('chibi-pet-canvas')).props.style.clipPath,undefined);
  restore();
 }
 globalThis.__idlePet.position={x:0,y:200};
 dockChanged({side:'right',mode:'Docked',epoch:++epoch});
 render();const rejectedCleanup=presenceEffect()();
 await playTransition();
 assert.equal(speechStore.dockState,'Off','a stale request away from its edge is rejected');
 assert.equal(globalThis.__idlePet.position.x,0);
 rejectedCleanup();
 eventCleanup();
 Math.random=()=>.999;clock=100000;render();
 globalThis.__idlePet.position={x:600,y:200};
 const leanAccentCleanup=effects.find(fn=>fn.toString().includes('intervalSecs'))();
 clock=200000;fire(timerWith(1000)[0]);clock=202001;fire(timerWith(1000)[0]);await flush();
 assert.match(stage(render()).props.className,/pose-idle/,'a random lean in the middle of the screen stays idle');
 globalThis.__idlePet.position={x:0,y:200};
 stage(render()).props.ref.current={getBoundingClientRect:()=>({width:282})};
 clock=250000;fire(timerWith(1000)[0]);clock=252001;fire(timerWith(1000)[0]);await flush();
 assert.match(stage(render()).props.className,/pose-lean/,'the same random gesture can lean at the real screen edge');
 for(let i=0;i<16;i++){await flush();const next=[...timers.entries()].find(([,t])=>t.ms>=50&&t.ms<=500);if(next)fire(next[0]);}
 await flush();assert.match(stage(render()).props.className,/pose-lean/,'random leaning holds after its entrance finishes');
 assert.match(stage(render()).props.style.transform,/translateX\(-\d/,'the left-facing hand shifts out to the screen edge');
 clock+=30_000;fire(timerWith(100)[0]);await flush();
 assert.match(stage(render()).props.className,/pose-lean/,'leaning remains a sustained state beyond an ordinary gesture');
 sprite(render()).props.onMouseDown({button:0,ctrlKey:false});
 clock+=100;fire(timerWith(100)[0]);await flush();
 assert.match(stage(render()).props.className,/pose-idle/,'direct interaction releases the held lean without a stale exit');
 leanAccentCleanup();
 console.log('Pet idle gestures, persistent sleep entry/hold and native dock transitions passed.');
} finally {
 globalThis.setTimeout=priorTimeout;globalThis.clearTimeout=priorClearTimeout;globalThis.window=priorWindow;Math.random=priorRandom;
 globalThis.performance=priorPerformance;
}
