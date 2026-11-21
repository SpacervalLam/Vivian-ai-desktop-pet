import { characterAssetPath } from './helpers/character-assets.mjs';
// @test-environment live-desktop
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {chromium} from 'playwright-core';
const root='src-tauri/src/remote/frontend/';
const html=await readFile(root+'index.html','utf8');
const browser=await chromium.launch({channel:'chrome',headless:true});
try {
 const page=await browser.newPage({viewport:{width:430,height:900}}),requests=[],errors=[];
 page.on('pageerror',e=>errors.push(e.message));
 await page.route('http://companion.test/**',async route=>{
  const u=new URL(route.request().url()),path=u.pathname;
  if(path==='/')return route.fulfill({contentType:'text/html',body:html});
  if(path==='/api/characters')return route.fulfill({json:{active_id:'claude',characters:['vivian','nana','deepseek','claude','chatgpt'].map(id=>({id,name:({vivian:'Vivian',nana:'Nana',deepseek:'DeepSeek',claude:'Claude',chatgpt:'ChatGPT'})[id],online:true}))}});
  if(path==='/api/chat'){const body=route.request().postDataJSON();requests.push(body);return route.fulfill({json:{replies:[{character_id:'claude',reply:'A voluntary reply',expression:'happy'},{character_id:'deepseek',reply:'',expression:'neutral'}]}});}
  if(path.startsWith('/api/'))return route.fulfill({json:{messages:[],history:[],toasts:[],confirmations:[],characters:[],tasks:[],entries:[]}});
  if(path.startsWith('/remote/model/'))return route.fulfill({body:await readFile(characterAssetPath('public/'+path.slice('/remote/model/'.length))),contentType:'image/webp'});
  try{return await route.fulfill({body:await readFile(root+path.slice(1)),contentType:path.endsWith('.js')?'text/javascript':'text/css'});}catch{return route.fulfill({body:'',contentType:path.endsWith('.js')?'text/javascript':'text/css'});}
 });
 await page.goto('http://companion.test/');await page.waitForFunction(()=>typeof availablePetIds!=='undefined' && availablePetIds.length===5);
 for(const [id,name] of [['deepseek','DeepSeek'],['claude','Claude'],['chatgpt','ChatGPT']]){
  await page.locator(`.character-switch [data-character="${id}"]`).click();
  assert.equal(await page.locator('#petSlot'+name).getAttribute('data-active'),'true');
  await page.locator('#directInput').fill('hello');await page.locator('#directSendBtn').click();await page.waitForFunction(()=>!directLoading);
  assert.equal(requests.at(-1).channel,'broadcast');assert.equal(requests.at(-1).character_id,undefined);
  assert.equal(await page.locator('#bubbleClaude').textContent(),'A voluntary reply');
  assert.equal(await page.locator('#bubbleDeepSeek').textContent(),'');
 }
 requests.length=0;await page.locator('.character-switch [data-character="both"]').click();await page.locator('#directInput').fill('hello everyone');await page.locator('#directSendBtn').click();await page.waitForFunction(()=>!directLoading);
 assert.equal(requests.length,1,'one public utterance regardless of the stage display mode');
 assert.equal(requests[0].channel,'broadcast');
 assert.equal(await page.locator('.pet-slot[data-active="true"]').count(),5);
 assert.deepEqual(errors,[]);console.log('Mobile installed-character stage: display switching, one public turn and silence without empty bubbles passed.');
}finally{await browser.close();}
