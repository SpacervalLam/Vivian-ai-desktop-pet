import assert from 'node:assert/strict';
import {readFile,readdir,stat} from 'node:fs/promises';
import path from 'node:path';
import vm from 'node:vm';
import {transform} from 'esbuild';
const readJson=async file=>JSON.parse(await readFile(file,'utf8'));
const capability=await readJson('src-tauri/capabilities/default.json');
const {characters}=await readJson('src/chibi/animations.json');
const app=await readFile('src/App.tsx','utf8');
const labelFunction=app.match(/function charScopedLabel\(base: string\): string \{[\s\S]*?\n\}/)?.[0];
const shared=app.match(/const SHARED_SUBWINDOWS = new Set\([^\n]+\);/)?.[0];
assert(labelFunction&&shared);
const code=(await transform(shared+'\n'+labelFunction,{loader:'ts'})).code;
function checkScopes(cap){
 for(const character of characters){
  const context=vm.createContext({getCharacterId:()=>character});vm.runInContext(code,context);
  const labels=[character,...['status','bubble','toast','chat','config','memory','input'].map(base=>context.charScopedLabel(base))];
  for(const label of labels)assert(cap.windows.includes(label),`${character}: missing native capability for ${label}`);
 }
 for(const permission of ['core:window:allow-set-size','core:window:allow-show','core:window:allow-hide','core:window:allow-start-dragging','core:window:allow-scale-factor','core:event:default'])assert(cap.permissions.includes(permission),permission);
}
checkScopes(capability);
// Native first-frame creation and frontend scaling must load the same geometry.
const native=await readFile('src-tauri/src/lib.rs','utf8');
const nativeFile=native.match(/include_str!\("([^"\n]*windowSize\.json)"\)/)?.[1];
const frontendFile=app.match(/import CHARACTER_WINDOW_SIZE from '([^']+)'/)?.[1];
assert(nativeFile&&frontendFile);
assert.equal(path.resolve('src-tauri/src',nativeFile),path.resolve('src',frontendFile));
const size=await readJson(path.resolve('src',frontendFile));
assert(size.w>=150&&size.h>=150&&Number.isFinite(size.w)&&Number.isFinite(size.h));
if(process.argv.includes('--compiled')){
 const outputs=[];
 for(const dir of await readdir('src-tauri/target/debug/build')){
  if(!dir.startsWith('vivian-'))continue;
  const file=path.join('src-tauri/target/debug/build',dir,'out/capabilities.json');
  try{outputs.push({file,time:(await stat(file)).mtimeMs});}catch(error){if(error.code!=='ENOENT')throw error;}
 }
 const latest=outputs.sort((a,b)=>b.time-a.time)[0];assert(latest,'Run cargo check before checking compiled capabilities');
 checkScopes((await readJson(latest.file)).default);
}
console.log('Pet window parity: all four main/private window scopes and shared native/frontend size verified.');
