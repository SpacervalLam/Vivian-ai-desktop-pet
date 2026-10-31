import {cp,mkdir,readFile,writeFile,rm,access} from 'node:fs/promises';
import {resolve,join,sep,dirname} from 'node:path';
const root=resolve('src-tauri/browser-runtime');
if(!root.startsWith(resolve('src-tauri')+sep)||root!==resolve('src-tauri/browser-runtime'))throw new Error('Invalid managed runtime directory');
// This directory is generated, ignored and belongs only to this packaging step.
await rm(root,{recursive:true,force:true});
await mkdir(root,{recursive:true});
for(const name of ['playwright-core','playwright','@playwright/test']){
  const source=resolve('node_modules',name);
  const pkg=JSON.parse(await readFile(join(source,'package.json'),'utf8'));
  if(pkg.version!=='1.64.1')throw new Error(`Unexpected ${name} version`);
  await cp(source,join(root,'node_modules',name),{recursive:true});
}
for(const name of ['host.mjs','task.mjs'])await cp(resolve('src-tauri/src/embedded_browser/runtime',name),join(root,name));
await cp(process.execPath,join(root,process.platform==='win32'?'node.exe':'node'));
// Node's distribution license includes its bundled third-party notices.
let license;
for(const candidate of [join(dirname(process.execPath),'LICENSE'),join(dirname(process.execPath),'..','LICENSE'),join(dirname(process.execPath),'..','share','doc','node','LICENSE')]){
  try{await access(candidate);license=await readFile(candidate);break;}catch{}
}
if(!license){const response=await fetch(`https://raw.githubusercontent.com/nodejs/node/${process.version}/LICENSE`);if(!response.ok)throw new Error('Node distribution license unavailable');license=Buffer.from(await response.arrayBuffer());}
await writeFile(join(root,'NODE-LICENSE.txt'),license);
await writeFile(join(root,'runtime.json'),JSON.stringify({node:process.version,playwright:'1.64.1',transport:'private-stdio'},null,2));
console.log('Packaged original Playwright 1.64.1 and Node runtime (private stdio; no browser download).');
