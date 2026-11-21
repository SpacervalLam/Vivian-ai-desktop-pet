import assert from 'node:assert/strict';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { build } from 'esbuild';
import { chromium } from 'playwright';
import { CHARACTER_IDS, BUILTIN_CHARACTER_IDS, registerCharacters } from '../src/characters.js';

assert.deepEqual(BUILTIN_CHARACTER_IDS, ['vivian', 'nana']);
assert.deepEqual(CHARACTER_IDS, ['vivian', 'nana']);
const manifest=JSON.parse(await readFile('character-packs/chatgpt/character.json','utf8'));
registerCharacters([manifest]);
assert.deepEqual(CHARACTER_IDS,['vivian','nana','chatgpt']);
registerCharacters([]);
const core=JSON.parse(await readFile('src/chibi/animations.json','utf8'));
assert.deepEqual(core.characters,['vivian','nana']);
const built=await build({stdin:{contents:`
import React from 'react'; import {createRoot} from 'react-dom/client';
import Settings from './src/components/settings/ResourceLibrarySettings';
import CharacterPackageSettings from './src/components/settings/CharacterPackageSettings';
import {ToggleField,sectionTitleStyle} from './src/components/settings/SettingsFields';
import {initializeCharacterPackages} from './src/characterPackages';
import {resolveAvatarUrl} from './src/characterContext';
import {optionalAssetSource} from './src/utils/optionalResources';
import {CHARACTER_IDS} from './src/characters';
import {ROUTING_TASKS} from './src/components/settings/ModelSettings';
import {resolveMotion,frameStyle} from './src/chibi/motionRegistry';
await initializeCharacterPackages();
window.characterIds=()=>[...CHARACTER_IDS];
window.lineAvatarSources=async id=>[await resolveAvatarUrl(id),optionalAssetSource('/icons/charaters/'+id+'.webp')];
window.routingTasks=()=>ROUTING_TASKS.map(task=>task.taskType);
window.motionPreview=()=>frameStyle(resolveMotion('happy','chatgpt'),1,'chatgpt');
window.activateForTest=async()=>{window.nativeActivate();await initializeCharacterPackages();window.dispatchEvent(new Event('character-packages:changed'));};
createRoot(document.getElementById('root')).render(<main style={{maxWidth:1040,margin:'0 auto'}}><h3 style={sectionTitleStyle}>角色与资源</h3><CharacterPackageSettings/><div style={sectionTitleStyle}>常规</div><ToggleField label='开机自动启动' help='登录 Windows 后自动启动（当前用户启动项，无需管理员权限）' value={false} onChange={()=>{}}/><Settings/><div style={sectionTitleStyle}>快捷键</div></main>);
`,resolveDir:process.cwd(),loader:'tsx'},bundle:true,write:false,outfile:'test.js',format:'esm',plugins:[{name:'character-native',setup(b){
  b.onResolve({filter:/^@tauri-apps\//},()=>({path:'native',namespace:'native'}));
  b.onLoad({filter:/.*/,namespace:'native'},()=>({contents:`
const manifest=${JSON.stringify(manifest)};
const state={active:[],installed:[]};
const listeners=new Set();
window.nativeActivate=()=>{state.installed.forEach(p=>delete p.pending_update);state.active=state.installed.map(manifest=>({manifest,root:'C:/characters/chatgpt'}));listeners.forEach(fn=>fn({payload:{phase:'loaded',id:'chatgpt'}}));};
export const invoke=async(command,args)=>{
 if(command==='character_package_status')return structuredClone(state);
 if(command==='get_avatar_url')return '/icons/charaters/'+args.characterId+'.webp';
 if(command==='import_character_package'){state.installed=[{...structuredClone(manifest),atlas_path:'C:/characters/chatgpt/assets/chibi/chatgpt-atlas.webp',avatar_path:'C:/characters/chatgpt/assets/icons/charaters/chatgpt.webp',...(state.active.length?{version:'1.0.1',pending_update:true}:{})}];return manifest;}
 throw Error('Unexpected command '+command);
};
export const convertFileSrc=path=>'http://asset.test/'+path;
export const open=async()=>'/selected/character.zip';
export const listen=async(name,fn)=>{listeners.add(fn);return()=>listeners.delete(fn);};
`}));
  b.onResolve({filter:/^react-i18next$/},()=>({path:'translation',namespace:'translation'}));
  b.onLoad({filter:/.*/,namespace:'translation'},()=>({contents:`export const useTranslation=()=>({i18n:{language:'zh-CN'},t:k=>k});`}));
}}]});
const js=built.outputFiles.find(f=>f.path.endsWith('.js')).text;
const css=(await readFile('src/styles/global.css','utf8'))+'\n'+built.outputFiles.find(f=>f.path.endsWith('.css')).text;
const browser=await chromium.launch({channel:'chrome',headless:true});
try {
 const page=await browser.newPage({viewport:{width:800,height:600}});const errors=[];
 page.on('pageerror',e=>errors.push(e.message));
 await page.route('http://character-package.test/**',route=>route.fulfill({contentType:'text/html',body:`<html data-theme="light"><head><style>body{background:#f8f7f5;font:14px system-ui,sans-serif;padding:20px;margin:0}${css}</style></head><body><div id="root"></div><script type="module">${js}</script></body></html>`}));
 await page.route('**/icons/charaters/*.webp',async route=>{
  const url=new URL(route.request().url());
  const file=url.hostname==='asset.test'
    ? 'character-packs/chatgpt/assets/icons/charaters/chatgpt.webp'
    : 'public'+url.pathname;
  await route.fulfill({contentType:'image/webp',body:await readFile(file)});
 });
 await page.route('**/chibi/*-atlas.webp',async route=>{
  const file=route.request().url().includes('chatgpt')?'character-packs/chatgpt/assets/chibi/chatgpt-atlas.webp':'public'+new URL(route.request().url()).pathname;
  await route.fulfill({contentType:'image/webp',body:await readFile(file)});
 });
 await page.goto('http://character-package.test/');
 await page.getByRole('button',{name:'导入角色包 ZIP'}).waitFor();
 await page.getByRole('heading',{name:'角色与资源',exact:true}).waitFor();
 await page.getByRole('heading',{name:'可选资源包',exact:true}).waitFor();
 assert.deepEqual(await page.evaluate(()=>window.characterIds()),['vivian','nana']);
 assert.equal(await page.evaluate(()=>window.routingTasks().includes('companion_chatgpt')),false);
 await page.getByRole('button',{name:'导入角色包 ZIP'}).click();
 await page.getByText('正在准备入住',{exact:true}).waitFor();
 const portraits=page.locator('.character-pack-portrait');
 assert.deepEqual(await portraits.evaluateAll(async images=>Promise.all(images.map(async img=>{
  await img.decode();
  return img.getAttribute('src');
 }))),['/icons/charaters/vivian.webp','/icons/charaters/nana.webp','http://asset.test/C:/characters/chatgpt/assets/icons/charaters/chatgpt.webp'],'the pending resident preview comes from its imported package');
 assert.deepEqual(await page.evaluate(()=>window.characterIds()),['vivian','nana'],'import does not activate a character in the running window');
 await page.evaluate(()=>window.activateForTest());
 await page.getByText('已入住',{exact:true}).waitFor();
 assert.deepEqual(await page.evaluate(()=>window.characterIds()),['vivian','nana','chatgpt']);
 for(const id of ['vivian','nana']){
  const portrait=`/icons/charaters/${id}.webp`;
  assert.deepEqual(await page.evaluate(id=>window.lineAvatarSources(id),id),[portrait,portrait]);
 }
 const packagedPortrait='http://asset.test/C:/characters/chatgpt/assets/icons/charaters/chatgpt.webp';
 assert.deepEqual(await page.evaluate(()=>window.lineAvatarSources('chatgpt')),[packagedPortrait,packagedPortrait]);
 assert.equal(await page.evaluate(()=>window.routingTasks().includes('companion_chatgpt')),true,'new model routes appear without reopening settings');
 const style=await page.evaluate(()=>window.motionPreview());
 assert.match(style.backgroundImage,/characters\/chatgpt\/assets\/chibi\/motion\/chatgpt\/chatgpt-happy-sheet.webp/);
 assert.ok(style.translate && style.scale,'installed per-frame registration is used');
 assert.equal(await page.getByRole('button',{name:'卸载',exact:true}).count(),0,'no uninstall action is exposed');
 await page.getByRole('button',{name:'导入角色包 ZIP'}).click();
 await page.getByText('等待空闲更新',{exact:true}).waitFor();
 assert.deepEqual(await page.evaluate(()=>window.characterIds()),['vivian','nana','chatgpt'],'staged update keeps the running registry');
 const output = process.env.VIVIAN_SCREENSHOT_DIR;
 if (output) await mkdir(output, {recursive:true});
 const capture = async name => {
   if (output) await page.screenshot({path:resolve(output, name)});
 };
 await capture('settings.png');
 await page.setViewportSize({width:1250,height:650});await capture('resources-light.png');
 await page.evaluate(()=>{document.documentElement.dataset.theme='dark';document.body.style.setProperty('background','var(--panel-bg)','important');document.body.style.setProperty('color','var(--panel-text)');});
 await page.waitForTimeout(250);
 await capture('resources-dark.png');
 await page.setViewportSize({width:390,height:850});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth),'narrow settings module does not overflow');
 await capture('resources-narrow.png');
 await page.evaluate(()=>window.activateForTest());
 assert.deepEqual(await page.evaluate(()=>window.characterIds()),['vivian','nana','chatgpt']);
 assert.deepEqual(errors,[]);
 console.log('Character import, live activation, motion registration, non-removable residents and idle update UI verified.');
} finally {await browser.close();}
