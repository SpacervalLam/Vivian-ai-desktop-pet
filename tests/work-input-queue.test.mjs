import assert from 'node:assert/strict';
import { build, transform } from 'esbuild';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';
let slots=[], effects=[], cursor=0, rendered;
const timers=new Map(), listeners=new Set();
const same=(a,b)=>a&&b&&a.length===b.length&&a.every((v,i)=>Object.is(v,b[i]));
globalThis.__workInput={
 state(initial){const i=cursor++;if(!(i in slots))slots[i]=typeof initial==='function'?initial():initial;return [slots[i],value=>{slots[i]=typeof value==='function'?value(slots[i]):value;}];},
 ref(initial){const i=cursor++;return slots[i]??={current:initial};},
 memo(fn,deps){const i=cursor++;if(!same(slots[i]?.deps,deps))slots[i]={value:fn(),deps};return slots[i].value;},
 effect(fn,deps){const i=cursor++;if(!same(effects[i]?.deps,deps))effects[i]={fn,deps,pending:true,cleanup:effects[i]?.cleanup};},
 invoke:async()=>{},listen:async(name,callback)=>{assert.equal(name,'coding:input_queue');listeners.add(callback);return()=>listeners.delete(callback);},
};
globalThis.window={setInterval(fn){const id=timers.size+1;timers.set(id,fn);return id;},clearInterval:id=>timers.delete(id)};
globalThis.document={visibilityState:'visible'};
const mocks={
 react:`const h=globalThis.__workInput;export const useState=h.state,useRef=h.ref,useEffect=h.effect,useCallback=(fn,deps)=>h.memo(()=>fn,deps);`,
 'react/jsx-runtime':`export const jsx=(type,props)=>({type,props}),jsxs=jsx;`,
 'react-i18next':`export const useTranslation=()=>({t:key=>key});`,
 '@tauri-apps/api/core':`export const invoke=(...args)=>globalThis.__workInput.invoke(...args);`,
 '@tauri-apps/api/event':`export const listen=(...args)=>globalThis.__workInput.listen(...args);`,
 'lucide-react':`export const ArrowUp='icon',Pencil='icon',Trash2='icon';`,
};
async function load(entry){const result=await build({entryPoints:[entry],bundle:true,write:false,format:'esm',platform:'node',plugins:[{name:'queue-fixture',setup(b){
 b.onResolve({filter:/^react$|^react\/jsx-runtime$|^react-i18next$|^@tauri-apps\/api\/|^lucide-react$/},({path})=>({path,namespace:'fixture'}));
 b.onResolve({filter:/workInputStrings$/},()=>({path:'strings',namespace:'fixture'}));
 b.onLoad({filter:/\.css$/},()=>({contents:'',loader:'js'}));
 b.onLoad({filter:/.*/,namespace:'fixture'},({path})=>({contents:mocks[path]??''}));
}}]});return(await import('data:text/javascript;base64,'+Buffer.from(result.outputFiles[0].text).toString('base64'))).default;}
const useQueue=await load('src/components/mind-inspector/pages/useWorkInputQueue.ts');
let sessionId='a';
let Component=()=>useQueue(sessionId);
const render=()=>{cursor=0;rendered=Component();for(const effect of effects.filter(Boolean))if(effect.pending){effect.pending=false;effect.cleanup?.();effect.cleanup=effect.fn();}return rendered;};
const settle=async()=>{for(let i=0;i<15;i++){await Promise.resolve();render();}};
const reset=()=>{for(const effect of effects.filter(Boolean))effect.cleanup?.();slots=[];effects=[];cursor=0;};
const q=(revision,content='message')=>({revision,paused:false,items:[{id:'queued',kind:'follow_up',payload:{content,images:[],file_refs:[]}}]});
let resolveA, resolveB;
globalThis.__workInput.invoke=async(command,args)=>{assert.equal(command,'coding_input_queue');return new Promise(resolve=>{if(args.sessionId==='a')resolveA=resolve;else resolveB=resolve;});};
render();await settle(); sessionId='b';render();await settle();
resolveB(q(3,'session b'));await settle();resolveA(q(100,'wrong session'));await settle();
assert.equal(rendered.queue.items[0].payload.content,'session b','old session replies cannot replace the active queue');
for(const handler of listeners)handler({payload:{session_id:'a',queue:q(999,'wrong event')}});await settle();
assert.equal(rendered.queue.revision,3);
for(const handler of listeners)handler({payload:{session_id:'b',queue:q(8,'latest')}});await settle();
globalThis.__workInput.invoke=async()=>q(4,'stale poll'); [...timers.values()][0]();await settle();
assert.equal(rendered.queue.items[0].payload.content,'latest','stale polling cannot overwrite newer queue events');
const calls=[];let fails=true;
globalThis.__workInput.invoke=async(command,args)=>{calls.push(args);if(fails)throw Error('disk unavailable');return q(10);};
const payload={content:'with attachments',images:[{media_type:'image/png',data:'BASE64',name:'image.png'}],file_refs:[{path:'src/a.rs'}]};
await assert.rejects(rendered.enqueue(payload));await settle();assert.match(rendered.error,/disk unavailable/);
fails=false;await rendered.enqueue(payload);await settle();
assert.equal(calls[0].id,calls[1].id,'retries use the same durable input identity even if the first response was lost');
assert.deepEqual(calls[1].images,payload.images);assert.deepEqual(calls[1].fileRefs,payload.file_refs);
let finishAction;
globalThis.__workInput.invoke=async(_,args)=>args.action==='list'?q(1):new Promise(resolve=>{finishAction=resolve;});
const action=rendered.action('pause');await settle();assert.ok(rendered.busy);
sessionId='a';render();await settle();assert.equal(rendered.busy,false);
finishAction(q(12));await action;await settle();assert.ok(!rendered.error.includes('disk unavailable'));
reset();assert.equal(timers.size,0);assert.equal(listeners.size,0);
let finishListen,released=0;
globalThis.__workInput.listen=async()=>new Promise(resolve=>{finishListen=resolve;});
render();reset();finishListen(()=>released++);for(let i=0;i<10;i++)await Promise.resolve();assert.equal(released,1,'late subscriptions are released after unmount');

