// @test-environment live-desktop
import assert from 'node:assert/strict';
import {build} from 'esbuild';
import {readFile} from 'node:fs/promises';
import {chromium} from 'playwright-core';
const rows=JSON.parse(await readFile('public/stickers/catalog.json','utf8'));
const built=await build({stdin:{contents:`import React from 'react';import{createRoot}from'react-dom/client';import Settings from './src/components/stickers/StickerSettings';createRoot(document.getElementById('root')).render(<Settings/>);`,resolveDir:process.cwd(),loader:'tsx'},bundle:true,write:false,jsx:'automatic',format:'iife',plugins:[{name:'settings-fixture',setup(b){
 b.onResolve({filter:/^@tauri-apps\//},args=>({path:args.path,namespace:'native'}));
 b.onLoad({filter:/.*/,namespace:'native'},()=>({contents:`export const invoke=async(command,args)=>{window.calls.push({command,args});if(command==='list_stickers')return{frequency:'occasional',stickers:window.rows};return{};};export const open=async()=> 'C:/fixture/sticker.png';`}));
 b.onResolve({filter:/react-i18next$|\/i18n$/},()=>({path:'translation',namespace:'translation'}));
 b.onLoad({filter:/.*/,namespace:'translation'},()=>({contents:`const i18n={language:'zh-CN',t:(key,opts)=>opts?.defaultValue||key};export default i18n;export const useTranslation=()=>({i18n});`}));
 b.onResolve({filter:/\.\/StickerImage$/},()=>({path:'image',namespace:'image'}));
 b.onLoad({filter:/.*/,namespace:'image'},()=>({contents:'export default function Image(){return null}'}));
 b.onResolve({filter:/optionalResources$/},()=>({path:'resources',namespace:'resources'}));
 b.onLoad({filter:/.*/,namespace:'resources'},()=>({contents:'export const resourcePackInstalled=()=>true;'}));
}}]});
const browser=await chromium.launch({channel:'chrome',headless:true});
try{
 const page=await browser.newPage(),errors=[];page.on('pageerror',error=>errors.push(error.message));
 await page.route('http://stickers.test/',route=>route.fulfill({contentType:'text/html',body:`<div id="root"></div><script>window.rows=${JSON.stringify(rows)};window.calls=[];</script><script>${built.outputFiles[0].text}</script>`}));
 await page.goto('http://stickers.test/');const library=page.locator('section[aria-label="共享贴纸库"]');
 await library.locator('h5').first().waitFor();
 assert.equal(await library.locator('h5').count(),new Set(rows.map(row=>row.category)).size);
 assert.equal((await library.textContent()).includes('薇薇安'),false);assert.equal((await library.textContent()).includes('娜娜'),false);
 assert.equal(await page.locator('select[aria-label$="表情包频率"]').count(),4);
 await library.locator('summary').click();
 await library.getByLabel('贴纸类型',{exact:true}).selectOption('comfort');
 await library.getByLabel('贴纸名称',{exact:true}).fill('抱抱');
 await library.getByLabel('贴纸含义',{exact:true}).fill('轻轻安慰对方');
 await library.getByRole('button',{name:'选择 PNG 并导入'}).click();
 await page.waitForFunction(()=>window.calls.some(call=>call.command==='import_sticker'));
 const imported=await page.evaluate(()=>window.calls.find(call=>call.command==='import_sticker').args);
 assert.equal(imported.category,'comfort');assert.equal(imported.label,'抱抱');assert.equal(imported.replaceId,null);
 await library.getByLabel('替换目标',{exact:true}).selectOption('vivian_shy_01');
 assert.equal(await library.getByLabel('贴纸类型',{exact:true}).inputValue(),'shy');
 assert.deepEqual(errors,[]);
 console.log('Sticker settings: one shared library, reaction-type groups, per-pet frequency and typed imports/replacements passed.');
}finally{await browser.close();}
