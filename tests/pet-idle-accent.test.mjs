import assert from 'node:assert/strict';
import { build } from 'esbuild';
const states=[],refs=[],effects=[],listeners=new Map(),timers=new Map();let stateIndex=0,refIndex=0,initial=true,timerId=0;
const speechStore={currentBubble:null,settledBubbles:[]};
const priorTimeout=globalThis.setTimeout,priorClearTimeout=globalThis.clearTimeout,priorWindow=globalThis.window,priorRandom=Math.random;
const priorPerformance=globalThis.performance;let clock=0;
globalThis.performance={now:()=>clock};
globalThis.__idlePet={
 state(value){const index=stateIndex++;if(initial)states[index]=typeof value==='function'?value():value;return [states[index],value=>states[index]=typeof value==='function'?value(states[index]):value];},
 ref(value){const index=refIndex++;if(initial)refs[index]={current:value};return refs[index];},
 effect(fn){effects.push(fn);},store:speechStore,
 async listen(name,callback){listeners.set(name,callback);return ()=>listeners.delete(name);},
};
globalThis.setTimeout=(fn,ms)=>{const id=++timerId;timers.set(id,{fn,ms});return id;};globalThis.clearTimeout=id=>timers.delete(id);
globalThis.window={setTimeout:globalThis.setTimeout,clearTimeout:globalThis.clearTimeout,innerWidth:300,innerHeight:300,matchMedia:()=>({matches:false})};
const flush=()=>new Promise(resolve=>setImmediate(resolve));
const find=(tree,predicate)=>{if(!tree||typeof tree!=='object')return null;if(predicate(tree))return tree;for(const child of [tree.props?.children].flat(Infinity)){const found=find(child,predicate);if(found)return found;}return null;};
const timerWith=ms=>[...timers.entries()].find(([,timer])=>timer.ms===ms);
const fire=id=>{const timer=timers.get(id);assert.ok(timer,'timer to fire exists');timers.delete(id);timer.fn();};
try{
 const bundle=await build({entryPoints:['src/components/ChibiPetCanvas.tsx'],bundle:true,write:false,platform:'node',format:'esm',loader:{'.css':'empty'},plugins:[{name:'idle-pet-fixtures',setup(b){
  b.onResolve({filter:/^react$|^react\/jsx-runtime$|^@tauri-apps\/api\/|\/characterContext$|\/useAppStore$|\/sheetLoader$|\/useDesktopPhysics$/},({path})=>({path,namespace:'fixture'}));
  b.onLoad({filter:/.*/,namespace:'fixture'},({path})=>({contents:
   path==='react'?`export const forwardRef=fn=>fn,useState=v=>globalThis.__idlePet.state(v),useRef=v=>globalThis.__idlePet.ref(v),useCallback=f=>f,useEffect=f=>globalThis.__idlePet.effect(f),useImperativeHandle=()=>{};`:
   path==='react/jsx-runtime'?`export const jsx=(type,props)=>({type,props}),jsxs=jsx;`:
   path.endsWith('characterContext')?`export const getCharacterId=()=>"nana";`:
   path.endsWith('useAppStore')?`export const useAppStore=selector=>selector(globalThis.__idlePet.store);useAppStore.getState=()=>globalThis.__idlePet.store;`:
   path.endsWith('useDesktopPhysics')?`export const useDesktopPhysics=()=>({airborne:false,error:false});`:
   path.endsWith('sheetLoader')?`export class SheetLoader{load(){return Promise.resolve(true)}isReady(){return true}clear(){}}`:
   path.endsWith('/event')?`export const listen=(...args)=>globalThis.__idlePet.listen(...args);`:
   path.endsWith('/window')?`export const currentMonitor=async()=>null,getCurrentWindow=()=>({}),getAllWindows=async()=>[];`:
   `export const convertFileSrc=p=>p;export const invoke=async()=>null;`
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
 assert.match(sprite(render()).props.style.backgroundImage,/nana-idle-sheet\.webp/,'idle uses the cached look opening frame');

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
 console.log('pet idle accent passed');
} finally {
 globalThis.setTimeout=priorTimeout;globalThis.clearTimeout=priorClearTimeout;globalThis.window=priorWindow;Math.random=priorRandom;
 globalThis.performance=priorPerformance;
}
