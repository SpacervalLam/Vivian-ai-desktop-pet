import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { transformSync } from 'esbuild';
import vm from 'node:vm';
const source=readFileSync('src/components/mind-inspector/pages/CodeAgentPageNew.tsx','utf8');
const start=source.indexOf('  // Finish once into a drawer;');
const end=source.indexOf('  // 组统计',start);
assert.ok(start>=0 && end>start);
let expanded,previousDeps,effect,dirty;
const context=vm.createContext({
 useState(initial){if(expanded===undefined)expanded=initial;return [expanded,value=>{const next=typeof value==='function'?value(expanded):value;dirty=next!==expanded;expanded=next;}];},
 useLayoutEffect(fn,deps){if(!previousDeps || deps[0]!==previousDeps[0]){previousDeps=deps;effect=fn;}},
});
vm.runInContext(transformSync(`function drawer(completed:boolean){${source.slice(start,end)}return {expanded,setExpanded};}`,{loader:'tsx'}).code,context);
function render(completed){let result;do {dirty=false;result=context.drawer(completed);const pending=effect;effect=undefined;pending?.();}while(dirty);return result;}
let view=render(false);assert.equal(view.expanded,false);
view.setExpanded(true);view=render(false);assert.equal(view.expanded,true);
view=render(true);assert.equal(view.expanded,false,'completion folds before paint even when the user expanded live work');
view.setExpanded(true);view=render(true);assert.equal(view.expanded,true,'finished work can be reopened');
assert.equal(render(true).expanded,true,'ordinary rerenders must preserve manual reopening');
view=render(false);view=render(true);assert.equal(view.expanded,false,'a later completion also closes the drawer');
console.log('Process drawer: collapse before paint, manual reopening and repeated completion passed.');
