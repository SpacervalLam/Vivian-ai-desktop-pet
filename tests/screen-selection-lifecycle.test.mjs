import assert from 'node:assert/strict';
import { build } from 'esbuild';
const states=[], refs=[], effects=[], events=new Map(), requests=new Map(), calls=[], urls=[], revoked=[], frames=new Map(), keys=new Map();
let cursor=0, refCursor=0, firstRender=true, frameId=0;
const fixtures={
 react:`export const useState=v=>globalThis.__selector.state(v),useRef=v=>globalThis.__selector.ref(v),useCallback=f=>f,useEffect=f=>globalThis.__selector.effect(f);`,
 'react/jsx-runtime':`export const jsx=(type,props)=>({type,props}),jsxs=jsx;`,
 'react-i18next':`export const useTranslation=()=>({i18n:{language:'zh'}});`,
 'lucide-react':`export const Copy=()=>null,Download=()=>null,ScanLine=()=>null,Sparkles=()=>null,X=()=>null;`,
 '@tauri-apps/api/core':`export const invoke=(...args)=>globalThis.__selector.invoke(...args);`,
 '@tauri-apps/api/event':`export const listen=(name,fn)=>globalThis.__selector.listen(name,fn);`,
};
const prior={window:globalThis.window,requestAnimationFrame:globalThis.requestAnimationFrame,cancelAnimationFrame:globalThis.cancelAnimationFrame,create:URL.createObjectURL,revoke:URL.revokeObjectURL};
globalThis.__selector={
 state(v){const index=cursor++;if(firstRender)states[index]=v;return [states[index],value=>states[index]=typeof value==='function'?value(states[index]):value];},
 ref(v){const index=refCursor++;if(firstRender)refs[index]={current:v};return refs[index];},
 effect(f){effects.push(f);},
 invoke(name,args){calls.push({name,args});if(name==='screen_selection_frame')return new Promise(resolve=>requests.set(args.sessionId,resolve));return Promise.resolve();},
 async listen(name,fn){events.set(name,fn);return ()=>{if(events.get(name)===fn)events.delete(name);};},
};
globalThis.window={addEventListener(name,fn){keys.set(name,fn);},removeEventListener(name){keys.delete(name);}};
globalThis.requestAnimationFrame=f=>{const id=++frameId;frames.set(id,f);return id;};
globalThis.cancelAnimationFrame=id=>frames.delete(id);
URL.createObjectURL=blob=>{assert.equal(blob.type,'image/png');const url=`blob:selector-${urls.length}`;urls.push(url);return url;};
URL.revokeObjectURL=url=>revoked.push(url);
const flush=async()=>{await new Promise(resolve=>setImmediate(resolve));};
const paint=()=>{const pending=[...frames.values()];frames.clear();pending.forEach(f=>f());};
const find=(tree,predicate)=>{if(!tree||typeof tree!=='object')return null;if(predicate(tree))return tree;for(const child of [tree.props?.children].flat(Infinity)){const found=find(child,predicate);if(found)return found;}return null;};
try{
 const bundle=await build({entryPoints:['src/components/ScreenSelectionWindow.tsx'],bundle:true,write:false,platform:'node',format:'esm',loader:{'.css':'empty'},plugins:[{name:'selector-fixtures',setup(b){b.onResolve({filter:/^react$|^react\/jsx-runtime$|^react-i18next$|^lucide-react$|^@tauri-apps\/api\//},({path})=>({path,namespace:'fixture'}));b.onLoad({filter:/.*/,namespace:'fixture'},({path})=>({contents:fixtures[path]}));}}]});
 const {default:Selector}=await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
 const render=()=>{cursor=0;refCursor=0;effects.length=0;const tree=Selector();firstRender=false;return tree;};
 render();const cleanup=effects[0]();await flush();
 assert.ok(events.has('screen_selection:start')&&events.has('screen_selection:reset'));
 assert.equal(calls.at(-1).name,'screen_selection_ready','listeners installed before ready acknowledgement');
 const begin=id=>events.get('screen_selection:start')({payload:{session_id:id,width:1920,height:1080}});
 begin('old');begin('current');requests.get('old')(new Uint8Array([1,2,3]).buffer);await flush();
 assert.equal(urls.length,0,'stale preview never replaces current session');
 requests.get('current')(new Uint8Array([137,80,78,71]).buffer);await flush();
 let tree=render();const image=find(tree,n=>n.type==='img');assert.equal(image.props.src,'blob:selector-0');
 assert.equal(calls.filter(c=>c.name==='screen_selection_present').length,0,'no show before image decode');
 image.props.onLoad();paint();paint();await flush();assert.deepEqual(calls.at(-1),{name:'screen_selection_present',args:{sessionId:'current'}});
 // Complete a real drag before exercising the keyboard default.
 refs.find(ref=>ref===tree.props.ref).current={getBoundingClientRect:()=>({left:0,top:0,width:1920,height:1080})};
 const target={getBoundingClientRect:()=>({left:0,top:0,width:1920,height:1080}),setPointerCapture(){},hasPointerCapture(){return false;}};
 const pointer=(x,y)=>({clientX:x,clientY:y,button:0,isPrimary:true,pointerId:1,currentTarget:target,target:{closest(){return null;}},preventDefault(){}});
 tree.props.onPointerDown(pointer(20,30));tree.props.onPointerUp(pointer(220,130));
 tree=render();effects[1]();
 keys.get('keydown')({key:'Enter',preventDefault(){}});await flush();
 assert.deepEqual(calls.at(-1).args,{sessionId:'current',region:{x:20,y:30,width:200,height:100},action:'copy_and_analyze'});
 const oldSessionTree=tree;
 events.get('screen_selection:reset')({payload:{session_id:'old'}});assert.equal(revoked.length,0,'old reset cannot clear new preview');
 events.get('screen_selection:reset')({payload:{session_id:'current'}});assert.deepEqual(revoked,['blob:selector-0']);
 tree=render();assert.equal(find(tree,n=>n.type==='img'),null,'end of session drops screenshot pixels');
 const oldTree=oldSessionTree;
 begin('next');requests.get('next')([137,80,78,71]);await flush();tree=render();assert.equal(find(tree,n=>n.type==='img').props.src,'blob:selector-1','same mounted selector handles another screenshot');
 tree.props.ref.current={getBoundingClientRect:target.getBoundingClientRect};
 tree.props.onPointerDown(pointer(30,40));tree.props.onPointerUp(pointer(130,140));tree=render();
 const copyButton=find(tree,n=>n.type==='button'&&n.props.children?.some?.(child=>child?.props?.children==='复制到剪贴板'));
 assert.ok(copyButton,'copy-only action is available after selecting');
 copyButton.props.onClick();await flush();assert.equal(calls.at(-1).args.action,'copy');
 begin('escape');requests.get('escape')([137,80,78,71]);await flush();tree=render();
 effects[1]();keys.get('keydown')({key:'Escape',preventDefault(){}});await flush();
 assert.equal(calls.at(-1).args.action,'cancel');
 const beforeCancel=calls.length;oldTree.props.onContextMenu({preventDefault(){}});await flush();assert.equal(calls.length,beforeCancel,'old UI callbacks cannot cancel the new session');
 begin('late');cleanup();requests.get('late')([1]);await flush();assert.equal(urls.length,3);assert.equal(events.size,0);assert.ok(revoked.includes('blob:selector-2'));
 console.log('Reusable selector: ready ordering, binary preview, stale sessions, decoded-image presentation, pixel cleanup, reuse and unmount passed');
}finally{
 for(const key of ['window','requestAnimationFrame','cancelAnimationFrame']){if(prior[key]===undefined)delete globalThis[key];else globalThis[key]=prior[key];}
 URL.createObjectURL=prior.create;URL.revokeObjectURL=prior.revoke;delete globalThis.__selector;
}
