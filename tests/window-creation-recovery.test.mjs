import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {transform} from 'esbuild';
import vm from 'node:vm';
const source=await readFile('src/App.tsx','utf8');
const start=source.indexOf('async function openWindow(');
const end=source.indexOf('/* ============ 主应用',start);
assert.ok(start>=0&&end>start);
const code=(await transform(source.slice(start,end),{loader:'ts',format:'cjs'})).code;
async function scenario(prewarm,failures){
 const cache=new Map(),closing=new Set(),pending=new Map();let count=0;
 class Window {
  static async getByLabel(){return null;}
  constructor(label){this.label=label;this.callbacks={};const fail=count++<failures;setTimeout(()=>{for(const fn of this.callbacks[fail?'tauri://error':'tauri://created']??[])fn({payload:'WebView2 disconnected'});},0);}
  once(event,fn){(this.callbacks[event]??=[]).push(fn);return Promise.resolve(()=>{});}
  onCloseRequested(){return Promise.resolve(()=>{});}
  async show(){} async isVisible(){return true;}
 }
 const context={WebviewWindow:Window,CHILD_WINDOWS:cache,CLOSE_CLEANUP_REGISTERED:closing,WINDOW_CREATION:pending,RAISE_UNLISTEN:new Map(),SHARED_SUBWINDOWS:new Set(['memory']),charScopedLabel:v=>v,getCharacterId:()=>null,raiseWindow:async()=>{},armSelfRevealFallback:()=>{},WINDOW_CREATION_TIMEOUT_MS:3000,window:{setTimeout,clearTimeout},screen:{width:100,height:100},console:{error:()=>{}}};
 vm.createContext(context);vm.runInContext(code,context);
 const result=await context.openWindow('memory','memory','Memory',100,100,{prewarm});
 return {result,count,cache,closing,pending};
}
let state=await scenario(false,1);assert.ok(state.result);assert.equal(state.count,2);assert.equal(state.cache.size,1);assert.equal(state.pending.size,0);
state=await scenario(false,2);assert.equal(state.result,null);assert.equal(state.count,2);assert.equal(state.cache.size,0);assert.equal(state.closing.size,0);
state=await scenario(true,1);assert.equal(state.result,null);assert.equal(state.count,1);assert.equal(state.cache.size,0);
console.log('Window creation failure: cache cleanup, one bounded retry, cancelled prewarm stays hidden: passed');
