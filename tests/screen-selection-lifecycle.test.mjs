import assert from 'node:assert/strict';
import { build } from 'esbuild';
const states=[], refs=[], effects=[], events=new Map(), requests=new Map(), calls=[], urls=[], revoked=[], frames=new Map(), keys=new Map();
let cursor=0, refCursor=0, firstRender=true, frameId=0, clock=0;
const fixtures={
 react:`export const useState=v=>globalThis.__selector.state(v),useRef=v=>globalThis.__selector.ref(v),useCallback=f=>f,useEffect=f=>globalThis.__selector.effect(f);`,
 'react/jsx-runtime':`export const Fragment=Symbol.for("fragment");export const jsx=(type,props)=>({type,props}),jsxs=jsx;`,
 'react-i18next':`export const useTranslation=()=>({i18n:globalThis.__selector.language});`,
 'lucide-react':`export const ChevronDown=()=>null,Copy=()=>null,Download=()=>null,ScanLine=()=>null,Sparkles=()=>null,X=()=>null;`,
 '@tauri-apps/api/core':`export const invoke=(...args)=>globalThis.__selector.invoke(...args);`,
 '@tauri-apps/api/event':`export const listen=(name,fn)=>globalThis.__selector.listen(name,fn);`,
};
const prior={window:globalThis.window,performance:globalThis.performance,requestAnimationFrame:globalThis.requestAnimationFrame,cancelAnimationFrame:globalThis.cancelAnimationFrame,create:URL.createObjectURL,revoke:URL.revokeObjectURL};
globalThis.__selector={
 hover:null,
 watchActive:null,
 language:{language:'zh',async changeLanguage(language){this.language=language;}},
 state(v){const index=cursor++;if(firstRender)states[index]=v;return [states[index],value=>states[index]=typeof value==='function'?value(states[index]):value];},
 ref(v){const index=refCursor++;if(firstRender)refs[index]={current:v};return refs[index];},
 effect(f){effects.push(f);},
 invoke(name,args){calls.push({name,args});if(name==='get_config')return Promise.resolve('ja');if(name==='list_characters')return Promise.resolve({active_id:'vivian',characters:[{id:'vivian',name:'Vivian',online:true},{id:'nana',name:'Nana',online:true},{id:'deepseek',name:'DeepSeek',online:false},{id:'claude',name:'Claude',online:true}]});if(name==='screen_watch_active')return Promise.resolve(globalThis.__selector.watchActive??null);if(name==='screen_selection_window_at')return Promise.resolve(globalThis.__selector.hover??null);if(name==='screen_selection_frame')return new Promise(resolve=>requests.set(args.sessionId,resolve));return Promise.resolve();},
 async listen(name,fn){events.set(name,fn);return ()=>{if(events.get(name)===fn)events.delete(name);};},
};
globalThis.window={addEventListener(name,fn){keys.set(name,fn);},removeEventListener(name){keys.delete(name);}};
globalThis.performance={now:()=>clock};
globalThis.requestAnimationFrame=f=>{const id=++frameId;frames.set(id,f);return id;};
globalThis.cancelAnimationFrame=id=>frames.delete(id);
URL.createObjectURL=blob=>{assert.equal(blob.type,'image/png');const url=`blob:selector-${urls.length}`;urls.push(url);return url;};
URL.revokeObjectURL=url=>revoked.push(url);
const flush=async()=>{await new Promise(resolve=>setImmediate(resolve));};
const paint=()=>{const pending=[...frames.values()];frames.clear();pending.forEach(f=>f());};
const find=(tree,predicate)=>{if(!tree||typeof tree!=='object')return null;if(predicate(tree))return tree;for(const child of [tree.props?.children].flat(Infinity)){const found=find(child,predicate);if(found)return found;}return null;};
try{
 const bundle=await build({stdin:{contents:`
   import {registerCharacters} from './src/characters';
   registerCharacters([{id:'deepseek',name:'DeepSeek'},{id:'claude',name:'Claude'}]);
   export {default} from './src/components/ScreenSelectionWindow';
 `,resolveDir:process.cwd()},bundle:true,write:false,platform:'node',format:'esm',loader:{'.css':'empty'},plugins:[{name:'selector-fixtures',setup(b){b.onResolve({filter:/^react$|^react\/jsx-runtime$|^react-i18next$|^lucide-react$|^@tauri-apps\/api\//},({path})=>({path,namespace:'fixture'}));b.onLoad({filter:/.*/,namespace:'fixture'},({path})=>({contents:fixtures[path]}));}}]});
 const {default:Selector}=await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
 const render=()=>{cursor=0;refCursor=0;effects.length=0;const tree=Selector();firstRender=false;return tree;};
 render();const cleanup=effects[0]();await flush();
 assert.ok(events.has('screen_selection:start')&&events.has('screen_selection:reset'));
 assert.equal(calls.at(-1).name,'screen_selection_ready','listeners installed before ready acknowledgement');
 assert.equal(globalThis.__selector.language.language,'ja','initial language comes from saved settings');
 assert.equal(find(render(),n=>n.type==='span').props.children,'ドラッグで選択');
 events.get('config:language-changed')({payload:{language:'en'}});
 assert.equal(find(render(),n=>n.type==='span').props.children,'Drag to select');
 events.get('config:language-changed')({payload:{language:'zh-CN'}});
 assert.equal(find(render(),n=>n.type==='span').props.children,'拖动选择区域');
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
 assert.deepEqual(calls.at(-1).args,{sessionId:'current',region:{x:20,y:30,width:200,height:100},action:'freeze'});
 const nodeText=node=>node==null||typeof node==='boolean'?'':Array.isArray(node)?node.map(nodeText).join(''):typeof node==='object'?nodeText(node.props?.children):String(node);
 const buttons=node=>!node||typeof node!=='object'?[]:[...(node.type==='button'?[node]:[]),...[node.props?.children].flat(Infinity).flatMap(buttons)];
 for(const [language,expected] of [['en',['Cancel','Save','Analyze','Copy to clipboard','Save & analyze','Freeze↵']],['ja',['キャンセル','保存','分析','クリップボードにコピー','保存して分析','画面を固定↵']],['zh-CN',['取消','保存','分析','复制到剪贴板','保存并分析','定格画面↵']]]){
   events.get('config:language-changed')({payload:{language}});
   tree=render();
   const toolbar=find(tree,n=>n.props?.className==='screen-selection-toolbar');
   assert.deepEqual(buttons(toolbar).filter(n=>!n.props['aria-haspopup']).map(nodeText),expected);
   assert.equal(find(tree,n=>n.type==='img').props.src,'blob:selector-0','language switching preserves the current screenshot');
 }
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
 for(const [action,label] of [['analyze','分析'],['save_and_analyze','保存并分析']]) {
   for(const character of ['vivian','nana','deepseek','claude',null]) {
     const id=`${action}-${character}`;begin(id);requests.get(id)([137,80,78,71]);await flush();tree=render();
     tree.props.ref.current={getBoundingClientRect:target.getBoundingClientRect};
     tree.props.onPointerDown(pointer(20,30));tree.props.onPointerUp(pointer(220,130));tree=render();
     const toggle=buttons(tree).find(n=>n.props['aria-label']===`${label} · 选择分析桌宠`);
     assert.ok(toggle,'analysis buttons expose a separate drawer');
     const before=calls.length;
     toggle.props.onClick({currentTarget:{getBoundingClientRect:()=>({right:1900,top:1020,bottom:1054})}});tree=render();
     let menu=find(tree,n=>n.props?.role==='menu');assert.ok(menu);assert.equal(calls.length,before,'opening the drawer does not submit the screenshot');
     assert.ok(menu.props.style.left+280<=1920&&menu.props.style.top+214<=1080,'four-character menu stays within the screen');
     assert.deepEqual(buttons(menu).map(button=>nodeText(button.props.children[1])),['Vivian','Nana','DeepSeek','Claude'].map(name=>`${action==='save_and_analyze'?'保存并':''}交由${name}分析`));
     effects[1]();keys.get('keydown')({key:'Escape',preventDefault(){}});tree=render();
     assert.equal(find(tree,n=>n.props?.role==='menu'),null);assert.equal(calls.length,before,'Escape dismisses the menu without cancelling capture');
     if(character) {
       toggle.props.onClick({currentTarget:{getBoundingClientRect:()=>({right:1900,top:1020,bottom:1054})}});tree=render();menu=find(tree,n=>n.props?.role==='menu');
       buttons(menu)[['vivian','nana','deepseek','claude'].indexOf(character)].props.onClick();await flush();
       assert.deepEqual(calls.at(-1).args,{sessionId:id,region:{x:20,y:30,width:200,height:100},action,analysisCharacter:character});
     } else {
       buttons(tree).find(n=>nodeText(n)===label).props.onClick();await flush();
       assert.equal(calls.at(-1).args.action,action);assert.equal(calls.at(-1).args.analysisCharacter,undefined,'main button leaves random selection to the backend');
     }
   }
 }
 const openWatch=async id=>{
   begin(id);requests.get(id)([137,80,78,71]);await flush();tree=render();
   tree.props.ref.current={getBoundingClientRect:target.getBoundingClientRect};tree=render();
   const hint=find(tree,n=>n.props?.className==='screen-selection-hint');
   assert.ok(buttons(hint).some(n=>nodeText(n)==='陪看'),'watch mode is available before selecting');
   buttons(hint).find(n=>nodeText(n)==='陪看').props.onClick();tree=render();
   tree.props.ref.current={getBoundingClientRect:target.getBoundingClientRect};
 };
 await openWatch('watch-window');
 globalThis.__selector.hover={title:'Editor',x:40,y:60,width:400,height:300};
 tree.props.onPointerMove(pointer(100,100));await flush();tree=render();
 assert.ok(find(tree,n=>n.props?.className==='screen-selection-hover'),'the first hover highlights immediately');
 const requestsAtPointer = () => calls.filter(call=>call.name==='screen_selection_window_at').length;
 const firstHoverRequests = requestsAtPointer();
 tree.props.onPointerMove(pointer(101,100));
 assert.equal(requestsAtPointer(),firstHoverRequests,'subsequent hover requests are throttled');
 clock += 80;
 tree.props.onPointerMove(pointer(102,100));await flush();tree=render();
 assert.equal(requestsAtPointer(),firstHoverRequests+1,'hover resumes at the throttle interval');
 tree.props.onPointerDown(pointer(100,100));tree.props.onPointerUp(pointer(100,100));tree=render();
 const vivian=buttons(tree).find(n=>nodeText(n)==='Vivian');
 assert.ok(vivian,'watch toolbar lists online characters');
 assert.equal(buttons(tree).find(n=>nodeText(n)==='DeepSeek'),undefined,'offline characters are excluded');
 vivian.props.onClick();await flush();
 assert.deepEqual(calls.at(-1).args,{sessionId:'watch-window',region:{x:40,y:60,width:400,height:300},action:'watch',analysisCharacter:'vivian'});
 await openWatch('watch-desktop');
 tree.props.onPointerDown(pointer(20,30));tree.props.onPointerUp(pointer(220,130));tree=render();
 buttons(tree).find(n=>nodeText(n)==='Vivian').props.onClick();await flush();
 assert.deepEqual(calls.at(-1).args,{sessionId:'watch-desktop',region:{x:20,y:30,width:200,height:100},action:'watch',analysisCharacter:'vivian',watchScope:'desktop'});
 globalThis.__selector.watchActive={phase:'paused',reason:'other_capture'};
 begin('exit-watch');requests.get('exit-watch')([137,80,78,71]);await flush();tree=render();
 const exitWatch=buttons(tree).find(n=>nodeText(n)==='退出陪看');
 assert.ok(exitWatch,'the selector can end an active watch');
 exitWatch.props.onClick();await flush();
 assert.ok(calls.some(c=>c.name==='screen_watch_dismiss'));
 assert.equal(calls.at(-1).name,'screen_selection_finish');
 assert.equal(calls.at(-1).args.action,'cancel');
 globalThis.__selector.watchActive=null;
 const urlCount=urls.length;
 begin('late');cleanup();requests.get('late')([1]);await flush();assert.equal(urls.length,urlCount);assert.equal(events.size,0);assert.ok(revoked.includes('blob:selector-2'));
 console.log('Reusable selector: ready ordering, binary preview, stale sessions, decoded-image presentation, pixel cleanup, reuse and unmount passed');
}finally{
 for(const key of ['window','performance','requestAnimationFrame','cancelAnimationFrame']){if(prior[key]===undefined)delete globalThis[key];else globalThis[key]=prior[key];}
 URL.createObjectURL=prior.create;URL.revokeObjectURL=prior.revoke;delete globalThis.__selector;
}
