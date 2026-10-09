import assert from 'node:assert/strict';
import { build } from 'esbuild';
const effects=[], calls=[], events=new Map(), pending=[], frames=new Map();
let frameId=0;
const oldRaf=globalThis.requestAnimationFrame, oldCancel=globalThis.cancelAnimationFrame;
globalThis.requestAnimationFrame=callback=>{const id=++frameId;frames.set(id,callback);return id;};
globalThis.cancelAnimationFrame=id=>frames.delete(id);
const paint=()=>{const callbacks=[...frames.values()];frames.clear();callbacks.forEach(callback=>callback());};
const oldDocument=globalThis.document,oldWindow=globalThis.window;
globalThis.document={documentElement:{dataset:{}}};globalThis.window={innerWidth:60};
globalThis.__edgeTheme={
 effects,
 async listen(name,callback){events.set(name,callback);return ()=>events.delete(name);},
 invoke(name,args){calls.push({name,args});if(name==='get_config')return new Promise(resolve=>pending.push(resolve));return Promise.resolve({quiet:false,dialogue:false});},
};
const flush=()=>new Promise(resolve=>setImmediate(resolve));
const send=(name,payload)=>events.get(name)({payload});
try{
 const fixtures={
  react:`export const useRef=v=>({current:v}),useState=v=>[v,()=>{}],useCallback=f=>f,useEffect=f=>globalThis.__edgeTheme.effects.push(f);`,
  'react/jsx-runtime':`export const jsx=(type,props)=>({type,props}),jsxs=jsx;`,
  'react-i18next':`export const useTranslation=()=>({i18n:{language:'zh'}});`,
  '@tauri-apps/api/core':`export const invoke=(...args)=>globalThis.__edgeTheme.invoke(...args);`,
  '@tauri-apps/api/event':`export const listen=(...args)=>globalThis.__edgeTheme.listen(...args),emit=async()=>{};`,
  './ChatEdgeMenu':`export default function Menu(){return null;}`,
 };
 const bundle=await build({entryPoints:['src/components/EdgeMenuWindow.tsx'],bundle:true,write:false,platform:'node',format:'esm',plugins:[{name:'edge-theme-fixtures',setup(b){b.onResolve({filter:/^react$|^react\/jsx-runtime$|^react-i18next$|^@tauri-apps\/api\/|^\.\/ChatEdgeMenu$/},({path})=>({path,namespace:'fixture'}));b.onLoad({filter:/.*/,namespace:'fixture'},({path})=>({contents:fixtures[path]}));}}]});
 const {default:Window}=await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);Window();const cleanup=effects[0]();await flush();
 assert.ok(events.has('config:theme-changed')&&events.has('edge_menu:shown'));
 send('config:theme-changed',{theme:'light'});assert.equal(document.documentElement.dataset.theme,'light');
 pending.shift()('dark');await flush();assert.equal(document.documentElement.dataset.theme,'light','late initial config cannot overwrite a live theme change');
 assert.equal(calls.filter(call=>call.name==='edge_menu_ready').length,0);
 paint();paint();await flush();assert.equal(calls.filter(call=>call.name==='edge_menu_ready').length,1,'show only after listeners, theme, and paint are ready');
 send('edge_menu:shown',{theme:'dark'});assert.equal(document.documentElement.dataset.theme,'dark','reopening catches theme changes missed while suspended');
 send('config:theme-changed',{theme:'system'});assert.equal(document.documentElement.dataset.theme,'system','system theme delegates to existing light/dark media queries');
 send('config:saved',{});send('config:theme-changed',{theme:'light'});pending.shift()('dark');await flush();assert.equal(document.documentElement.dataset.theme,'light');
 send('edge_menu:shown',{});pending.shift()('dark');await flush();assert.equal(document.documentElement.dataset.theme,'dark');
 send('config:saved',{});const late=pending.shift();cleanup();late('light');await flush();assert.equal(document.documentElement.dataset.theme,'dark');assert.equal(events.size,0,'all subscriptions are disposed');
 assert.ok(calls.filter(call=>call.name==='get_config').every(call=>call.args.key==='base.theme'));
 console.log('Edge menu theme: correct config key, live light/dark/system, reopen after suspension, stale responses and listener cleanup passed');
}finally{
 if(oldDocument===undefined)delete globalThis.document;else globalThis.document=oldDocument;
 if(oldWindow===undefined)delete globalThis.window;else globalThis.window=oldWindow;
 if(oldRaf===undefined)delete globalThis.requestAnimationFrame;else globalThis.requestAnimationFrame=oldRaf;
 if(oldCancel===undefined)delete globalThis.cancelAnimationFrame;else globalThis.cancelAnimationFrame=oldCancel;
 delete globalThis.__edgeTheme;
}
