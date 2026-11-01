import {cp,mkdir,readFile,writeFile,rm,access,readdir,stat,unlink} from 'node:fs/promises';
import {resolve,join,sep,dirname} from 'node:path';
import {createHash} from 'node:crypto';
import {spawnSync} from 'node:child_process';

const packs=JSON.parse(await readFile('src/optional-resources.json','utf8'));
const manifest=packs.find(pack=>pack.id==='browser-runtime');
if(!manifest?.download)throw new Error('optional-resources.json must declare the browser-runtime download');
const root=resolve('src-tauri/browser-runtime');
if(root!==resolve('src-tauri/browser-runtime'))throw new Error('Invalid managed runtime directory');
const version='1.64.1';
const archive=resolve('release',manifest.download.filename);
const staging=resolve('tmp/browser-runtime-payload');
const sha256Path=async path=>createHash('sha256').update(await readFile(path)).digest('hex');

// This directory is generated, ignored and belongs only to this packaging step.
await rm(root,{recursive:true,force:true});
await rm(staging,{recursive:true,force:true});
await mkdir(root,{recursive:true});
await mkdir(staging,{recursive:true});

// Only files reached while loading `playwright-core` and `@playwright/test` are kept.
// Type declarations, Vite UI assets, the test runner/babel bundle, the CLI and docs
// are never imported by the private stdio worker.
const packages=[
  ['playwright-core','node_modules/playwright-core',[
    'package.json','index.js','index.mjs','LICENSE','NOTICE','ThirdPartyNotices.txt','browsers.json',
    'lib/bootstrap.js','lib/coreBundle.js','lib/utilsBundle.js','lib/utilsBundle.js.LICENSE',
    'lib/webp_codec.wasm','lib/webp_codec.LICENSE','lib/server/chromium/appIcon.png',
  ]],
  ['playwright','node_modules/playwright',[
    'package.json','index.js','index.mjs','test.js','test.mjs','LICENSE','NOTICE','ThirdPartyNotices.txt',
    'lib/index.js','lib/errorContext.js','lib/util.js','lib/globals.js','lib/package.js',
    'lib/common/index.js','lib/matchers/expect.js','lib/mcp/test/browserBackend.js',
  ]],
  ['@playwright/test','node_modules/@playwright/test',[
    'package.json','index.js','index.mjs','LICENSE','NOTICE',
  ]],
];
for(const [name,target,paths] of packages){
  const packageJson=JSON.parse(await readFile(resolve('node_modules',name,'package.json'),'utf8'));
  if(packageJson.version!==version)throw new Error(`Unexpected ${name} version ${packageJson.version}`);
  for(const path of paths){
    const source=resolve('node_modules',name,path);
    try{await access(source);}catch{throw new Error(`Missing runtime file: ${name}/${path}`);}
    const destination=join(root,target,path);
    await mkdir(dirname(destination),{recursive:true});
    await cp(source,destination);
  }
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
await writeFile(join(root,'runtime.json'),JSON.stringify({node:process.version,playwright:version,transport:'private-stdio'},null,2));

// Fail loudly if a future Playwright release starts shipping something we dropped.
for(const forbidden of [
  'node_modules/playwright-core/types',
  'node_modules/playwright-core/lib/vite',
  'node_modules/playwright-core/lib/tools',
  'node_modules/playwright/types',
  'node_modules/playwright/lib/transform',
  'node_modules/playwright/lib/runner',
  'node_modules/playwright/lib/cli',
  'node_modules/playwright/cli.js',
]){
  try{await access(join(root,forbidden));throw new Error(`Unexpected runtime path: ${forbidden}`);}catch(error){if(error.code!=='ENOENT')throw error;}
}

// The packaged Node must load both entrypoints from the trimmed tree.
const probe=join(root,'runtime-verify.mjs');
await writeFile(probe,`import { chromium } from 'playwright-core';\nimport { expect } from '@playwright/test';\nif (typeof chromium.connectOverCDP !== 'function') throw new Error('chromium entrypoint unavailable');\nexpect(1).toBe(1);\nconsole.log('runtime-ok');\n`);
const result=spawnSync(join(root,process.platform==='win32'?'node.exe':'node'),[probe],{cwd:root,encoding:'utf8',timeout:60000});
await unlink(probe);
if(result.status!==0||!result.stdout.includes('runtime-ok'))throw new Error(`Trimmed runtime failed to load:\n${result.stderr||result.stdout}`);

async function walk(directory){
  const files=[];
  for(const entry of await readdir(directory,{withFileTypes:true})){
    const path=join(directory,entry.name);
    if(entry.isDirectory())files.push(...await walk(path));
    else if(entry.isFile())files.push(path.slice(root.length+1).replace(/\\/g,'/'));
    else throw new Error(`Unexpected runtime entry: ${path}`);
  }
  return files;
}
// The installed tree keeps node.exe loadable; the archive swaps it for an LZMA payload.
const payloads=new Map();
if(process.platform==='win32')payloads.set('node.exe','node.exe.lz');
const files=(await walk(root)).sort();
const hashes={};
let bytes=0;
let archivedBytes=0;
const entries=[{name:'pack.json',source:join(root,'pack.json')}];
for(const path of files){
  hashes[path]=await sha256Path(join(root,path));
  const size=(await stat(join(root,path))).size;
  bytes+=size;
  const payload=payloads.get(path);
  if(!payload){entries.push({name:path,source:join(root,path)});archivedBytes+=size;continue;}
  const compressed=join(staging,payload);
  const packed=spawnSync('python',['-c',
    'import sys,lzma\nfrom pathlib import Path\nPath(sys.argv[2]).write_bytes(lzma.compress(Path(sys.argv[1]).read_bytes(),format=lzma.FORMAT_ALONE,preset=6))\n',
    join(root,path),compressed],{stdio:'inherit'});
  if(packed.status!==0)throw new Error(`Could not compress ${path}`);
  const packedBytes=(await stat(compressed)).size;
  archivedBytes+=packedBytes;
  entries.push({name:payload,source:compressed});
  console.log(`[browser-runtime] ${path}: ${(size/1048576).toFixed(1)} MiB -> ${payload} ${(packedBytes/1048576).toFixed(1)} MiB`);
}
await writeFile(join(root,'pack.json'),JSON.stringify({
  id:manifest.id,version:manifest.version,playwright:version,node:process.version,payloads:Object.fromEntries(payloads),files:hashes,
},null,2));
entries[0].source=join(root,'pack.json');

await mkdir(resolve('release'),{recursive:true});
const zipped=spawnSync('python',['-c',
  'import sys,json\nfrom pathlib import Path\nfrom zipfile import ZipFile,ZipInfo,ZIP_DEFLATED,ZIP_STORED\nentries=json.loads(sys.argv[2])\nwith ZipFile(sys.argv[1],"w") as z:\n for entry in entries:\n  data=Path(entry["source"]).read_bytes()\n  info=ZipInfo(entry["name"],date_time=(2026,1,1,0,0,0))\n  info.external_attr=0o644<<16\n  if entry["name"].endswith(".lz"):\n   info.compress_type=ZIP_STORED\n   z.writestr(info,data)\n  else:\n   info.compress_type=ZIP_DEFLATED\n   z.writestr(info,data,compress_type=ZIP_DEFLATED,compresslevel=9)\n',
  archive,JSON.stringify(entries)],{stdio:'inherit'});
if(zipped.status!==0)throw new Error('Could not create browser runtime archive');
await rm(staging,{recursive:true,force:true});
const archiveBytes=(await stat(archive)).size;
const archiveHash=await sha256Path(archive);
// Release metadata is shared with the installer packs; merge instead of overwriting.
const metadataPath=resolve('release/resource-packs.json');
let released=[];
try{released=JSON.parse(await readFile(metadataPath,'utf8'));}catch{}
released=released.filter(pack=>pack.id!==manifest.id);
released.push({
  id:manifest.id,label:manifest.label,version:manifest.version,filename:manifest.download.filename,
  url:manifest.download.url,bytes:archiveBytes,sha256:archiveHash,
});
await writeFile(metadataPath,JSON.stringify(released,null,2)+'\n');
console.log(`[browser-runtime] ${files.length} files, ${(bytes/1048576).toFixed(1)} MiB unpacked -> ${manifest.download.filename} (${(archiveBytes/1048576).toFixed(1)} MiB)`);
