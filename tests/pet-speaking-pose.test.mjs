import assert from 'node:assert/strict';
import { build } from 'esbuild';
const states=[],refs=[],effects=[],listeners=new Map(),timers=new Map();let stateIndex=0,refIndex=0,initial=true,timerId=0,handle;
const speechStore={currentBubble:null,settledBubbles:[]};
const names=['window','setTimeout','clearTimeout'];const prior=Object.fromEntries(names.map(name=>[name,globalThis[name]]));
globalThis.__speechPet={
 state(value){const index=stateIndex++;if(initial)states[index]=typeof value==='function'?value():value;return [states[index],value=>states[index]=typeof value==='function'?value(states[index]):value];},
 ref(value){const index=refIndex++;if(initial)refs[index]={current:value};return refs[index];},
 effect(fn){effects.push(fn);},store:speechStore,setHandle(value){handle=value;},
 async listen(name,callback){listeners.set(name,callback);return ()=>listeners.delete(name);},
};
globalThis.setTimeout=(fn,ms)=>{const id=++timerId;timers.set(id,{fn,ms});return id;};globalThis.clearTimeout=id=>timers.delete(id);
globalThis.window={setTimeout:globalThis.setTimeout,clearTimeout:globalThis.clearTimeout,innerWidth:300,innerHeight:300,matchMedia:()=>({matches:false})};
const flush=()=>new Promise(resolve=>setImmediate(resolve));
const find=(tree,predicate)=>{if(!tree||typeof tree!=='object')return null;if(predicate(tree))return tree;for(const child of [tree.props?.children].flat(Infinity)){const found=find(child,predicate);if(found)return found;}return null;};
try{
 const bundle=await build({entryPoints:['src/components/ChibiPetCanvas.tsx'],bundle:true,write:false,platform:'node',format:'esm',loader:{'.css':'empty'},plugins:[{name:'speech-pet-fixtures',setup(b){
 b.onResolve({filter:/^react$|^react\/jsx-runtime$|^@tauri-apps\/api\/|\/characterContext$|\/useAppStore$|\/sheetLoader$|\/useDesktopPhysics$/},({path})=>({path,namespace:'fixture'}));
 b.onLoad({filter:/.*/,namespace:'fixture'},({path})=>({contents:
 path==='react'?`export const forwardRef=fn=>fn,useState=v=>globalThis.__speechPet.state(v),useRef=v=>globalThis.__speechPet.ref(v),useCallback=f=>f,useEffect=f=>globalThis.__speechPet.effect(f),useImperativeHandle=(ref,create)=>globalThis.__speechPet.setHandle(create());`:
 path==='react/jsx-runtime'?`export const jsx=(type,props)=>({type,props}),jsxs=jsx;`:
 path.endsWith('characterContext')?`export const getCharacterId=()=>"nana";`:
 path.endsWith('useAppStore')?`export const useAppStore=selector=>selector(globalThis.__speechPet.store);useAppStore.getState=()=>globalThis.__speechPet.store;`:
 path.endsWith('useDesktopPhysics')?`export const useDesktopPhysics=()=>({airborne:Boolean(globalThis.__speechPet.airborne),error:false});`:
 path.endsWith('sheetLoader')?`export class SheetLoader{load(){return globalThis.__speechPet.pendingSheet??Promise.resolve(true)}isReady(){return !globalThis.__speechPet.pendingSheet}clear(){}}`:
 path.endsWith('/event')?`export const listen=(...args)=>globalThis.__speechPet.listen(...args);`:
 path.endsWith('/window')?`export const currentMonitor=async()=>null,getCurrentWindow=()=>({}),getAllWindows=async()=>[];`:
 `export const convertFileSrc=p=>p;export const invoke=async()=>null;`
 }));}}]});
 const {ChibiPetCanvas:Pet}=await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
 const render=()=>{stateIndex=0;refIndex=0;effects.length=0;const tree=Pet({ambientMotionEnabled:false},{});initial=false;return tree;};
 const stage=tree=>find(tree,n=>n.props?.className?.includes('chibi-pet-stage'));
 const sprite=tree=>find(tree,n=>n.props?.className==='chibi-pet-sprite');
 const send=(name,payload)=>listeners.get(name)({payload});
 let tree=render();assert.match(stage(tree).props.className,/pose-idle/);
 const eventEffect=effects.find(fn=>fn.toString().includes('tts:started'));assert.ok(eventEffect);const cleanup=eventEffect();await flush();
 speechStore.currentBubble='主动想和你说一句话';tree=render();assert.match(stage(tree).props.className,/pose-talk/);assert.equal(sprite(tree).props.style.backgroundPosition,'50% 100%','use actual talk atlas cell, not the stale idle/animation frame');
 speechStore.currentBubble=null;send('tts:started',{character_id:'vivian'});tree=render();assert.doesNotMatch(stage(tree).props.className,/pose-talk/,'other character audio is isolated');
 send('tts:started',{character_id:'nana'});tree=render();assert.match(stage(tree).props.className,/pose-talk/);
 send('chat:done',{character_id:'nana',stream_id:'finished-text'});await flush();tree=render();assert.match(stage(tree).props.className,/pose-talk/,'generation done/emotion cannot end active spoken audio');
 send('tts:finished',{character_id:'nana'});await flush();tree=render();assert.doesNotMatch(stage(tree).props.className,/pose-talk/);
 speechStore.settledBubbles=[{text:'还有一段没读完的气泡'}];tree=render();assert.match(stage(tree).props.className,/pose-talk/,'settled text remains spoken while visible');
 speechStore.settledBubbles=[{text:'',sticker:{}}];tree=render();assert.doesNotMatch(stage(tree).props.className,/pose-talk/,'sticker alone is not speech');
 send('chat:waiting',{character_id:'nana',stream_id:'new'});send('tts:started',{character_id:'nana'});tree=render();assert.match(stage(tree).props.className,/pose-thinking/,'speech preserves the thinking animation until its end signal and natural outro');
 send('presentation:stop',{speaker_id:'nana'});tree=render();assert.doesNotMatch(stage(tree).props.className,/pose-talk/);
 send('chat:error',{character_id:'nana',stream_id:'new'});send('tts:started',{character_id:'nana'});
 handle.setExpression('drag');tree=render();assert.match(stage(tree).props.className,/pose-drag/,'dragging is not overwritten by speech');
 send('tts:finished',{character_id:'nana'});tree=render();assert.match(stage(tree).props.className,/pose-drag/,'audio completion preserves dragging');
 send('tts:started',{character_id:'nana'});tree=render();assert.match(stage(tree).props.className,/pose-drag/,'audio start preserves dragging');
 send('tts:error',{character_id:'nana'});tree=render();assert.match(stage(tree).props.className,/pose-drag/,'audio failure preserves dragging');
 void handle.playWalk('left',14,80);tree=render();assert.match(stage(tree).props.className,/pose-walk/,'walking sprite and movement stay synchronized');
 send('presentation:stop',{speaker_id:'nana'});handle.resetExpression();tree=render();
 send('cross:start',{stream_id:'pair-1',speaker_id:'vivian',listener_id:'nana'});tree=render();assert.match(stage(tree).props.className,/pose-listen/);
 send('cross:chunk',{stream_id:'pair-1',speaker_id:'nana',listener_id:'vivian',text:'reply'});tree=render();assert.match(stage(tree).props.className,/pose-talk/);
 handle.setExpression('drag');send('cross:done',{stream_id:'pair-1',speaker_id:'nana',listener_id:'vivian'});tree=render();assert.match(stage(tree).props.className,/pose-drag/,'cross completion cannot replace a user drag');
 handle.resetExpression();send('cross:start',{stream_id:'pair-2',speaker_id:'vivian',listener_id:'nana'});send('cross:done',{stream_id:'pair-1',speaker_id:'nana',listener_id:'vivian'});tree=render();assert.match(stage(tree).props.className,/pose-listen/,'stale done preserves newer listening');
 send('cross:error',{stream_id:'pair-2',source_id:'vivian',target_id:'nana'});tree=render();assert.match(stage(tree).props.className,/pose-idle/,'error releases listening');
 send('cross:start',{stream_id:'pair-3',speaker_id:'vivian',listener_id:'nana'});send('chat:waiting',{character_id:'nana',stream_id:'user'});send('cross:chunk',{stream_id:'pair-3',speaker_id:'vivian',listener_id:'nana',text:'late'});tree=render();assert.doesNotMatch(stage(tree).props.className,/pose-listen/,'late peer text cannot reclaim a user conversation');
 send('chat:error',{character_id:'nana',stream_id:'user'});
 // Finish the thought presentation before testing unrelated peer ownership.
 // The frame-driven natural outro is exercised in pet-thinking-loop.test.mjs.
 handle.resetExpression();
 send('cross:start',{stream_id:'pair-4',speaker_id:'vivian',listener_id:'nana'});send('character:online_changed',{character_id:'vivian',online:false});tree=render();assert.match(stage(tree).props.className,/pose-idle/,'offline peer releases presentation');
 send('cross:start',{stream_id:'pair-5',speaker_id:'vivian',listener_id:'nana'});const watchdog=[...timers.values()].find(timer=>timer.ms===180000);assert.ok(watchdog);watchdog.fn();tree=render();assert.match(stage(tree).props.className,/pose-idle/,'missing terminal event cannot leave listening forever');
 handle.resetExpression();globalThis.__speechPet.airborne=true;
 tree=render();assert.match(stage(tree).props.className,/pose-drag/,'frontend inertia keeps the floating atlas pose');
 assert.equal(sprite(tree).props.style.backgroundPosition,'100% 0%','inertia uses the third atlas cell');
 handle.setExpression('dizzy',1800);
 tree=render();assert.match(stage(tree).props.className,/pose-dizzy/,'ceiling dizziness remains visible while airborne');
 send('tts:started',{character_id:'nana'});tree=render();assert.match(stage(tree).props.className,/pose-dizzy/,'spoken collision reaction cannot replace dizzy with talk');
 globalThis.__speechPet.airborne=false;
 send('presentation:stop',{speaker_id:'nana'});handle.resetExpression();
 send('drag:fling',{character_id:'vivian',generation:1,active:true});
 tree=render();assert.doesNotMatch(stage(tree).props.className,/pose-drag/,'other character flings are isolated');
 send('drag:fling',{character_id:'nana',generation:1,active:true});
 tree=render();assert.match(stage(tree).props.className,/pose-drag/,'backend inertia keeps the floating pose without desktop physics');
 assert.equal(sprite(tree).props.style.backgroundPosition,'100% 0%');
 assert.match(sprite(tree).props.style.backgroundImage,/nana-atlas\.webp/,'inertial floating uses the static atlas');
 send('tts:started',{character_id:'nana'});tree=render();assert.match(stage(tree).props.className,/pose-drag/,'speech cannot replace inertial floating');
 send('chat:waiting',{character_id:'nana',stream_id:'fling-wait'});
 handle.setExpression('dizzy',1800);tree=render();assert.match(stage(tree).props.className,/pose-dizzy/,'collision dizziness overrides floating even while waiting for a reply');
 assert.equal(sprite(tree).props.style.backgroundPosition,'0% 100%','dizzy uses its own atlas cell');
 send('chat:error',{character_id:'nana',stream_id:'fling-wait'});handle.resetExpression();
 tree=render();assert.match(stage(tree).props.className,/pose-drag/,'after dizziness clears an ongoing flight resumes floating');
 send('drag:fling',{character_id:'nana',generation:2,active:true});
 send('drag:fling',{character_id:'nana',generation:1,active:false});
 tree=render();assert.match(stage(tree).props.className,/pose-drag/,'an old flight cannot clear its successor');
 send('drag:fling',{character_id:'nana',generation:2,active:false});
 tree=render();assert.doesNotMatch(stage(tree).props.className,/pose-drag/,'finishing inertia restores normal presentation');
 send('drag:fling',{character_id:'nana',generation:2,active:true});
 tree=render();assert.doesNotMatch(stage(tree).props.className,/pose-drag/,'a delayed start cannot revive a finished flight');
 // Effects are deliberately not flushed by render(): the very first render of
 // the new loop must already use frame zero, rather than an old atlas slot.
 speechStore.currentBubble=null;speechStore.settledBubbles=[];
 send('presentation:stop',{speaker_id:'nana'});tree=render();
 handle.resetExpression();handle.setExpression('happy');tree=render();
 assert.equal(sprite(tree).props.style.backgroundPosition,'50% 0%');
 handle.setExpression('music');tree=render();
 assert.match(stage(tree).props.className,/pose-music/);
 assert.equal(sprite(tree).props.style.backgroundPosition,'0% 0%','loop entry resets its index in the same commit as the new pose');
 cleanup();assert.equal(listeners.size,0);
 console.log('Pet speaking sprites: actual atlas cell, silent/proactive bubbles, ongoing audio after chat done, settled/sticker distinction, thinking priority, role isolation and physical actions passed');
}finally{for(const name of names){if(prior[name]===undefined)delete globalThis[name];else globalThis[name]=prior[name];}delete globalThis.__speechPet;}
