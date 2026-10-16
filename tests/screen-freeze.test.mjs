import assert from 'node:assert/strict';
import { build } from 'esbuild';

const states=[], refs=[], effects=[], calls=[], listeners=new Map(), revoked=[], paints=[];
let slot=0, refSlot=0, first=true, finishResize, finishAnalysis;
const flush=()=>new Promise(resolve=>setImmediate(resolve));
const native={
  async startDragging(){calls.push({name:'drag'});},
  async close(){calls.push({name:'close'});},
  async show(){calls.push({name:'show'});},
};
const element={addEventListener(name,fn,options){listeners.set(name,fn);if(name==='wheel')assert.equal(options.passive,false);},removeEventListener(name){listeners.delete(name);}};
globalThis.__freeze={
  state(value){const i=slot++;if(first)states[i]=value;return [states[i],v=>states[i]=v];},
  ref(value){const i=refSlot++;if(first)refs[i]={current:value};return refs[i];},
  effect(fn){effects.push(fn);},
  native,
  invoke(name,args){
    calls.push({name,args});
    if(name==='screen_freeze_frame')return Promise.resolve(new Uint8Array([137,80,78,71]).buffer);
    if(name==='screen_freeze_resize')return new Promise(resolve=>{finishResize=()=>resolve(args.scale);});
    if(name==='screen_freeze_save')return Promise.resolve('C:/Pictures/Screenshot.png');
    if(name==='screen_freeze_analyze')return new Promise((resolve,reject)=>{finishAnalysis={resolve,reject};});
    return Promise.resolve('zh-CN');
  },
};
const original={window:globalThis.window,create:URL.createObjectURL,revoke:URL.revokeObjectURL,raf:globalThis.requestAnimationFrame};
globalThis.requestAnimationFrame=fn=>paints.push(fn);
const paint=()=>{const pending=paints.splice(0);pending.forEach(fn=>fn());};
globalThis.window={innerWidth:320,innerHeight:240,addEventListener(name,fn){listeners.set(name,fn);},removeEventListener(name){listeners.delete(name);}};
URL.createObjectURL=()=> 'blob:frozen';URL.revokeObjectURL=value=>revoked.push(value);
const fixtures={
  react:`export const useState=v=>globalThis.__freeze.state(v),useRef=v=>globalThis.__freeze.ref(v),useEffect=f=>globalThis.__freeze.effect(f);`,
  'react/jsx-runtime':`export const jsx=(type,props)=>({type,props}),jsxs=jsx;`,
  'react-i18next':`export const useTranslation=()=>({i18n:{language:'zh-CN',changeLanguage:async()=>{}}});`,
  'lucide-react':`export const Download=()=>null,Sparkles=()=>null,X=()=>null;`,
  '@tauri-apps/api/core':`export const invoke=(...args)=>globalThis.__freeze.invoke(...args);`,
  '@tauri-apps/api/window':`export const getCurrentWindow=()=>globalThis.__freeze.native;`,
  '@tauri-apps/api/event':`export const listen=async()=>()=>{};`,
};
const find=(node,match)=>{if(!node||typeof node!=='object')return null;if(match(node))return node;for(const child of [node.props?.children].flat(Infinity)){const value=find(child,match);if(value)return value;}return null;};
const text=node=>node==null?'':Array.isArray(node)?node.map(text).join(''):typeof node==='object'?text(node.props?.children):String(node);
try {
  const bundle=await build({entryPoints:['src/components/ScreenFreezeWindow.tsx'],bundle:true,write:false,platform:'node',format:'esm',loader:{'.css':'empty'},plugins:[{name:'freeze-fixtures',setup(b){
    b.onResolve({filter:/^react$|^react\/jsx-runtime$|^react-i18next$|^lucide-react$|^@tauri-apps\/api\//},({path})=>({path,namespace:'fixture'}));
    b.onLoad({filter:/.*/,namespace:'fixture'},({path})=>({contents:fixtures[path]}));
  }}]});
  const {default:Freeze}=await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
  const render=()=>{slot=0;refSlot=0;effects.length=0;const tree=Freeze();first=false;tree.props.ref.current=element;return tree;};
  render();const cleanup=effects.map(effect=>effect());await flush();
  let tree=render();const image=find(tree,n=>n.type==='img');assert.equal(image.props.src,'blob:frozen');
  assert.equal(calls.filter(c=>c.name==='screen_freeze_resize').length,0,'hidden until image decodes');
  image.props.onLoad();
  assert.equal(calls.filter(c=>c.name==='screen_freeze_present').length,0,'do not reveal before decoded image paints');
  paint();paint();await flush();
  assert.equal(calls.at(-1).name,'screen_freeze_present');
  assert.equal(calls.filter(c=>c.name==='screen_freeze_resize').length,0,'initial presentation preserves the exact prepared region');
  const wheel=deltaY=>listeners.get('wheel')({deltaY,preventDefault(){}});
  wheel(-100);wheel(-100);wheel(-100);
  assert.equal(calls.filter(c=>c.name==='screen_freeze_resize').length,1,'no overlapping native resizes');
  finishResize();await flush();assert.ok(calls.at(-1).args.scale>1.8,'pending wheel events coalesce to the latest zoom');
  finishResize();await flush();
  wheel(100);assert.ok(calls.at(-1).args.scale<1.8,'scroll down shrinks');finishResize();await flush();
  const edge=find(tree,n=>n.props?.className==='screen-freeze-edge edge-nw');
  assert.ok(edge,'corner resize handle is available');
  let captured=false;
  const pointer=(x,y)=>({button:0,isPrimary:true,pointerId:7,screenX:x,screenY:y,stopPropagation(){},preventDefault(){},currentTarget:{setPointerCapture(){captured=true;},hasPointerCapture(){return captured;},releasePointerCapture(){captured=false;}}});
  edge.props.onPointerDown(pointer(100,100));assert.equal(captured,true);
  const beforeEdge=calls.length;
  edge.props.onPointerMove(pointer(68,76));
  assert.equal(calls.at(-1).args.edge,'nw');
  const dragScale=calls.at(-1).args.scale;
  assert.ok(dragScale>1.6,'pulling the top-left corner outward enlarges');
  assert.equal(calls.filter(c=>c.name==='drag').length,0,'edge resize never starts moving the window');
  wheel(-100);assert.equal(calls.length,beforeEdge+1,'wheel does not compete with edge dragging');
  edge.props.onPointerUp(pointer(68,76));assert.equal(captured,false);
  finishResize();await flush();
  wheel(100);assert.equal(calls.at(-1).args.edge,null,'wheel resumes regular zoom after edge resizing');finishResize();await flush();
  tree.props.onPointerDown({button:2});assert.notEqual(calls.at(-1).name,'drag');
  tree.props.onPointerDown({button:0});assert.equal(calls.at(-1).name,'drag');
  const openMenu=()=>{tree=render();tree.props.onContextMenu({clientX:319,clientY:239,preventDefault(){}});tree=render();return find(tree,n=>n.props?.role==='menu');};
  let menu=openMenu();assert.deepEqual(menu.props.style,{left:166,top:126});
  assert.deepEqual(menu.props.children.map(text),['保存','关闭','分析']);
  menu.props.children[0].props.onClick();await flush();tree=render();assert.ok(text(tree).includes('C:/Pictures/Screenshot.png'));
  menu=openMenu();menu.props.children[2].props.onClick();menu.props.children[2].props.onClick();
  assert.equal(calls.filter(c=>c.name==='screen_freeze_analyze').length,1,'duplicate analysis is blocked');
  finishAnalysis.reject('视觉功能未启用');await flush();tree=render();assert.ok(text(tree).includes('视觉功能未启用'));
  menu=openMenu();assert.equal(menu.props.children[2].props.disabled,false,'analysis errors allow retry');
  menu.props.children[1].props.onClick();await flush();assert.equal(calls.at(-1).name,'close');
  cleanup.forEach(fn=>fn?.());assert.deepEqual(revoked,['blob:frozen']);assert.equal(listeners.size,0);
  console.log('Frozen snapshots: decoded presentation, serialized wheel zoom, drag, bounded menu, save, analysis retry, close and pixel cleanup passed');
} finally {
  if(original.window===undefined)delete globalThis.window;else globalThis.window=original.window;
  URL.createObjectURL=original.create;URL.revokeObjectURL=original.revoke;delete globalThis.__freeze;
  if(original.raf===undefined)delete globalThis.requestAnimationFrame;else globalThis.requestAnimationFrame=original.raf;
}
