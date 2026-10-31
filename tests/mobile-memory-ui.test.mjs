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
let companionFixtures=false, workFailure=false, workStatus='running', questionPending=true, connectionFailure=false;
const noteBlocks=[{type:'paragraph',text:'保留原有笔记样式',style:{color:'#876543',bold:true}},{type:'image',url:'https://example.com/old.png',caption:'原图说明'},{type:'table',headers:['项目'],rows:[['原始数据']],caption:'原始表格'}];
const workQuestion={question_id:42,session_id:'work-fixture',question:'下一步想先做哪件事？',context:'两种方案都已经准备好。',multi_select:false,options:[{label:'完善手机界面',description:'继续补上手机端体验'},{label:'先检查数据同步',description:'确认电脑和手机的一致性'}]};
const server = createServer((req, res) => {
  const path = new URL(req.url, 'http://localhost').pathname;
  if (path.startsWith('/api/')) {
    const entry={path,method:req.method,body:''};requests.push(entry);req.on('data',chunk=>entry.body+=chunk);
    let data = {};
    const owner=path.includes('/nana/')?'nana':'vivian';
    if (path === '/api/health') {if(connectionFailure){res.statusCode=503;res.end('电脑离线');return;}data={status:'ok',initialized:true};}
    else if (companionFixtures && path.startsWith('/api/work/')) {
      if(path.endsWith('/send')&&workFailure){res.statusCode=503;res.end('临时发送失败');return;}
      if(path==='/api/work/sessions')data=req.method==='POST'?{session_id:'work-created'}:{sessions:[{session_id:'work-fixture',char_id:'vivian',title:'完善手机端体验',working_directory:'G:\\vivian-rs',updated_at:1792026000000,message_count:3,status:workStatus,pending_question:questionPending?workQuestion:null}]};
      else if(path.endsWith('/stop')){workStatus='canceled';data={canceled:true};}
      else if(path.endsWith('/answer')){questionPending=false;data={answered:true};}
      else if(path.endsWith('/send')){workStatus='running';data={sent:true};}
      else data={session_id:path.split('/').at(-1),char_id:'vivian',title:'完善手机端体验',working_directory:'G:\\vivian-rs',status:workStatus,pending_question:questionPending?workQuestion:null,messages:[{role:'user',content:'让手机端更完整一些。',timestamp:1792026000000},{role:'thinking',content:'先看看工作区里的文件，再决定改哪一处。<script>window.unsafeWork=true</script>',timestamp:1792026000050},{role:'tool_use',tool_name:null,tool_call_id:null,tool_arguments:[{id:'call-read',name:'read_file',arguments:{path:'G:\\vivian-rs\\src-tauri\\src\\remote\\frontend\\index.html'}},{id:'call-search',name:'grep_search',arguments:{pattern:'unsafeWork'}}],timestamp:1792026000100},{role:'tool_result',tool_call_id:'call-read',tool_name:'read_file',tool_arguments:{path:'G:\\vivian-rs\\src-tauri\\src\\remote\\frontend\\index.html'},content:'{"total_lines":1200}\n<script>window.unsafeWork=true</script>',tool_success:true,timestamp:1792026000100},{role:'tool_result',tool_call_id:'call-search',tool_name:'grep_search',tool_arguments:{pattern:'unsafeWork'},content:'{"matches":[{"file":"src-tauri/src/remote/frontend/companion-ui.js"}],"total":1}',tool_success:true,timestamp:1792026000150},{role:'assistant',content:'**界面已经准备好。**\n\n接下来检查小屏幕和键盘遮挡。\n\n```js\nconst message = "你好";\n```',timestamp:1792026002000}],deliverables:['G:\\vivian-rs\\mobile.html'],work_todos:[{content:'完成手机布局',status:'completed'}]};
    }
    else if (companionFixtures && path.endsWith('/mind'))data={current_thought:owner==='nana'?'想把今天的小事写下来。':'窗边的阳光很好，想再陪你坐一会儿。',inner_monologue_enabled:true,attention_top:[{entity:'一起散步',weight:.8}],goals:[{description:'把今天的约定完成'}]};
    else if (companionFixtures && path.endsWith('/mood'))data={primary_emotion:'curiosity',arousal:.4,fatigue:0,stress:.12};
    else if (companionFixtures && path.endsWith('/relationship'))data={trust:86,intimacy:70,familiarity:74,interaction_count:128};
    else if (companionFixtures && path.endsWith('/presence'))data={state:'online',display_zh:'在你身边'};
    else if (companionFixtures && path.endsWith('/diary'))data={diaries:[{id:owner+'-today',date:'2026-10-15',content:'今天和你一起散步。\n\n<script>window.unsafeDiary=true</script>\n\n路边的小花开得很好看。',key_events:['一起沿河散步','约好了周末再见'],word_count:86,mood_tag:'happy',created_at:1792026000},{id:owner+'-yesterday',date:'2026-10-14',content:'昨天在窗边读了半本书。',key_events:['一起读书'],word_count:18,mood_tag:'neutral',created_at:1791939600}]};
    else if (companionFixtures && path.endsWith('/notes'))data={notes:[{id:'note-fixture',title:owner==='nana'?'Nana 的灵感手记':'Vivian 的灵感手记',updated_at:1792026000,block_count:3,tags:['灵感','散步']}]};
    else if (companionFixtures && path.includes('/notes/'))data={id:'note-fixture',title:'散步的小记',blocks:noteBlocks,layout:'magazine',palette:'lavender',tags:['灵感']};
    else if (companionFixtures && path==='/api/todos')data={items:[{id:'todo-open',title:'把沿途的照片整理好',description:'挑选三张喜欢的照片',priority:2,completed:false},{id:'todo-done',title:'一起出门散步',priority:1,completed:true}]};
    else if (companionFixtures && path==='/api/tasks')data={tasks:[{id:'task-fixture',message:'记得喝水',scheduled_time:1792036000,status:'paused',repeat_interval:3600}]};
    else if (path === '/api/characters') data = {characters: [{id:'vivian',name:'Vivian'}, {id:'nana',name:'Nana'}], active_id:'vivian'};
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
    await evaluate(`Promise.all(Array.from(document.getElementById('pageWechat').getAnimations()).map(animation=>animation.finished.catch(()=>{}))).then(()=>true)`);
    const chat=await evaluate(`(() => {const bar=document.querySelector('.wx-input-bar').getBoundingClientRect(),input=document.getElementById('wxInput').getBoundingClientRect();return {width:input.width,bottom:bar.bottom,overflow:document.documentElement.scrollWidth>innerWidth};})()`);
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
  companionFixtures=true;
  await send('Emulation.setDeviceMetricsOverride',{width:390,height:844,deviceScaleFactor:1,mobile:true});
  await send('Page.navigate',{url});await delay(700);
  await evaluate(`document.querySelector('.home-header .mobile-navigation-button').click()`);
  assert.equal(await evaluate(`document.getElementById('navigationDrawer').classList.contains('open')`),true);
  await evaluate(`navigateMobile('mind')`);await delay(400);
  assert(await evaluate(`document.getElementById('mindContent').textContent.includes('窗边的阳光')`));
  assert.equal(await evaluate(`document.querySelector('[role="meter"][aria-label="疲倦程度"]').getAttribute('aria-valuenow')`),'0');
  await shot('mind');
  await evaluate(`document.querySelector('#mindOwners [data-owner="nana"]').click();document.querySelector('#mindOwners [data-owner="vivian"]').click()`);await delay(350);
  assert(await evaluate(`document.getElementById('mindContent').textContent.includes('窗边的阳光')`));
  await evaluate(`navigateMobile('diary')`);await delay(300);
  assert.equal(await evaluate(`document.querySelectorAll('[data-diary-id]').length`),2);
  await shot('diary-list');
  await evaluate(`document.getElementById('diarySearch').value='读书';document.getElementById('diarySearch').dispatchEvent(new Event('input'))`);
  assert.equal(await evaluate(`document.querySelectorAll('[data-diary-id]').length`),1);
  await evaluate(`document.getElementById('diarySearch').value='';document.getElementById('diarySearch').dispatchEvent(new Event('input'));document.getElementById('diaryDate').value='2026-10-15';document.getElementById('diaryDate').dispatchEvent(new Event('change'));document.querySelector('[data-diary-id]').click()`);
  assert(await evaluate(`document.getElementById('diaryReader').textContent.includes('周末再见')`));
  assert.equal(await evaluate(`window.unsafeDiary`),undefined);
  assert.equal(await evaluate(`document.querySelector('#diaryReader script')`),null);
  await shot('diary-reader');await evaluate(`MobileUI.diaryBack();document.querySelector('#diaryOwners [data-owner="nana"]').click()`);await delay(250);
  assert(await evaluate(`document.querySelector('[data-diary-id]').dataset.diaryId.startsWith('nana')`));
  await evaluate(`navigateMobile('notes')`);await delay(250);
  assert(await evaluate(`document.getElementById('noteList').textContent.includes('编辑笔记')`));await shot('notes');
  await evaluate(`document.querySelector('#noteOwners [data-owner="nana"]').click()`);await delay(200);
  assert(await evaluate(`document.getElementById('noteList').textContent.includes('Nana')`));
  await evaluate(`openNote('note-fixture')`);
  assert.equal(await evaluate(`buildBlocks()[0].style.color`),'#876543');
  assert.equal(await evaluate(`JSON.stringify(buildBlocks()[2])`),JSON.stringify(noteBlocks[2]));
  await evaluate(`document.getElementById('noteTitle').value='手机上的草稿';document.getElementById('noteTitle').dispatchEvent(new Event('input',{bubbles:true}));hideModal('noteModal');openNote('note-fixture')`);
  assert.equal(await evaluate(`document.getElementById('noteTitle').value`),'手机上的草稿');
  await evaluate(`(() => {const input=document.querySelectorAll('#noteBlocks textarea')[1];input.value='https://example.com/new.png';input.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  assert.equal(await evaluate(`buildBlocks()[1].url`),'https://example.com/new.png');
  await evaluate(`saveNote()`);await delay(150);
  const savedNote=requests.findLast(r=>r.method==='PUT'&&r.path==='/api/characters/nana/notes/note-fixture');assert(savedNote);
  assert.equal(JSON.parse(savedNote.body).palette,undefined);assert.equal(JSON.parse(savedNote.body).layout,undefined);
  assert.deepEqual(JSON.parse(savedNote.body).blocks[2],noteBlocks[2]);
  await evaluate(`navigateMobile('todo')`);await delay(200);
  assert.equal(await evaluate(`document.querySelectorAll('#todoList .todo-item:not([hidden])').length`),1);
  await evaluate(`document.querySelector('#planFilters [data-filter="completed"]').click()`);
  assert(await evaluate(`document.querySelector('#todoList .todo-item:not([hidden])').textContent.includes('一起出门散步')`));await shot('plans');
  await evaluate(`navigateMobile('work')`);await delay(250);await shot('work-sessions');
  await evaluate(`document.getElementById('toastContainer').replaceChildren()`);
  await evaluate(`document.querySelector('[data-work-id]').click()`);await delay(250);
  assert(await evaluate(`document.getElementById('workQuestion').textContent.includes('下一步')`));
  assert.equal(await evaluate(`window.unsafeWork`),undefined);
  assert.equal(await evaluate(`document.getElementById('workProcessEntry').hidden`),false);
  assert(await evaluate(`document.getElementById('workProcessSummaryText').textContent.includes('读取 1 个文件')`));
  assert(await evaluate(`document.getElementById('workProcessSummaryText').textContent.includes('搜索 1 次')`));
  await evaluate(`document.getElementById('workProcessEntry').click()`);await delay(250);
  assert.equal(await evaluate(`document.getElementById('workProcessSheet').hidden`),false);
  assert(await evaluate(`document.querySelector('#workProcessList').textContent.includes('读取文件')`));
  assert(await evaluate(`document.querySelector('#workProcessList details[data-key]')!==null`));
  assert.equal(await evaluate(`window.unsafeWork`),undefined);
  await evaluate(`document.querySelector('[data-process-filter="failed"]').click()`);await delay(100);
  assert(await evaluate(`document.getElementById('workProcessList').textContent.includes('这个筛选下没有内容')`));
  await evaluate(`document.querySelector('[data-process-filter="all"]').click()`);await delay(100);
  await evaluate(`document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))`);await delay(100);
  assert.equal(await evaluate(`document.getElementById('workProcessSheet').hidden`),true);
  assert.equal(await evaluate(`document.getElementById('workDetail').hidden`),false);
  await shot('work-question');
  await evaluate(`document.querySelector('#workAnswerForm input').checked=true;document.getElementById('workAnswerForm').requestSubmit()`);await delay(250);
  const answer=requests.findLast(r=>r.path==='/api/work/sessions/work-fixture/answer');assert.equal(JSON.parse(answer.body).question_id,42);assert.deepEqual(JSON.parse(answer.body).answer.selected,['完善手机界面']);
  await evaluate(`document.getElementById('workStop').click()`);await delay(200);
  assert(requests.some(r=>r.path==='/api/work/sessions/work-fixture/stop'&&r.method==='POST'));
  workFailure=true;
  await evaluate(`document.getElementById('workInput').value='继续完成手机界面';document.getElementById('workInput').dispatchEvent(new Event('input'));document.getElementById('workComposer').requestSubmit()`);await delay(200);
  assert.equal(await evaluate(`document.getElementById('workInput').value`),'继续完成手机界面');
  assert(await evaluate(`document.getElementById('workError').textContent.includes('内容已保留')`));
  await evaluate(`MobileUI.workBack();document.querySelector('[data-mobile-action="new-work"]').click();document.getElementById('workCharacter').value='nana';document.getElementById('workDirectory').value='G:\\vivian-rs';document.getElementById('workNewForm').requestSubmit()`);await delay(250);
  const created=requests.findLast(r=>r.path==='/api/work/sessions'&&r.method==='POST');assert.equal(JSON.parse(created.body).character_id,'nana');
  assert.equal(await evaluate(`document.getElementById('workDetail').hidden`),false);
  await evaluate(`MobileUI.workBack()`);await delay(150);await evaluate(`document.querySelector('[data-work-id]').click()`);await delay(150);
  assert.equal(await evaluate(`document.getElementById('workInput').value`),'继续完成手机界面');
  workFailure=false;
  await evaluate(`document.getElementById('workComposer').requestSubmit()`);await delay(250);
  assert.equal(await evaluate(`document.getElementById('workInput').value`),'');
  await shot('work');
  for(const [w,h] of [[320,568],[844,390]]){
    await send('Emulation.setDeviceMetricsOverride',{width:w,height:h,deviceScaleFactor:1,mobile:true});await delay(200);
    const bounds=await evaluate(`(() => {const input=document.getElementById('workComposer').getBoundingClientRect();return {overflow:document.documentElement.scrollWidth>innerWidth,bottom:input.bottom};})()`);assert(!bounds.overflow&&bounds.bottom<=h+1,JSON.stringify(bounds));
  }
  await send('Emulation.setDeviceMetricsOverride',{width:390,height:844,deviceScaleFactor:1,mobile:true});
  await send('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:'dark'}]});await evaluate(`navigateMobile('mind')`);await delay(250);await shot('mind-dark');
  assert.equal(await evaluate(`getComputedStyle(document.documentElement).getPropertyValue('--bg').trim()`),'#211f28');
  await send('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:'light'}]});await evaluate(`navigateMobile('diary')`);await delay(250);await shot('diary-light');
  await evaluate(`navigateMobile('mind')`);await delay(250);await shot('mind-light');
  await evaluate(`navigateMobile('todo');newTask();document.querySelector('[data-reminder-minutes="30"]').click()`);
  assert.equal(await evaluate(`document.getElementById('taskTime').type`),'datetime-local');
  assert(await evaluate(`Math.abs(new Date(document.getElementById('taskTime').value).getTime()-Date.now()-1800000)<61000`));
  await evaluate(`document.getElementById('taskMessage').value='半小时后喝水';document.getElementById('taskRepeat').value='3600';saveTask()`);await delay(150);
  const reminder=requests.findLast(r=>r.path==='/api/tasks'&&r.method==='POST');assert(reminder);assert.equal(JSON.parse(reminder.body).repeat_interval,3600);
  connectionFailure=true;await evaluate(`document.getElementById('mobileConnection').click()`);await delay(150);
  assert.equal(await evaluate(`document.getElementById('connectionBanner').hidden`),false);
  connectionFailure=false;await evaluate(`document.querySelector('[data-mobile-action="reconnect"]').click()`);await delay(250);
  assert.equal(await evaluate(`document.getElementById('connectionBanner').hidden`),true);
  await send('Emulation.setEmulatedMedia',{features:[]});
  console.log('PASS mobile mind, diary, note preservation/drafts, plan filters, work create/send/stop/questions, failure recovery, small/landscape layout and dark theme');
  await send('Emulation.setDeviceMetricsOverride',{width:390,height:844,deviceScaleFactor:1,mobile:true});
  await send('Page.navigate',{url});await delay(600);
  await evaluate(`document.getElementById('directInput').focus();Object.defineProperty(window.visualViewport,'height',{configurable:true,value:400});window.visualViewport.dispatchEvent(new Event('resize'));`);await delay(200);
  assert.equal(await evaluate(`document.documentElement.classList.contains('keyboard-open')`),true);
  assert(await evaluate(`document.querySelector('.direct-input-bar').getBoundingClientRect().bottom<=400`));
  await shot('keyboard');
  await evaluate(`delete window.visualViewport.height;document.getElementById('directInput').blur();syncMobileViewport()`);await delay(200);
  assert.equal(await evaluate(`document.documentElement.classList.contains('keyboard-open')`),false);
  await evaluate(`navigateMobile('work')`);await delay(200);
  await evaluate(`document.querySelector('[data-work-id]').click()`);await delay(200);
  await evaluate(`document.getElementById('workInput').focus();Object.defineProperty(window.visualViewport,'height',{configurable:true,value:400});window.visualViewport.dispatchEvent(new Event('resize'));`);await delay(150);
  assert(await evaluate(`document.getElementById('workComposer').getBoundingClientRect().bottom<=400`));
  assert(await evaluate(`document.getElementById('workMessages').clientHeight>0`));
  await shot('work-keyboard');
  await evaluate(`delete window.visualViewport.height;document.getElementById('workInput').blur();syncMobileViewport()`);await delay(100);
  assert.equal(errors.length,0,JSON.stringify(errors));
  console.log('PASS simulated visual viewport keyboard, restoration; no JS exceptions');
  socket.send(JSON.stringify({id:9999,method:'Browser.close'}));await delay(300);
} finally {
  socket?.close();chrome.kill();server.close();
  assert(resolve(profile).startsWith(resolve(tmpdir())+sep)&&profile.includes('vivian-mobile-ui-'));
  await delay(500);try{rmSync(profile,{recursive:true,force:true});}catch{}
}

