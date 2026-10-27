import assert from 'node:assert/strict';
import { build } from 'esbuild';
let slots=[],effects=[],cursor=0,tree,Component,props={},calls=[],invalidated=0,installed=0;
const timers=new Map();
const same=(a,b)=>a&&b&&a.length===b.length&&a.every((v,i)=>Object.is(v,b[i]));
globalThis.__advisorUI={
  state(initial){const i=cursor++;if(!(i in slots))slots[i]=typeof initial==='function'?initial():initial;return[slots[i],v=>{slots[i]=typeof v==='function'?v(slots[i]):v;}];},
  ref(initial){const i=cursor++;return slots[i]??={current:initial};},
  callback(fn,deps){const i=cursor++;if(!same(slots[i]?.deps,deps))slots[i]={fn,deps};return slots[i].fn;},
  effect(fn,deps){const i=cursor++;if(!same(effects[i]?.deps,deps))effects[i]={fn,deps,pending:true,cleanup:effects[i]?.cleanup};},
  invoke:(...args)=>invoke(...args),invalidate:()=>{invalidated++;},emit:async()=>{},choose:async()=>null,
};
globalThis.window={setInterval(fn){const id=timers.size+1;timers.set(id,fn);return id;},clearInterval(id){timers.delete(id);}};
const mocks={
  react:`const h=globalThis.__advisorUI;export const useState=h.state,useRef=h.ref,useEffect=h.effect,useCallback=h.callback;export default {};`,
  'react/jsx-runtime':`export const jsx=(type,props)=>({type,props});export const jsxs=jsx;`,
  'react-i18next':`export const useTranslation=()=>({i18n:{language:'zh'},t:key=>key});`,
  '@tauri-apps/api/core':`export const invoke=(...args)=>globalThis.__advisorUI.invoke(...args);`,
  '@tauri-apps/api/event':`export const emit=(...args)=>globalThis.__advisorUI.emit(...args);`,
  '@tauri-apps/plugin-dialog':`export const open=(...args)=>globalThis.__advisorUI.choose(...args);`,
  '@tauri-apps/plugin-shell':`export const open=async()=>{};`,
  characterContext:`export const getCharacterId=()=>'vivian';`,
  ModelSettings:`export const invalidateProviderPresetsCache=()=>globalThis.__advisorUI.invalidate();`,
};
async function load(entry){const bundle=await build({entryPoints:[entry],bundle:true,write:false,format:'esm',platform:'node',plugins:[{name:'advisor-fixture',setup(b){
  b.onResolve({filter:/^react$|^react\/jsx-runtime$|^react-i18next$|^@tauri-apps\//},({path})=>({path,namespace:'fixture'}));
  b.onResolve({filter:/characterContext$|ModelSettings$/},({path})=>({path:path.endsWith('ModelSettings')?'ModelSettings':'characterContext',namespace:'fixture'}));
  b.onLoad({filter:/.*/,namespace:'fixture'},({path})=>({contents:mocks[path]}));
}}]});return(await import('data:text/javascript;base64,'+Buffer.from(bundle.outputFiles[0].text).toString('base64'))).default;}
const walk=n=>Array.isArray(n)?n.flatMap(walk):n&&typeof n==='object'?[n,...walk(n.props?.children)]:[];
const text=n=>typeof n==='string'||typeof n==='number'?String(n):Array.isArray(n)?n.map(text).join(''):text(n?.props?.children??'');
const find=predicate=>{const n=walk(tree).find(predicate);assert.ok(n,'Expected control');return n;};
const button=label=>find(n=>n.type==='button'&&text(n)===label);
const field=label=>find(n=>n.props?.['aria-label']===label);
function render(){cursor=0;tree=Component(props);for(const e of effects.filter(Boolean))if(e.pending){e.pending=false;e.cleanup?.();e.cleanup=e.fn();}}
async function settle(){for(let i=0;i<35;i++){await Promise.resolve();render();}}
async function click(label){assert.equal(!!button(label).props.disabled,false);button(label).props.onClick();await settle();}
function reset(){for(const e of effects.filter(Boolean))e.cleanup?.();slots=[];effects=[];cursor=0;}
let data={automatic:true,searching:false,gaps:[{id:'gap',need:'日历批量导出',evidence:['task-a','task-b'],lastSearch:0,searchError:null,resolved:false}],suggestions:[]};
const preview={fingerprint:'reviewed-fingerprint',name:'calendar',version:'1',missing_requirements:[],js_sources:[{source:'FULL_REVIEW_SOURCE'}],mcp_servers:[{command:'example',args:['--read']}],inspection:'structure_only_no_code_executed'};
const candidate={id:'candidate',gapId:'gap',name:'calendar',source:'https://github.com/example/calendar',revision:'a'.repeat(40),reason:'manifest only',status:'discovered',error:null,preview:null,testedTools:[]};
let installFails=false,holdSearch;
async function invoke(command,args){calls.push({command,args});
  if(command==='plugin_advisor_state')return structuredClone(data);
  if(command==='get_config')return {};
  if(command!=='plugin_advisor_action')throw Error('Unexpected command: '+command);
  const {action,input}=args;
  if(action==='search'){data.suggestions=[{...candidate}];if(holdSearch)await holdSearch;}
  if(action==='inspect'){data.suggestions[0]={...candidate,status:'reviewed',preview};return structuredClone({candidate:data.suggestions[0],state:data});}
  if(action==='install'){
    assert.deepEqual(input,{recommendationId:'candidate',fingerprint:'reviewed-fingerprint'});
    if(installFails)throw Error('Package changed after review');
    data.suggestions[0].status='loaded';
  }
  if(action==='dismiss')data.suggestions[0].status='dismissed';
  if(action==='automatic')data.automatic=input.enabled;
  if(action==='resolve')data.gaps[0].resolved=true;
  if(action==='draft')return structuredClone({sessionId:'work-session',state:data});
  return structuredClone(data);
}
Component=await load('src/components/plugins/PluginSuggestionsPanel.tsx');props={onInstalled:async()=>{installed++;}};
render();await settle();
let release;holdSearch=new Promise(r=>{release=r;});
button('查找兼容候选').props.onClick();button('查找兼容候选').props.onClick();await settle();
assert.equal(calls.filter(c=>c.args?.action==='search').length,1,'Double click does not duplicate discovery');
release();holdSearch=undefined;await settle();
assert.ok(text(tree).includes('已核对清单，待检查文件'));
await click('下载并检查 / 重新检查');
assert.ok(text(tree).includes('FULL_REVIEW_SOURCE'));
assert.ok(!calls.some(c=>c.args?.action==='install'),'Discovery and review do not install');
await click('取消');assert.ok(!walk(tree).some(n=>n.props?.role==='region'));
await click('下载并检查 / 重新检查');installFails=true;await click('确认安装并信任');
assert.ok(text(find(n=>n.props?.role==='alert')).includes('Package changed after review'));
assert.equal(installed,0);assert.equal(invalidated,0);
installFails=false;await click('确认安装并信任');
assert.equal(installed,1);assert.equal(invalidated,1);
assert.ok(text(tree).includes('已装载，业务待验证'));
assert.ok(!text(tree).includes('已有成功工具调用'));
await click('忽略此建议');assert.ok(!text(tree).includes('manifest only'));
field('关联能力缺口').props.onChange({target:{value:'gap'}});render();
await click('交给工作侧设计');assert.ok(text(tree).includes('已交给工作侧设计'));
find(n=>n.type==='input'&&n.props.type==='checkbox').props.onChange({target:{checked:false}});await settle();assert.equal(data.automatic,false);
await click('关闭此记录');assert.ok(text(tree).includes('尚无能力缺口记录'));
reset();assert.equal(timers.size,0,'Polling is removed on unmount');

Component=await load('src/components/ConfirmToast.tsx');
for(const tool of ['create_tool','create_plugin','install_plugin']){
  props={requestId:1,tool,reason:'Review exact contents',riskLevel:'high',allowAlwaysScope:'session',charId:'vivian',args:tool==='install_plugin'?{preview,source:candidate.source}:{name:'fixture',script:'FULL_SCRIPT',tools:[{name:'fixture',script:'FULL_SCRIPT'}]},onDone:()=>{}};
  render();await settle();
  assert.ok(!walk(tree).some(n=>n.type==='button'&&text(n).includes('allow_session')),'Executable packages never offer session-wide authorization');
  if(tool==='install_plugin')assert.ok(text(tree).includes('FULL_REVIEW_SOURCE'));
  else assert.ok(text(tree).includes('FULL_SCRIPT'));
  reset();
}
assert.equal(timers.size,0);
console.log('Plugin suggestions: discovery/review separation, explicit installation, stale package rejection, deduplication, dismissal, design, cleanup and one-time previews passed.');
