import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { webcrypto } from 'node:crypto';
const calls=[];
globalThis.__desktopCalls=calls;
globalThis.__desktopInvoke=async(name,args)=>{
  calls.push({name,args});
  if(name==='companion_desktop_context')return {shortcuts:[]};
  return {};
};
const oldWindow=globalThis.window, oldCrypto=Object.getOwnPropertyDescriptor(globalThis,'crypto');
globalThis.window={location:{search:''}};
Object.defineProperty(globalThis,'crypto',{configurable:true,value:webcrypto});
const bundle=await build({entryPoints:['src/components/DesktopAssistantWindow.tsx'],bundle:true,write:false,platform:'node',format:'esm',loader:{'.css':'empty'},plugins:[{name:'desktop-fixtures',setup(b){
 b.onResolve({filter:/^react$|^react\/jsx-runtime$/},a=>({path:a.path,namespace:'fixture'}));
 b.onLoad({filter:/.*/,namespace:'fixture'},a=>({contents:a.path==='react'?
  `export const useState=value=>[globalThis.__desktopStates.length?globalThis.__desktopStates.shift():value,value=>globalThis.__desktopUpdates.push(value)]; export const useRef=value=>({current:value}); export const useEffect=effect=>globalThis.__desktopEffects.push(effect); export const useCallback=fn=>fn;`
  :`export const jsx=(type,props)=>({type,props});export const jsxs=jsx;export const Fragment='fragment';`}));
 b.onResolve({filter:/^@tauri-apps\/api\//},a=>({path:a.path,namespace:'native'}));
 b.onLoad({filter:/.*/,namespace:'native'},()=>({contents:`export const convertFileSrc=p=>p;export const invoke=(...args)=>globalThis.__desktopInvoke(...args);export const emit=async(...args)=>{globalThis.__desktopCalls.push({name:'emit',args});};export const listen=async()=>()=>{};export const getCurrentWindow=()=>({isVisible:async()=>true});`}));
 b.onResolve({filter:/^react-i18next$/},a=>({path:a.path,namespace:'i18n'}));
 b.onLoad({filter:/.*/,namespace:'i18n'},()=>({contents:`export const useTranslation=()=>({i18n:{language:globalThis.__desktopLanguage??'zh-CN'}});`}));
 b.onResolve({filter:/^lucide-react$/},a=>({path:a.path,namespace:'icons'}));
 b.onLoad({filter:/.*/,namespace:'icons'},()=>({contents:`export const ArrowUpRight=()=>null, Monitor=()=>null, NotebookPen=()=>null, Plus=()=>null, Rocket=()=>null, Trash2=()=>null;`}));
}}]});
const {default:Page}=await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
// 状态槽顺序必须和组件里的 useState 声明顺序一致：
// character / context / busy / message / error / note / shortcutName / shortcutTarget / shortcutKind / shortcutFormOpen
const states=()=>['nana',{shortcuts:[],quiet_reason:null},false,'','','note text','','','website',false];
function render(panel,props={},slots){
 globalThis.__desktopEffects=[];
 globalThis.__desktopUpdates=[];
 globalThis.__desktopStates=slots??states();
 return Page({panel,...props});
}
const text=node=>node==null?'':Array.isArray(node)?node.map(text).join(''):typeof node==='object'?text(node.props?.children):String(node);
function find(node,match){if(node==null)return null;if(Array.isArray(node)){for(const n of node){const found=find(n,match);if(found)return found;}return null;}if(typeof node==='object'){if(match(node))return node;return find(node.props?.children,match);}return null;}
let page;

// —— 随手记：保存只落到用户笔记，不变成宠物笔记本、也不触发说话 ——
page=render('notes');
await find(page,n=>n.type==='button'&&text(n)==='保存').props.onClick();
await new Promise(resolve=>setImmediate(resolve));
assert.deepEqual(calls.find(c=>c.name==='user_quick_notes_save').args,{content:'note text'});
assert.equal(calls.filter(c=>c.name==='create_notebook'||(c.name==='emit'&&c.args[0]==='companion:interaction')).length,0,'user notes never become pet notebooks or trigger speech on save');

const noteList = find(page,n=>typeof n.type==='function'&&n.type.name==='UserQuickNotes');
assert.ok(noteList);
globalThis.__desktopStates=[[{id:'user-note',title:'灵感',content:'我的灵感',created_at:1}],false,''];
globalThis.__desktopEffects=[];
const notesView=noteList.type();
const notesCleanup=globalThis.__desktopEffects[0]();
await new Promise(resolve=>setImmediate(resolve));
assert.equal(calls.find(c=>c.name==='user_quick_notes_list').args,undefined,'the user notebook is shared and has no character scope');
assert.ok(text(notesView).includes('我的灵感'));
await find(notesView,n=>n.type==='button'&&n.props['aria-label']==='删除随手记').props.onClick();
await new Promise(resolve=>setImmediate(resolve));
assert.deepEqual(calls.find(c=>c.name==='user_quick_notes_delete').args,{id:'user-note'});
assert.equal(calls.filter(c=>c.name==='list_notebooks'||c.name==='get_notebook_detail').length,0);
notesCleanup();
calls.length=0;

for (const [language, empty, remove] of [['zh-CN','暂无随手记','删除随手记'], ['ja','メモはまだありません','メモを削除'], ['en','No notes yet','Delete note']]) {
  globalThis.__desktopLanguage=language;
  globalThis.__desktopStates=[[],false,''];
  assert.equal(text(noteList.type()),empty);
  globalThis.__desktopStates=[[{id:'n',title:'User title',content:'User content',created_at:1}],false,''];
  assert.ok(find(noteList.type(),n=>n.type==='button'&&n.props['aria-label']===remove));
}
delete globalThis.__desktopLanguage;

// —— 快捷启动：条目由共享配置驱动，点开走 companion_launch_shortcut ——
page=render('shortcuts',{},['nana',{shortcuts:[{id:'s1',name:'B站',kind:'website',target:'https://bilibili.com'}],quiet_reason:null},false,'','','','','','website',false]);
assert.ok(find(page,n=>n.props?.['aria-label']==='添加快捷方式'),'the shortcuts page keeps its add button');
await find(page,n=>n.type==='button'&&n.props['aria-label']==='打开 B站').props.onClick();
await new Promise(resolve=>setImmediate(resolve));
assert.deepEqual(calls.find(c=>c.name==='companion_launch_shortcut').args,{shortcutId:'s1',characterId:'nana'});
calls.length=0;

// —— 总览层已移除：工具项（含截图分析 / 设置）不再出现在页面里 ——
page=render('notes');
assert.ok(find(page,n=>n.type==='textarea'),'the notes page is still the editor');
for (const gone of ['截图分析','设置','偏好','语音诊断']) {
  assert.equal(find(page,n=>n.type==='button'&&text(n)===gone),null,`「${gone}」不再由工具页提供`);
}
assert.equal(find(page,n=>n.props?.className==='assistant-grid'),null,'the overview grid is gone');
assert.equal(find(page,n=>n.props?.className==='assistant-tile'),null,'the overview tiles are gone');
assert.equal(find(page,n=>n.props?.className==='assistant-voice-groups'),null,'the voice diagnostics panel is gone');
// 认不出的分区兜底到随手记，而不是渲染出一个空壳。
page=render('daily');
assert.ok(find(page,n=>n.type==='textarea'),'an unknown panel falls back to notes instead of rendering nothing');
assert.equal(page.props['data-panel'],'notes');

// —— 嵌入聊天窗口时的可点击性生命周期 ——
calls.length=0;
page=render('notes',{embedded:true,initialCharacter:'nana'});
assert.ok(page.props.className.includes('desktop-assistant-embedded'));
const interactionEffect=globalThis.__desktopEffects.at(-1);
const release=interactionEffect();
assert.deepEqual(calls.find(c=>c.name==='ensure_chat_interactive')?.args,undefined,'entering a tool page enables clicks before any field gets focus');
assert.equal(page.props.onBlurCapture,undefined,'blurring a button or field must not make the whole page click-through');
assert.equal(page.props.onFocusCapture,undefined,'interaction is owned by the page lifecycle, not individual fields');
release();
const releaseAgain=interactionEffect();
assert.deepEqual(calls.filter(c=>c.name==='ensure_chat_interactive').length,3,'StrictMode setup/cleanup/setup ends with an interactive window');
releaseAgain();
assert.equal(calls.at(-1).name,'ensure_chat_interactive','leaving a tool page keeps chat interactive');
calls.length=0;
render('notes');
globalThis.__desktopEffects.at(-1)()?.();
assert.equal(calls.length,0,'standalone rendering does not alter the chat sidebar');

globalThis.window=oldWindow;if(oldCrypto)Object.defineProperty(globalThis,'crypto',oldCrypto);else delete globalThis.crypto;
delete globalThis.__desktopStates;delete globalThis.__desktopEffects;delete globalThis.__desktopUpdates;delete globalThis.__desktopCalls;delete globalThis.__desktopInvoke;
console.log('Tool pages: user-owned quick notes, configured shortcuts, no overview layer and interactive window lifecycle passed');
