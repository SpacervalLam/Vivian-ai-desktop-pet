// @test-environment live-desktop
import assert from 'node:assert/strict';
import {build} from 'esbuild';
import {readFile} from 'node:fs/promises';
import {chromium} from 'playwright-core';

const built=await build({stdin:{contents:`import React,{useState} from 'react';import{createRoot}from'react-dom/client';
 import {registerCharacters} from './src/characters';
 registerCharacters([{id:'deepseek',name:'DeepSeek'},{id:'claude',name:'Claude'}]);
 const Input=(await import('./src/components/InputDialog')).default;
 function App(){const[id,setId]=useState('deepseek'),[show,setShow]=useState(true),[broadcast,setBroadcast]=useState(false);window.chooseCompanion=setId;window.showInput=setShow;window.publicSpeech=()=>{setBroadcast(true);setShow(true)};
 return <>{show&&<Input characterId={id} broadcast={broadcast} />}</>}
 createRoot(document.getElementById('root')).render(<App/>);`,resolveDir:process.cwd(),loader:'tsx'},
 bundle:true,write:false,outfile:'tmp/new-character-ui.js',format:'esm',jsx:'automatic',
 plugins:[{name:'native-fixture',setup(b){
  b.onResolve({filter:/^@tauri-apps\/api\//},args=>({path:args.path,namespace:'native'}));
  b.onLoad({filter:/.*/,namespace:'native'},()=>({contents:`export const invoke=async(command,args)=>{
   window.nativeCalls.push({command,args});const id=args?.characterId;if(command==='polish_asr_text')return args.text;
   if(command==='get_mind_state')return{current_thought:id+' thought',goals:[{active:true,description:id+' goal'}]};
   if(command==='get_current_mood')return{valence:.4,arousal:.3,primary_emotion:'joy'};
   if(command==='get_psychology_state')return{emotion:{joy:.6},needs:{security:.7},relationship:{trust:.8}};
   return[];};export const listen=async(name,callback)=>{window.nativeListeners[name]=callback;return()=>{delete window.nativeListeners[name]}};export const emit=async()=>{};
   export const convertFileSrc=path=>path; export const getCurrentWindow=()=>({setFocus:async()=>{},hide:async()=>{},onFocusChanged:async()=>()=>{}});`}));
  b.onResolve({filter:/^react-i18next$/},()=>({path:'translation',namespace:'native-translation'}));
  b.onLoad({filter:/.*/,namespace:'native-translation'},()=>({contents:`export const useTranslation=()=>({t:(key)=>key,i18n:{language:'zh-CN'}});`}));
 }}]});
const js=built.outputFiles.find(f=>f.path.endsWith('.js')).text,css=built.outputFiles.find(f=>f.path.endsWith('.css')).text;
const browser=await chromium.launch({channel:'chrome',headless:true});
try{
 const page=await browser.newPage({viewport:{width:900,height:700}}),errors=[];
 page.on('pageerror',error=>errors.push(error.message));
 await page.route('http://companions-ui.test/**',async route=>{
  const path=new URL(route.request().url()).pathname;
  if(path==='/')return route.fulfill({contentType:'text/html',body:`<style>body{margin:0;background:#faf8f5}${css}</style><div id="root"></div><script>window.nativeCalls=[];window.nativeListeners={};</script><script type="module">${js}</script>`});
  if(path.startsWith('/chibi/'))return route.fulfill({contentType:'image/webp',body:await readFile('public'+path)});
  return route.fulfill({body:''});
 });
 await page.goto('http://companions-ui.test/');
 for(const[id,name]of[['deepseek','DeepSeek'],['claude','Claude'],['vivian','Vivian'],['nana','Nana']]){
  await page.evaluate(id=>window.chooseCompanion(id),id);
  await page.locator('.quick-compose--'+id).waitFor();
  assert.equal(await page.locator('.quick-compose-name').textContent(),name);
  assert.equal(await page.locator('.quick-compose input').getAttribute('placeholder'),'input_dialog.placeholder_'+id);
 }
 await page.evaluate(()=>window.publicSpeech());
 await page.locator('.quick-compose--broadcast').waitFor();
 assert.equal(await page.locator('.quick-compose-whisper').count(),0);
 await page.locator('.quick-compose input').fill('hello everyone');
 await page.locator('.quick-compose input').press('Enter');
 await page.waitForFunction(()=>window.nativeCalls.some(c=>c.command==='send_spoken_message'));
 assert.deepEqual(await page.evaluate(()=>window.nativeCalls.filter(c=>c.command==='send_spoken_message').map(c=>c.args)),[{message:'hello everyone'}]);
 await page.waitForTimeout(250);
 await page.evaluate(()=>window.showInput(false));
 await page.evaluate(()=>window.publicSpeech());
 await page.locator('.quick-compose-voice').click();
 await page.waitForFunction(()=>window.nativeCalls.some(c=>c.command==='start_recognition'));
 await page.evaluate(()=>window.nativeListeners['asr:event']({payload:{type:'final_result',text:'a spoken message'}}));
 await page.waitForFunction(()=>document.querySelector('.quick-compose input').value==='a spoken message');
 await page.evaluate(()=>window.nativeListeners['asr:event']({payload:{type:'stopped'}}));
 await page.waitForFunction(()=>window.nativeCalls.filter(c=>c.command==='send_spoken_message').length===2);
 assert.equal(await page.evaluate(()=>window.nativeCalls.some(c=>c.command==='send_message_stream')),false);
 assert.deepEqual(await page.evaluate(()=>window.nativeCalls.filter(c=>c.command==='send_spoken_message').map(c=>c.args)),[{message:'hello everyone'},{message:'a spoken message'}]);
 assert.deepEqual(errors,[]);
 console.log('Four-character names plus unified text/ASR sends and public composer without recipient controls passed.');
}finally{await browser.close();}
