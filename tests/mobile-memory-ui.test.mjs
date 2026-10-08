// @test-environment live-desktop
// Self-contained Chrome fixture server; does not access application data or a model service.
import { createServer } from 'node:http';
import { readFileSync, writeFileSync, mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { spawn } from 'node:child_process';
import { resolve, join, sep } from 'node:path';
import { tmpdir } from 'node:os';
import assert from 'node:assert/strict';
const root = resolve('src-tauri/src/remote/frontend');
const html = readFileSync(join(root, 'index.html'), 'utf8');
new Function(html.match(/<script>([\s\S]*?)<\/script>/)[1]);
const history = Array.from({length: 12}, (_, i) => ({role: i % 2 ? 'user' : 'assistant', content: i % 2 ? '今天想一起去散步，顺便看看沿途的风景。' : '好呀，我陪你。慢慢走就好，今天发生了什么想和我分享的事吗？', timestamp: 1780000000 + i * 600}));
function memoryFixture(owner) {
  const name=owner==='vivian'?'Vivian':'Nana', quote='我喜欢安静的散步，我们一起完成散步计划吧';
  const first={id:owner+'-first',title:'一起规划散步',started_at:1780000000,ended_at:1780000300,participants:['user',owner],channels:['direct','wechat'],turns:Array.from({length:8},(_,i)=>({id:owner+'-turn-'+i,speaker:i%2?owner:'user',listener:i%2?'user':owner,text:i===0?quote:i%2?'好呀（轻轻点头）':'你好',timestamp:1780000000+i*30,channel:'direct'}))};
  const second={id:owner+'-second',title:'散步后的分享',started_at:1780100000,ended_at:1780100100,participants:['user',owner],channels:['wechat'],turns:[{id:owner+'-done',speaker:'user',listener:owner,text:'今天完成了散步计划',timestamp:1780100000,channel:'wechat'}]};
  const event=(id,quote,phase)=>({id:'walk',title:'一起完成散步计划',detail:quote,phase,source_message_id:id,source_quote:quote});
  const summary=(record,events,status='complete')=>({id:'summary-'+record.id,content:name+'的会话摘要（时间待定）',tags:[],created_at:record.ended_at,metadata:{record_kind:'session_summary',conversation_id:record.id,event_schema_version:1,summary_status:status,completed_parts:1,total_parts:2,summary_parts:[{events}]}});
  return {conversations:[first,second],memories:[
    {id:owner+'-fact',content:name+'记得：你喜欢安静的散步',tags:['preference'],created_at:1780000000,metadata:{record_kind:'fact',source_quote:'我喜欢安静的散步',conversation_id:first.id,topics:['preference']},open_hooks:[{condition:'下次一起沿河散步',closed_at:null}]},
    {id:'seed',content:'系统身份不可显示',tags:[],metadata:{record_kind:'fact',source:'system_seed'}},
    {id:'internal',content:'内部思考不可显示',tags:[],metadata:{record_kind:'subjective'}},
    {id:'old-dialogue',content:'旧对白不可回填',tags:[],metadata:{record_kind:'dialogue'}},
    summary(first,[event(owner+'-turn-0','一起完成散步计划','planned')],'partial'),
    summary(second,[event(owner+'-done','今天完成了散步计划','completed'),event(owner+'-done','从未说过的内容','completed')])
  ]};
}
const requests=[];
let memoryFailure=false, emptyMemory=false;
const server = createServer((req, res) => {
  const path = new URL(req.url, 'http://localhost').pathname;
  if (path.startsWith('/api/')) {
    const entry={path,method:req.method,body:''};requests.push(entry);req.on('data',chunk=>entry.body+=chunk);
    let data = {};
    const owner=path.includes('/nana/')?'nana':'vivian';
    if (path === '/api/characters') data = {characters: [{id:'vivian',name:'Vivian'}, {id:'nana',name:'Nana'}], active_id:'vivian'};
    else if (path.endsWith('/memory')) {
      if(memoryFailure){res.statusCode=500;res.end('临时连接错误');return;}
      data=emptyMemory?{memories:[],conversations:[]}:memoryFixture(owner);
    }
    else if (path.endsWith('/summarize')) data={content:'整理后的摘要',metadata:{summary_status:'partial'}};
    else if (path.endsWith('/profile/discovery')) data={interests:[{domain:'散步',weight:.8,source:'feedback'}],disliked_topics:['剧透'],exploration_openness:.6,store_available:5,store_recommended:2,probes:[{domain:'城市摄影',reason:'你提到留意街边风景',specifics:['街景'],confirmation_count:1,confirmation_threshold:3}]};
    else if (path.endsWith('/profile')) data={basic_facts:[{fact_type:'name',label:'姓名',content:owner==='vivian'?'小林':'小奈',is_pinned:true},{fact_type:'occupation',label:'职业',content:'设计师'}],recent_state:{recent_goals:['完成散步计划'],current_projects:['手机界面'],recent_preferences:['安静的地方']},custom_facts:[{fact_type:'custom',label:'自由事实',content:'喜欢记录沿途风景'}]};
    else if (path.endsWith('/history')) data = {history};
    else if (path.endsWith('/notes')) data = {notes: []};
    else if (path === '/api/todos') data = {todos: []};
    else if (path === '/api/tasks') data = {tasks: []};
    else if (path === '/api/confirmations') data = {confirmations: []};
    else if (path === '/api/toasts') data = {toasts: []};
    else if (path.endsWith('/memories')) data = {memories: []};
    res.setHeader('Content-Type','application/json');
    if(path.endsWith('/memory')&&owner==='nana')setTimeout(()=>res.end(JSON.stringify(data)),220);else res.end(JSON.stringify(data));return;
  }
  try {
    let file = path.startsWith('/remote/model/') ? resolve('public', path.slice('/remote/model/'.length)) : join(root, path === '/' ? 'index.html' : path.slice(1));
    const data = readFileSync(file);
    res.setHeader('Content-Type',file.endsWith('.html')?'text/html; charset=utf-8':file.endsWith('.css')?'text/css':file.endsWith('.js')?'application/javascript':file.endsWith('.webp')?'image/webp':file.endsWith('.png')?'image/png':'application/json'); res.end(data);
  } catch {res.statusCode=404;res.end();}
});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
const url = `http://127.0.0.1:${server.address().port}`;
const port = 9500 + Math.floor(Math.random()*500);
const profile = mkdtempSync(join(tmpdir(),'vivian-mobile-ui-'));
const chrome = spawn('C:/Program Files/Google/Chrome/Application/chrome.exe', ['--headless=new',`--remote-debugging-port=${port}`,`--user-data-dir=${profile}`,'--no-first-run','--disable-breakpad','about:blank'], {stdio:'ignore',windowsHide:true});
const delay = ms=>new Promise(r=>setTimeout(r,ms));
let socket;
try {
  let target;
  for(let i=0;i<60&&!target;i++) {try {target=(await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find(t=>t.type==='page');} catch {} if(!target) await delay(200);}
  assert(target);
  socket=new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r,j)=>{socket.onopen=r;socket.onerror=j;});
  let seq=0; const pending=new Map(); const errors=[];
  socket.onmessage=e=>{const msg=JSON.parse(e.data);if(msg.method==='Runtime.exceptionThrown')errors.push(msg.params.exceptionDetails);pending.get(msg.id)?.(msg);pending.delete(msg.id);};
  const send=(method,params={})=>new Promise((r,j)=>{const id=++seq;const timer=setTimeout(()=>j(Error(method)),10000);pending.set(id,msg=>{clearTimeout(timer);r(msg);});socket.send(JSON.stringify({id,method,params}));});
  const evaluate=async expression=>{const r=await send('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true});assert(!r.result.exceptionDetails,JSON.stringify(r.result.exceptionDetails));return r.result.result.value;};
  await send('Runtime.enable');
  await send('Page.enable');
  mkdirSync('tmp/mobile-ui-preview',{recursive:true});
  const shot=async name=>{await delay(300);const r=await send('Page.captureScreenshot',{format:'png'});writeFileSync(`tmp/mobile-ui-preview/${name}.png`,Buffer.from(r.result.data,'base64'));};
  for(const [w,h] of [[320,568],[390,844],[430,932],[844,390]]) {
    await send('Emulation.setDeviceMetricsOverride',{width:w,height:h,deviceScaleFactor:1,mobile:true});
    await send('Page.navigate',{url});await delay(800);
    assert.equal(await evaluate('document.querySelectorAll(".mobile-nav").length'),1);
    const bounds=await evaluate(`(() => { const input=document.querySelector('.direct-input-bar').getBoundingClientRect(), nav=document.querySelector('.mobile-nav').getBoundingClientRect(), app=document.querySelector('.app').getBoundingClientRect(), layer=document.querySelector('.pet-layer').getBoundingClientRect();return {overflow:document.documentElement.scrollWidth>innerWidth, inputBottom:input.bottom, navTop:nav.top, navBottom:nav.bottom, appHeight:app.height,layerHeight:layer.height};})()`);
    assert(!bounds.overflow,JSON.stringify(bounds));assert(bounds.inputBottom<=h+1,JSON.stringify(bounds));assert(Math.abs(bounds.appHeight-h)<2,JSON.stringify(bounds));assert(bounds.layerHeight>0,JSON.stringify(bounds));
    assert.equal(await evaluate(`document.querySelector('.home-heading, #stageToggle')`),null);
    await evaluate(`document.querySelector('[data-character="nana"]').click()`);
    assert.equal(await evaluate(`document.querySelector('[data-character="nana"]').getAttribute('aria-pressed')`),'true');
    await evaluate(`document.querySelector('[data-character="both"]').click()`);
    assert.equal(await evaluate(`petBothMode`),true);
    await evaluate(`document.querySelector('[data-character="vivian"]').click()`);
    // Stage swipes and scrolling chat must not change the selected character or both mode.
    for (const mode of ['single','both']) {
      await evaluate(`setBothMode(${mode==='both'})`);
      for (const target of ['petStage','directMessages']) for (const [dx,dy] of [[100,0],[-100,0],[0,-100],[0,100]]) {
        const result=await evaluate(`(() => {
          const el=document.getElementById('${target}');
          const emit=(type,x,y)=>{const ev=new Event(type,{bubbles:true});Object.defineProperty(ev,'touches',{value:[{clientX:x,clientY:y}]});Object.defineProperty(ev,'changedTouches',{value:[{clientX:x,clientY:y}]});el.dispatchEvent(ev);};
          emit('touchstart',150,200);emit('touchmove',150+${dx},200+${dy});emit('touchend',150+${dx},200+${dy});
          closeNavigation();return {char:petActiveChar,both:petBothMode};
        })()`);
        assert.equal(result.char,'vivian');assert.equal(result.both,mode==='both');
      }
    }
    await evaluate(`document.querySelector('[data-character="vivian"]').click()`);
    if(w===390)await shot('home');
    // A right swipe starting in the middle of the screen opens the left drawer.
    await evaluate(`(() => {
      const el=document.getElementById('directInput');
      const emit=(type,x,y)=>{const ev=new Event(type,{bubbles:true,cancelable:true});Object.defineProperty(ev,'touches',{value:[{clientX:x,clientY:y}]});Object.defineProperty(ev,'changedTouches',{value:[{clientX:x,clientY:y}]});el.dispatchEvent(ev);};
      emit('touchstart',160,260);emit('touchmove',255,265);emit('touchend',255,265);
    })()`);await delay(280);
    assert.equal(await evaluate(`document.getElementById('navigationDrawer').getAttribute('aria-hidden')`),'false');
    assert.equal(await evaluate(`document.getElementById('app').inert`),true);
    await evaluate(`(async () => {const end=performance.now()+2000;while(Math.abs(document.querySelector('.mobile-nav').getBoundingClientRect().left)>1&&performance.now()<end)await new Promise(r=>setTimeout(r,30));})()`);
    assert(Math.abs(await evaluate(`document.querySelector('.mobile-nav').getBoundingClientRect().left`))<=1);
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('.mobile-nav')).flexDirection`),'column');
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('.mobile-nav [data-page="home"]')).display`),'flex');
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('.mobile-nav [data-page="home"]')).borderTopWidth`),'0px');
    // Navigation clicks work immediately after the opening swipe.
    await evaluate(`document.querySelector('[data-page="memory"]').click()`);await delay(200);
    assert.equal(await evaluate(`document.getElementById('navigationDrawer').inert`),true);
    assert.equal(await evaluate(`document.getElementById('pageMemory').classList.contains('open')`),true);
    await evaluate(`openNavigation();document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))`);
    assert.equal(await evaluate(`document.getElementById('app').inert`),false);
    await evaluate(`goHome()`);
    await evaluate(`navigateMobile('memory')`);await delay(250);
    assert.equal(await evaluate(`document.querySelectorAll('.memory-layer').length`),4);
    assert.equal(await evaluate(`document.querySelectorAll('#memoryList .memory-entry').length`),1);
    assert(await evaluate(`document.getElementById('memoryList').textContent.includes('下次一起沿河散步')`));
    assert(!await evaluate(`/系统身份不可显示|内部思考不可显示|旧对白不可回填/.test(document.getElementById('memoryList').textContent)`));
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('.memory-layer')).borderRadius`),'20px');
    if(w===390)await shot('memory-facts');
    await evaluate(`const search=document.getElementById('memorySearch');search.value='安静';search.dispatchEvent(new Event('input',{bubbles:true}))`);
    assert(await evaluate(`document.querySelectorAll('.memory-highlight').length>=1`));
    await evaluate(`document.getElementById('memorySearch').value='不存在关键词';document.getElementById('memorySearch').dispatchEvent(new Event('input',{bubbles:true}))`);
    assert(await evaluate(`document.getElementById('memoryList').textContent.includes('没有找到匹配')`));
    await evaluate(`document.getElementById('memorySearch').value='';document.getElementById('memorySearch').dispatchEvent(new Event('input',{bubbles:true}));document.querySelector('[data-layer="episodes"]').click()`);
    assert.equal(await evaluate(`document.querySelectorAll('.memory-event-progress li').length`),2);
    assert(!await evaluate(`document.getElementById('memoryList').textContent.includes('从未说过')`));
    if(w===390){await evaluate(`document.querySelector('#pageMemory .page-body').scrollTop=180`);await shot('memory-events');}
    await evaluate(`document.querySelector('#memoryList [data-action="original"]').click()`);await delay(250);
    assert.equal(await evaluate(`document.querySelector('[data-layer="recent"]').getAttribute('aria-pressed')`),'true');
    assert.equal(await evaluate(`document.querySelectorAll('#mobile-conversation-vivian-first .memory-turn').length`),8);
    assert.equal(await evaluate(`document.querySelectorAll('#memoryList .memory-action-text').length`),4);
    await evaluate(`document.querySelector('#mobile-conversation-vivian-first [data-action="summary"]').click()`);
    assert(await evaluate(`document.querySelector('#mobile-conversation-vivian-first').textContent.includes('Vivian的会话摘要')`));
    assert.equal(await evaluate(`document.querySelectorAll('#mobile-conversation-vivian-first .memory-action-text').length`),0);
    assert(await evaluate(`document.querySelector('#mobile-conversation-vivian-second').textContent.includes('今天完成了散步计划')`));
    if(w===390)await shot('memory-conversations');
    await evaluate(`document.querySelector('#mobile-conversation-vivian-first [data-action="organize"]').click()`);await delay(200);
    assert(requests.some(r=>r.method==='POST'&&r.path==='/api/characters/vivian/memory/conversations/vivian-first/summarize'));
    await evaluate(`document.querySelector('[data-layer="profile"]').click()`);
    assert.equal(await evaluate(`getComputedStyle(document.getElementById('memorySearchWrap')).display`),'none');
    assert(await evaluate(`document.getElementById('memoryList').textContent.includes('小林')&&document.getElementById('memoryList').textContent.includes('城市摄影')`));
    await evaluate(`document.querySelector('[data-operation="respond_probe"][data-response="confirm"]').click()`);await delay(200);
    assert(requests.some(r=>r.method==='POST'&&r.path==='/api/characters/vivian/profile/discovery'&&r.body.includes('"response":"confirm"')));
    if(w===390){await evaluate(`document.querySelector('#pageMemory .page-body').scrollTop=100`);await shot('memory-profile');}
    await evaluate(`document.querySelector('#memTabs button:last-child').click();document.querySelector('#memTabs button:first-child').click()`);await delay(400);
    assert(await evaluate(`document.getElementById('memoryList').textContent.includes('小林')&&!document.getElementById('memoryList').textContent.includes('小奈')`));
    await evaluate(`document.querySelector('#memTabs button:last-child').click()`);await delay(400);
    await evaluate(`document.querySelector('#memoryList [data-action="edit-fact"]').click()`);
    assert(await evaluate(`!document.getElementById('memoryFactModal').classList.contains('hidden')`));
    await evaluate(`document.getElementById('memoryFactInput').value='新姓名';document.querySelector('#memoryFactModal form').dispatchEvent(new Event('submit',{bubbles:true,cancelable:true}))`);await delay(400);
    assert(requests.some(r=>r.method==='PUT'&&r.path==='/api/characters/nana/profile/name'));
    assert(requests.some(r=>r.method==='PUT'&&r.path==='/api/characters/nana/profile/name'&&r.body.includes('"pinned":true')));
    await evaluate(`document.querySelector('[data-layer="facts"]').click();document.querySelector('#memTabs button:first-child').click();goHome()`);await delay(100);
    if(w===390) {
      await evaluate(`openPage(null,'memory');selectMemoryLayer('profile')`);await delay(250);
      assert.equal(await evaluate(`document.querySelector('[data-layer="profile"]').getAttribute('aria-pressed')`),'true');
      memoryFailure=true;await evaluate(`selectMemoryLayer('facts');loadMemories()`);
      assert(await evaluate(`document.getElementById('memoryStatus').textContent.includes('读取失败')`));
      memoryFailure=false;emptyMemory=true;await evaluate(`selectMemoryLayer('recent');loadMemories()`);
      assert.equal(await evaluate(`document.querySelectorAll('#memoryList .memory-turn').length`),0);
      assert(await evaluate(`document.getElementById('memoryList').textContent.includes('还没有对话记录')`));
      emptyMemory=false;await evaluate(`selectMemoryLayer('facts');loadMemories();goHome()`);await delay(200);
    }
    await evaluate(`toggleChatPanel()`);assert.equal(await evaluate(`getComputedStyle(document.querySelector('.chat-panel')).display`),'none');
    await evaluate(`toggleChatPanel()`);
    await evaluate(`navigateMobile('wechat')`);await delay(200);
    assert.equal(await evaluate(`document.querySelector('.mobile-nav [aria-current="page"]').dataset.page`),'wechat');
    await evaluate(`openWxChat('vivian')`);await delay(200);
    await evaluate(`document.getElementById('wxInput').value='一条新消息';onWxInput()`);
    const chat=await evaluate(`(() => {const bar=document.querySelector('.wx-input-bar').getBoundingClientRect(),input=document.getElementById('wxInput').getBoundingClientRect();return {right:bar.right,width:input.width,bottom:bar.bottom,navTop:document.querySelector('.mobile-nav').getBoundingClientRect().top,overflow:document.documentElement.scrollWidth>innerWidth};})()`);
    assert(!chat.overflow&&chat.width>=60&&chat.bottom<=h+1,JSON.stringify(chat));
    await evaluate(`toggleWxDrawer('emoji')`);
    assert(await evaluate(`document.getElementById('wxEmojiDrawer').clientHeight>0`));
    await evaluate(`closeWxDrawers()`);
    if(w===390)await shot('chat');
    await evaluate(`goHome();openNavigation()`);
    assert.equal(await evaluate(`document.getElementById('navigationDrawer').inert`),false);
    if(w===390)await shot('more');
    await evaluate(`closeNavigation();openPage(null,'todo');newTodo()`);
    await evaluate(`(() => {
      const el=document.getElementById('todoTitle');el.value='保留表单草稿';
      const emit=(target,type,x,y)=>{const ev=new Event(type,{bubbles:true,cancelable:true});Object.defineProperty(ev,'touches',{value:[{clientX:x,clientY:y}]});Object.defineProperty(ev,'changedTouches',{value:[{clientX:x,clientY:y}]});target.dispatchEvent(ev);};
      emit(el,'touchstart',150,250);emit(el,'touchmove',250,254);emit(el,'touchend',250,254);
    })()`);
    assert.equal(await evaluate(`document.getElementById('navigationDrawer').classList.contains('open')`),true);
    assert.equal(await evaluate(`document.getElementById('todoModal').inert`),true);
    await evaluate(`(() => {
      const nav=document.querySelector('.mobile-nav');
      const emit=(type,x,y)=>{const ev=new Event(type,{bubbles:true,cancelable:true});Object.defineProperty(ev,'touches',{value:[{clientX:x,clientY:y}]});Object.defineProperty(ev,'changedTouches',{value:[{clientX:x,clientY:y}]});nav.dispatchEvent(ev);};
      emit('touchstart',200,250);emit('touchmove',100,254);emit('touchend',100,254);
    })()`);
    assert.equal(await evaluate(`document.getElementById('navigationDrawer').inert`),true);
    assert.equal(await evaluate(`document.getElementById('todoModal').inert`),false);
    assert.equal(await evaluate(`document.getElementById('todoTitle').value`),'保留表单草稿');
    assert(await evaluate(`document.getElementById('todoModal').getBoundingClientRect().bottom<=innerHeight+1`));
    if(w===390)await shot('todo-modal');
    await evaluate(`hideModal('todoModal');goHome();document.getElementById('directInput').value='正在选词';document.getElementById('directInput').dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',isComposing:true,bubbles:true}))`);
    assert.equal(await evaluate(`document.getElementById('directInput').value`),'正在选词');
    console.log(`PASS ${w}x${h}: four memory views, evidence, search, summary, profile ownership, navigation, IME`);
  }
  await send('Emulation.setDeviceMetricsOverride',{width:390,height:844,deviceScaleFactor:1,mobile:true});
  await send('Page.navigate',{url});await delay(600);
  await evaluate(`document.getElementById('directInput').focus();Object.defineProperty(window.visualViewport,'height',{configurable:true,value:400});window.visualViewport.dispatchEvent(new Event('resize'));`);await delay(200);
  assert.equal(await evaluate(`document.documentElement.classList.contains('keyboard-open')`),true);
  assert(await evaluate(`document.querySelector('.direct-input-bar').getBoundingClientRect().bottom<=400`));
  await shot('keyboard');
  await evaluate(`delete window.visualViewport.height;document.getElementById('directInput').blur();syncMobileViewport()`);await delay(200);
  assert.equal(await evaluate(`document.documentElement.classList.contains('keyboard-open')`),false);
  assert.equal(errors.length,0,JSON.stringify(errors));
  console.log('PASS simulated visual viewport keyboard, restoration; no JS exceptions');
  socket.send(JSON.stringify({id:9999,method:'Browser.close'}));await delay(300);
} finally {
  socket?.close();chrome.kill();server.close();
  assert(resolve(profile).startsWith(resolve(tmpdir())+sep)&&profile.includes('vivian-mobile-ui-'));
  await delay(500);try{rmSync(profile,{recursive:true,force:true});}catch{}
}

