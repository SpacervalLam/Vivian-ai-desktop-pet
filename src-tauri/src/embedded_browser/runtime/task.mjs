import { parentPort, workerData } from 'node:worker_threads';
import { chromium } from 'playwright-core';
import { expect } from '@playwright/test';
import { start } from 'node:repl';
import { PassThrough } from 'node:stream';
import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { inspect } from 'node:util';

const send = value => parentPort.postMessage(value);
const transport = {
  send: message => send({kind:'cdp',message}),
  close: () => transport.onclose?.(),
};
let execute;
parentPort.on('message', message => {
  if (message.kind === 'cdp') transport.onmessage?.(message.message);
  else if (message.kind === 'execute') execute?.(message);
});
const browser = await chromium.connectOverCDP(transport, {noDefaults:true, isWebView:true, timeout:15000});
const context = browser.contexts()[0];
const events=[];
let images=[], logs=[], artifactBytes=0;
const short = value => inspect(value, {depth:3, maxArrayLength:20, maxStringLength:2000}).slice(0,4000);
const record = event => { events.push(event); if(events.length>40) events.shift(); };
const decorated = new WeakSet();
function decorate(page) {
  if (decorated.has(page)) return;
  decorated.add(page);
  page.on('framenavigated',frame=>record({type:'navigation',url:frame.url(),main:frame===page.mainFrame()}));
  page.on('dialog',dialog=>record({type:'dialog',dialog:dialog.type(),message:dialog.message().slice(0,1000)}));
  page.on('requestfailed',request=>record({type:'request_failed',url:request.url(),error:request.failure()?.errorText}));
  page.on('response',response=>{if(response.status()>=400)record({type:'http_error',url:response.url(),status:response.status()});});
  const original=page.screenshot.bind(page);
  // Preserve Playwright's Buffer return. The separate receipt delivers actual pixels.
  page.screenshot=async (...args)=>{
    const bytes=await original(...args);
    if(images.length<4 && bytes.length<=12*1024*1024 && artifactBytes+bytes.length<=64*1024*1024){
      await mkdir(workerData.artifacts,{recursive:true});
      const png=bytes.subarray(1,4).toString()==='PNG';
      const path=join(workerData.artifacts,randomUUID()+(png?'.png':'.jpg'));
      await writeFile(path,bytes);
      artifactBytes+=bytes.length;
      images.push({path,media_type:png?'image/png':'image/jpeg',bytes:bytes.length,
        ...(png?{width:bytes.readUInt32BE(16),height:bytes.readUInt32BE(20)}:{})});
    } else record({type:'image_budget_exceeded',message:'Pixels were not attached; screenshot Buffer is still returned to the program.'});
    return bytes;
  };
}
context.on('page',page=>{decorate(page);record({type:'page_opened',url:page.url()});});
context.pages().forEach(decorate);
const page=context.pages()[0];
const tabbit={
  async observe({snapshot=false,maxChars=6000}={}) {
    maxChars=Math.max(0,Math.min(16000,Number(maxChars)||0));
    const pages=[];
    for(const p of context.pages()){
      const item={url:p.url(),title:await p.title(),frames:p.frames().map(f=>({url:f.url(),main:f===p.mainFrame()}))};
      if(snapshot && maxChars){
        const text=await p.ariaSnapshot({mode:'ai'});item.snapshot=text.slice(0,maxChars);maxChars-=item.snapshot.length;
        item.domHints=await p.locator('input[type=checkbox],input[type=radio]').evaluateAll(inputs=>inputs
          .filter(input=>!input.getClientRects().length || getComputedStyle(input).visibility==='hidden')
          .flatMap(input=>Array.from(input.labels||[]).filter(label=>label.getClientRects().length).slice(0,2).map(label=>({
            control:input.id?`#${CSS.escape(input.id)}`:input.type,
            visibleLabel:label.innerText.slice(0,100),
            labelSelector:label.id?`#${CSS.escape(label.id)}`:input.id?`label[for="${CSS.escape(input.id)}"]`:null,
          }))).slice(0,20));
      }
      pages.push(item);
    }
    return {pages,events:events.splice(0),untrusted:true};
  },
  async triggerAndObserve(action,{event,timeout=5000,...options}={}) {
    // Register before input. No implicit extra waiting is added to native actions.
    const pending=event==='popup'?context.waitForEvent('page',{timeout}):null;
    if(pending) pending.catch(()=>{});
    const value=await action();
    if(pending)await pending;
    return {value,observation:await tabbit.observe(options)};
  },
};
const repl=start({input:new PassThrough(),output:new PassThrough(),terminal:false,useGlobal:false,ignoreUndefined:true});
repl.output.resume();
// Node's REPL routes rejected top-level await through its domain rather than
// the eval callback. Observe that channel too; never lose an assertion failure.
if(!repl._domain?.on)throw new Error('Unsupported Node REPL error channel');
Object.assign(repl.context,{browser,context,page,expect,tabbit,console:{
  log:(...args)=>{if(logs.length<20)logs.push(args.map(short).join(' '));},
  warn:(...args)=>{if(logs.length<20)logs.push(args.map(short).join(' '));},
  error:(...args)=>{if(logs.length<20)logs.push(args.map(short).join(' '));},
}});
let busy=false;
execute=message=>{
  if(busy){send({kind:'fatal',error:'Concurrent program submission is not allowed'});return;}
  busy=true; images=[]; logs=[];
  const started=Date.now();
  let completed=false;
  const complete=async(error,value)=>{
    if(completed)return;completed=true;repl._domain.removeListener('error',domainError);
    let result;
    try { result=value===undefined?null:Buffer.isBuffer(value)?{type:'Buffer',bytes:value.length,images}:JSON.parse(JSON.stringify(value,(_,v)=>typeof v==='bigint'?v.toString():v)); }
    catch { result=short(value); }
    if(JSON.stringify(result??null).length>16000)result={truncated:true,preview:short(result)};
    send({kind:'receipt',id:message.id,status:error?'failed':'succeeded',value:result??null,
      ...(error?{error:String(error.stack||error).slice(0,6000)}:{}),
      images,logs,events:events.splice(0),elapsed_ms:Date.now()-started,goal_verified:false,untrusted:true});
    busy=false;
  };
  const domainError=error=>complete(error);
  repl._domain.prependListener('error',domainError);
  repl.eval(message.code,repl.context,'vivian-browser-task.js',complete);
};
send({kind:'ready',version:'1.64.1',pages:context.pages().length});