const Panel=await load('src/components/mind-inspector/pages/WorkInputQueue.tsx');
let edited;
const controller={queue:q(1),busy:false,error:'',action:async(...args)=>{edited=args;}};
controller.queue.items[0].payload.images=payload.images;
Component=()=>Panel({controller});render();
const walk=n=>Array.isArray(n)?n.flatMap(walk):!n||typeof n!=='object'?[]:[n,...walk(n.props?.children)];
const text=n=>typeof n==='string'?n:Array.isArray(n)?n.map(text).join(''):text(n?.props?.children??'');
const button=label=>{const found=walk(rendered).find(n=>n.type==='button'&&(n.props['aria-label']===label||text(n)===label));assert.ok(found,label);return found;};
button('workInput.edit').props.onClick();await settle();
walk(rendered).find(n=>n.type==='textarea').props.onChange({target:{value:'updated'}});await settle();
button('workInput.save').props.onClick();await settle();assert.equal(edited[0],'edit');assert.equal(edited[1],'queued');assert.equal(edited[2].content,'updated');
controller.queue.paused=true;render();button('workInput.resume').props.onClick();await settle();assert.equal(edited[0],'resume');
reset();

const office=(await readFile('src/components/mind-inspector/pages/CodeAgentPageNew.tsx','utf8')).replaceAll('\r\n','\n');
const send=office.indexOf('  const handleSend = useCallback('), start=office.indexOf('    if (running) {',send),end=office.indexOf('    // 无工作模型',start);
assert.ok(start>0&&end>start);
const branch=(await transform(`async function send(){${office.slice(start,end)}}`,{loader:'ts',format:'cjs'})).code;
let queued,cleared=0,revoked=0,notify=[];
const images=[{file:{type:'image/png',name:'a.png'},previewUrl:'blob:sample'}],refs=[{path:'src/a.rs'}];
const context={running:true,text:'queued text',input:'queued text',draftImages:images,draftRefs:refs,activeIdRef:{current:'a'},
 viewDraftsRef:{current:new Map([['a',{text:'queued text',images,refs}]])},
 composerDraftRef:{current:{text:'queued text',images,refs}},fileToBase64:async()=> 'BASE64',
 pendingInputs:{enqueue:async p=>{queued=p;}},notifyError:error=>notify.push(error),
 setInput:()=>cleared++,setDraftImages:()=>cleared++,setDraftRefs:()=>cleared++,URL:{revokeObjectURL:()=>revoked++},Promise};
vm.createContext(context);vm.runInContext(branch,context);await context.send();
assert.equal(queued.images[0].data,'BASE64');assert.equal(queued.file_refs[0].path,'src/a.rs');assert.equal(cleared,3);assert.equal(revoked,1);
cleared=0;revoked=0;context.pendingInputs.enqueue=async()=>{throw Error('failed');};await context.send();assert.equal(cleared,0);assert.equal(revoked,0);assert.equal(notify.length,1);
context.pendingInputs.enqueue=async()=>{context.activeIdRef.current='b';};await context.send();assert.equal(cleared,0,'late enqueue acknowledgements must not erase another session’s draft');
assert.equal(context.viewDraftsRef.current.get('a').text,'','an acknowledged background draft must not reappear as unsent input');
const eventSlice = marker => { const start=office.indexOf(`      await add('${marker}',`); const end=office.indexOf('      await add(',start+20);assert.ok(start>0&&end>start);return office.slice(start,end);};
const handlers=new Map();let finishRefresh,shown=[],isRunning=true;
const eventsContext={inputEpoch:0,Date,activeIdRef:{current:'a'},sessionSelection:{current:0},
 add:async(name,handler)=>handlers.set(name,handler),isMine:p=>p.session_id==='a',append:msg=>shown.push(msg),setRunning:value=>{isRunning=value;},
 setSessions:()=>{},setMessages:value=>{shown=value;},setStats:()=>{},setDeliverables:()=>{},setThinking:()=>{},setThinkingText:()=>{},setCompacting:()=>{},setStreamingText:()=>{},
 refreshSessions:async()=>new Promise(resolve=>{finishRefresh=resolve;}),invoke:async()=>[{session_id:'a',status:'idle',messages:[{content:'stale previous turn'}]}],notifyError:assert.fail};
vm.createContext(eventsContext);
const eventsCode=(await transform(`(async()=>{${eventSlice('coding:user_message')}${eventSlice('coding:turn_done')}})()`,{loader:'ts',format:'cjs'})).code;
await vm.runInContext(eventsCode,eventsContext);
handlers.get('coding:turn_done')({session_id:'a',status:'idle'});
handlers.get('coding:user_message')({session_id:'a',queued_id:'next',content:'new queued task'});
finishRefresh();for(let i=0;i<12;i++)await Promise.resolve();
assert.ok(isRunning,'late previous-turn hydration cannot stop the new queued turn');
assert.equal(shown[0].content,'new queued task');

console.log('Work input queue: session/revision races, idempotent retries, attachments, inline editing, pause/resume, subscription cleanup and draft preservation passed.');
