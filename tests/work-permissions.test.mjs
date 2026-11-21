import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { transform } from 'esbuild';
import vm from 'node:vm';
const source=await readFile('src/components/mind-inspector/pages/CodeAgentPageNew.tsx','utf8');
const start=source.indexOf('  const permissionChangeSequence = useRef(0);');
const end=source.indexOf('  const handleModelChange',start);
assert.ok(start>=0 && end>start);
const compiled=await transform(`function render(activeId: string|null) {${source.slice(start,end)};return {change:handlePermissionChange};}`,{loader:'tsx',target:'es2022'});
const refs=[];let cursor=0,permission='workspace_write',sessions=[{session_id:'a',permission},{session_id:'b',permission}],calls=[],errors=[];
const context=vm.createContext({
 useRef(initial){return refs[cursor++]??={current:initial};},useCallback:fn=>fn,
 activeIdRef:{current:'a'},setPermission:value=>permission=value,
 setSessions:fn=>sessions=fn(sessions),notifyError:error=>errors.push(error),pickDirectory:async()=>null,
 invoke(command,args){return new Promise((resolve,reject)=>calls.push({command,args,resolve,reject}));},
});
vm.runInContext(compiled.code,context);
const render=id=>{cursor=0;context.activeIdRef.current=id;return context.render(id);};
const settle=async()=>{for(let i=0;i<12;i++)await Promise.resolve();};
let handler=render('a');handler.change('read_only');handler.change('full_access');await settle();
assert.equal(calls.length,1,'permission mutations must be serialized');
assert.equal(permission,'workspace_write','unacknowledged access cannot be displayed as active');
calls[0].resolve();await settle();
assert.equal(permission,'read_only','each acknowledged change is the current effective access, even with a queued choice');
assert.equal(calls.length,2);assert.equal(calls[1].args.permission,'full_access');
calls[1].reject(Error('policy save failed'));await settle();
assert.equal(permission,'read_only','a failed later choice must keep the last acknowledged access');assert.equal(errors.length,1);
handler.change('workspace_write');await settle();render('b');calls[2].resolve();await settle();
assert.equal(permission,'read_only','an inactive session response must not change the active label');
assert.equal(sessions.find(s=>s.session_id==='a').permission,'workspace_write');
console.log('Live permissions: serialized changes, acknowledged state, failure recovery and session switching passed.');
