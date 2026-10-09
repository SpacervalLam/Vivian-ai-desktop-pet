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
 b.onResolve({filter:/^react$|^react\/jsx-runtime$|^@tauri-apps\/api\/|\/characterContext$|\/useAppStore$|\/sheetLoader$/},({path})=>({path,namespace:'fixture'}));
 b.onLoad({filter:/.*/,namespace:'fixture'},({path})=>({contents:
 path==='react'?`export const forwardRef=fn=>fn,useState=v=>globalThis.__speechPet.state(v),useRef=v=>globalThis.__speechPet.ref(v),useCallback=f=>f,useEffect=f=>globalThis.__speechPet.effect(f),useImperativeHandle=(ref,create)=>globalThis.__speechPet.setHandle(create());`:
 path==='react/jsx-runtime'?`export const jsx=(type,props)=>({type,props}),jsxs=jsx;`:
 path.endsWith('characterContext')?`export const getCharacterId=()=>"nana";`:
 path.endsWith('useAppStore')?`export const useAppStore=selector=>selector(globalThis.__speechPet.store);useAppStore.getState=()=>globalThis.__speechPet.store;`:
 path.endsWith('sheetLoader')?`export class SheetLoader{load(){return Promise.resolve(false)}isReady(){return false}clear(){}}`:
 path.endsWith('/event')?`export const listen=(...args)=>globalThis.__speechPet.listen(...args);`:
 path.endsWith('/window')?`export const currentMonitor=async()=>null,getCurrentWindow=()=>({});`:
 `export const invoke=async()=>null;`
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
 send('chat:waiting',{character_id:'nana',stream_id:'new'});send('tts:started',{character_id:'nana'});tree=render();assert.match(stage(tree).props.className,/pose-talk/,'playback takes precedence over a queued thinking reply');
 send('presentation:stop',{speaker_id:'nana'});tree=render();assert.doesNotMatch(stage(tree).props.className,/pose-talk/);
 send('chat:error',{character_id:'nana',stream_id:'new'});send('tts:started',{character_id:'nana'});
 handle.setExpression('drag');tree=render();assert.match(stage(tree).props.className,/pose-drag/,'dragging is not overwritten by speech');
 handle.setExpression('walk');tree=render();assert.match(stage(tree).props.className,/pose-walk/,'walking sprite and movement stay synchronized');
 cleanup();assert.equal(listeners.size,0);
 console.log('Pet speaking sprites: actual atlas cell, silent/proactive bubbles, ongoing audio after chat done, settled/sticker distinction, thinking priority, role isolation and physical actions passed');
}finally{for(const name of names){if(prior[name]===undefined)delete globalThis[name];else globalThis[name]=prior[name];}delete globalThis.__speechPet;}
