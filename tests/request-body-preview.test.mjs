import assert from 'node:assert/strict';
import { build } from 'esbuild';
import {mkdir,writeFile} from 'node:fs/promises';

// Exercise the component's effects and handlers with deterministic hooks and time.
const slots = [], effects = [], timers = new Map(), calls = [], applied = [];
let cursor = 0, dirty = true, tree, now = 0, timerId = 0;
const equal = (a, b) => a && b && a.length === b.length && a.every((v, i) => Object.is(v, b[i]));
globalThis.__requestUIHooks = {
  state(initial) {
    const index = cursor++;
    if (!(index in slots)) slots[index] = initial;
    return [slots[index], value => {
      const next = typeof value === 'function' ? value(slots[index]) : value;
      if (!Object.is(slots[index], next)) { slots[index] = next; dirty = true; }
    }];
  },
  ref(initial) { const index = cursor++; return slots[index] ??= { current: initial }; },
  effect(callback, deps) {
    const index = cursor++;
    if (!equal(effects[index]?.deps, deps)) effects[index] = { deps, callback, cleanup: effects[index]?.cleanup, pending: true };
  },
};
const info = {known:true,supportsDisable:true,efforts:[],budget:null,source:null,verifiedAt:null,preview:{model:'test'},baseBody:{model:'test',messages:[]}};
let deferred;
globalThis.__requestUIInvoke = async (_name, args) => {
  calls.push(args);
  if (deferred) { const pending = deferred; deferred = undefined; return pending; }
  return {...info, preview: args.overrides?.$request?.body ?? info.preview};
};
const result = await build({entryPoints:['src/components/settings/ReasoningConfigField.tsx'],bundle:true,write:false,format:'esm',platform:'node',jsx:'transform',tsconfigRaw:{compilerOptions:{jsx:'react'}},loader:{'.css':'empty'},plugins:[{
  name:'ui-test-hooks', setup(build) {
    build.onResolve({filter:/^(react|react-i18next|lucide-react|@tauri-apps\/api\/core)$/}, args => ({path:args.path,namespace:'mock'}));
    build.onLoad({filter:/.*/,namespace:'mock'}, args => ({contents: args.path === 'react'
      ? `export const useState=(v)=>globalThis.__requestUIHooks.state(v),useRef=(v)=>globalThis.__requestUIHooks.ref(v),useEffect=(f,d)=>globalThis.__requestUIHooks.effect(f,d); export default {createElement:(type,props,...children)=>({type,props:{...props,children}})};`
      : args.path === 'react-i18next' ? `export const useTranslation=()=>({i18n:{language:'zh-CN'}});`
      : args.path === 'lucide-react' ? `export const ChevronDown='icon',SlidersHorizontal='icon',Code2='icon';`
      : `export const invoke=(...args)=>globalThis.__requestUIInvoke(...args);`}));
  },
} ]});
await mkdir(new URL('../tmp/',import.meta.url),{recursive:true});
const moduleUrl=new URL('../tmp/request-body-ui-test.mjs',import.meta.url);
await writeFile(moduleUrl,result.outputFiles[0].text);
const {default: Component} = await import(moduleUrl.href);
const props = {label:'思考模式',providerType:'chat_completions',model:'test',value:null,overrides:null,t:k=>k,onChange:()=>{},onOverridesChange:v=>applied.push(v)};
const realTimeout = globalThis.setTimeout, realClear = globalThis.clearTimeout;
globalThis.setTimeout = (callback, delay) => { const id=++timerId;timers.set(id,{callback,due:now+delay});return id; };
globalThis.clearTimeout = id => timers.delete(id);
async function settle() {
  for(let iteration=0;iteration<20;iteration++) {
    if(dirty) {
      dirty=false;cursor=0;tree=Component(props);
      for(const effect of effects) if(effect?.pending) {effect.pending=false;effect.cleanup?.();effect.cleanup=effect.callback();}
    }
    await Promise.resolve();
    if(!dirty) {await Promise.resolve();if(!dirty)return;}
  }
  throw new Error('Hook updates did not settle');
}
async function advance(ms) {
  now+=ms;
  for(const [id,timer] of [...timers]) if(timer.due<=now && timers.has(id)) {timers.delete(id);await timer.callback();await settle();}
}
function find(predicate,node=tree) {
  if(!node || typeof node!=='object')return;
  if(predicate(node))return node;
  for(const child of node.props?.children?.flat(Infinity)??[]) {const match=find(predicate,child);if(match)return match;}
}
const editor=()=>find(node=>node.type==='textarea'&&node.props['aria-label']==='自定义请求体 JSON');
async function edit(value) {editor().props.onChange({target:{value}});await settle();}
try {
  await settle();await advance(180);
  assert.ok(editor().props.placeholder.includes('$requestRef'));
  const actions=find(node=>node.props?.className==='reasoning-actions').props.children.flat();
  assert.equal(actions.length,2);
  assert.equal(actions[0].props.children[0],'格式化 JSON');
  assert.equal(actions[1].props.children[0],'应用配置');
  const baseline=calls.length;
  await edit('{"temperature":0.3}');await advance(2999);assert.equal(calls.length,baseline);
  await edit('{"temperature":0.8}');await advance(2999);assert.equal(calls.length,baseline);
  await advance(1);assert.equal(calls.length,baseline+1);assert.equal(calls.at(-1).overrides.$request.body.temperature,0.8);
  assert.equal(applied.length,0,'Automatic preview must not save configuration');
  await edit('{invalid');await advance(3000);assert.equal(calls.length,baseline+1);
  assert.ok(find(node=>node.props?.role==='alert'));
  // An older asynchronous result must not overwrite a more recent edit.
  let resolveOld;
  deferred=new Promise(resolve=>{resolveOld=resolve;});
  await edit('{"temperature":0.2}');
  now+=3000;
  const pending=[...timers].find(([,timer])=>timer.due<=now);
  timers.delete(pending[0]);const pendingCall=pending[1].callback();await settle();
  assert.equal(editor().props.disabled,false,'Preview must not lock typing');
  await edit('{"temperature":0.9}');resolveOld({...info,preview:{stale:true}});await pendingCall;await settle();
  assert.ok(!JSON.stringify(tree).includes('"stale"'));
  await advance(3000);assert.equal(calls.at(-1).overrides.$request.body.temperature,0.9);
  // Applying cancels the pending debounce instead of launching a duplicate preview.
  await edit('{"temperature":1}');
  await find(node=>node.props?.className==='reasoning-apply').props.onClick();await settle();
  const afterApply=calls.length;await advance(3000);
  assert.equal(applied.at(-1).$request.body.temperature,1);assert.equal(calls.length,afterApply);
  console.log('Request body UI: equal action order, empty template, 3s debounce, invalid JSON, stale results and apply cancellation passed.');
} finally {
  for(const effect of effects)effect?.cleanup?.();
  globalThis.setTimeout=realTimeout;globalThis.clearTimeout=realClear;
  delete globalThis.__requestUIHooks;delete globalThis.__requestUIInvoke;
}
