// @test-environment live-desktop
// Exercise production React controls with an isolated host fixture, no app data or model.
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { createServer } from 'node:http';
import { chromium } from '@playwright/test';
const root=path.resolve('.'),out=path.join(root,'tmp','agreements-ui');await mkdir(out,{recursive:true});
const host=String.raw`
const scopes=[{id:'ongoing-vivian',owner:'vivian',title:'番剧清单',goal:'持续维护看番记录',trigger:'用户提到追番进展',mode:'continuous',status:'active',revision:1,updated_at:1,entries:[{key:'dungeon',label:'迷宫饭',state:'completed',detail:'已看完',evidence:{source_id:'user-1',quote:'迷宫饭看完了',timestamp:1}}]},
{id:'ongoing-nana',owner:'nana',title:'读书记录',goal:'持续整理读书记录',trigger:'用户聊到新书',mode:'continuous',status:'candidate',revision:1,updated_at:1,entries:[]}];
let todos=[];window.__calls=[];window.__fail=false;window.__listeners={};window.__scopes=scopes;window.__todos=()=>todos;
const fire=name=>(window.__listeners[name]||[]).forEach(fn=>fn({payload:{}}));
window.__invoke=async(name,args={})=>{window.__calls.push({name,args:structuredClone(args)});if(window.__fail&&['create_agreement','set_agreement_state','add_todo_item'].includes(name))throw new Error('保存失败，草稿仍保留');
 if(name==='get_memories')return [...scopes.filter(s=>s.owner===args.characterId).map(s=>({id:s.id,content:s.goal,memory_type:'project',tags:['ongoing_task'],importance:.8,created_at:1,metadata:{record_kind:'ongoing_task',memory_owner:s.owner,ongoing_task:structuredClone(s)}})),{id:'shared-agreement',content:'共享信息不会转移约定',metadata:{record_kind:'ongoing_task',memory_owner:'someone-else',known_by:[args.characterId],ongoing_task:structuredClone(scopes[0])}}];
 if(name==='list_todos')return {items:structuredClone(todos)};if(name==='list_scheduled_tasks')return {tasks:[]};
 if(name==='create_agreement'){const id='new-'+scopes.length;scopes.push({id,owner:args.characterId,title:args.title,goal:args.goal,trigger:args.trigger,revision:1,status:'active',mode:'continuous',updated_at:2,entries:[]});fire('memory:updated');return id;}
 if(name==='set_agreement_state'){const s=scopes.find(s=>s.id===args.id&&s.owner===args.characterId);if(!s||s.revision!==args.revision)throw new Error('约定已变化，请刷新');s.status=args.status;s.revision++;fire('memory:updated');return true;}
 if(name==='add_todo_item'){const s=scopes.find(s=>s.id===args.agreement?.id&&s.owner===args.agreement?.character_id);if(args.agreement&&s?.status!=='active')throw new Error('约定已暂停');const item={id:'todo-'+todos.length,title:args.title,description:args.description,completed:false,priority:1,created_at:1,agreement:args.agreement?{...args.agreement,title:s.title}:null};todos.push(item);fire('todo:changed');return {item:structuredClone(item)};}
 if(name==='complete_todo_item'){const item=todos.find(t=>t.id===args.id);item.completed=true;fire('todo:changed');return {item:structuredClone(item)};}
 return [];
};
`;
await build({stdin:{contents:`import React,{useState} from 'react';import{createRoot}from'react-dom/client';import Planner from './src/components/mind-inspector/pages/PlannerPage';import{NavigationProvider}from './src/components/mind-inspector/NavigationContext';${host}
function App(){const[params,setParams]=useState({sub:'agreements'});return <NavigationProvider value={{pageParams:params,navigateTo:(_,p)=>setParams({...p}),activePage:'planner',clearPageParams:()=>setParams({}),headerExtra:null,setHeaderExtra:()=>{}}}><div className="record-scope claude-theme"><Planner embedded/></div></NavigationProvider>}createRoot(document.getElementById('root')).render(<App/>);`,resolveDir:root,loader:'tsx'},outfile:path.join(out,'app.js'),bundle:true,format:'esm',platform:'browser',loader:{'.css':'empty'},plugins:[{name:'isolated-agreement-host',setup(b){
 b.onResolve({filter:/^@tauri-apps\/api\//},args=>({path:args.path,namespace:'host'}));
 b.onLoad({filter:/.*/,namespace:'host'},()=>({contents:`export const invoke=(...a)=>window.__invoke(...a);export const listen=async(name,fn)=>{(window.__listeners[name]??=[]).push(fn);return()=>{window.__listeners[name]=window.__listeners[name].filter(v=>v!==fn)}};export const emit=()=>{};export const getCurrentWindow=()=>({});export const convertFileSrc=x=>x;`}));
 b.onResolve({filter:/^react-i18next$/},()=>({path:'translations',namespace:'translations'}));
 b.onLoad({filter:/.*/,namespace:'translations'},()=>({resolveDir:root,contents:`import{comicTranslations}from'./src/i18n/comic.js';const words={'mind_inspector.nav_todo':'待办','mind_inspector.nav_scheduler':'提醒','todo_window.btn_add':'新增待办','todo_window.btn_complete':'完成','todo_window.tab_pending':'待完成','todo_window.tab_completed':'已完成','todo_window.tab_all':'全部'};export const useTranslation=()=>({i18n:{language:'zh-CN',resolvedLanguage:'zh-CN'},t:(key)=>words[key]??comicTranslations['zh-CN']?.[key.replace(/^comic\\./,'')]??key});`}));
}}]});
const css=await Promise.all(['ClaudeTheme.css','RecordTheme.css','AgreementsPage.css'].map(file=>readFile(path.join(root,'src/components/mind-inspector/pages',file),'utf8')));
await writeFile(path.join(out,'index.html'),`<!doctype html><meta charset="utf-8"><style>${css.join('\n')}body{margin:0;background:#faf9f5;font:14px system-ui;color:#292724}#root{max-width:900px;padding:24px;margin:auto}.record-scope{min-height:600px}.comic-plan-content{padding:12px}.agreement-page{max-height:none}</style><div id="root"></div><script type="module" src="/app.js"></script>`);
const server=createServer(async(req,res)=>{try{res.setHeader('content-type',req.url==='/app.js'?'text/javascript':'text/html; charset=utf-8');res.end(await readFile(path.join(out,req.url==='/app.js'?'app.js':'index.html')));}catch{res.statusCode=500;res.end();}});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
let browser;
try{
 browser=await chromium.launch({headless:true,executablePath:'C:/Program Files/Google/Chrome/Application/chrome.exe'});
 const page=await browser.newPage({viewport:{width:1100,height:960}});const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.goto(`http://127.0.0.1:${server.address().port}`);await page.getByRole('heading',{name:'番剧清单',exact:true}).waitFor();
 assert.equal(await page.locator('[data-agreement-id]').count(),1);
 await page.getByRole('button',{name:'新增关联待办',exact:true}).click();
 await page.getByLabel('这次要完成的具体行动').fill('周末补齐旧番记录');await page.getByLabel('完成条件或补充说明').fill('补齐以前提到的作品');
 await page.getByRole('button',{name:'保存待办',exact:true}).click();await page.getByRole('button',{name:'完成待办',exact:true}).waitFor();
 assert.deepEqual(await page.evaluate(()=>window.__calls.find(c=>c.name==='add_todo_item').args.agreement),{character_id:'vivian',id:'ongoing-vivian',title:'番剧清单'});
 await page.getByRole('button',{name:'完成待办',exact:true}).click();await page.waitForFunction(()=>window.__todos()[0].completed);
 assert.equal(await page.evaluate(()=>window.__scopes[0].status),'active');
 await page.getByRole('button',{name:'打开关联待办',exact:true}).click();await page.getByRole('button',{name:/来自约定：番剧清单/}).waitFor();
 await page.getByRole('button',{name:/来自约定：番剧清单/}).click();await page.getByRole('heading',{name:'番剧清单',exact:true}).waitFor();
 await page.getByRole('button',{name:'暂停维护',exact:true}).click();await page.waitForFunction(()=>window.__scopes[0].status==='paused');
 await page.getByRole('button',{name:'已暂停',exact:true}).click();await page.getByRole('heading',{name:'番剧清单',exact:true}).waitFor();
 assert.equal(await page.getByRole('button',{name:'新增关联待办',exact:true}).count(),0);
 await page.getByRole('button',{name:'恢复维护',exact:true}).click();await page.waitForFunction(()=>window.__scopes[0].status==='active');
 await page.getByRole('button',{name:'查看全部关注与约定',exact:true}).click();
 await page.getByRole('button',{name:'候选关注',exact:true}).click();await page.getByRole('button',{name:'启用这个约定',exact:true}).click();
 await page.waitForFunction(()=>window.__scopes[1].status==='active');assert.equal(await page.evaluate(()=>window.__todos().length),1);
 await page.getByRole('button',{name:'新增约定',exact:true}).click();await page.getByLabel('交给谁').selectOption('nana');
 await page.getByLabel('标题',{exact:true}).fill('读书摘录<script>');await page.getByLabel('持续维护什么').fill('整理读书摘录');await page.getByLabel('遇到什么信息时更新').fill('用户分享新摘录');
 await page.evaluate(()=>window.__fail=true);await page.getByRole('button',{name:'保存并启用',exact:true}).click();await page.getByRole('alert').waitFor();
 assert.equal(await page.getByLabel('标题',{exact:true}).inputValue(),'读书摘录<script>');
 await page.evaluate(()=>window.__fail=false);await page.getByRole('button',{name:'保存并启用',exact:true}).click();await page.getByRole('heading',{name:'读书摘录<script>',exact:true}).waitFor();
 assert.equal(await page.locator('.agreement-card script').count(),0);assert.equal(await page.evaluate(()=>window.__todos().length),1);
 await page.screenshot({path:path.join(out,'desktop.png'),fullPage:true});
 await page.setViewportSize({width:390,height:844});await page.screenshot({path:path.join(out,'narrow.png'),fullPage:true});
 assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);assert.deepEqual(errors,[]);
 console.log('PASS desktop agreement UI: linked todo origin/navigation, independent lifecycle/counts, activation, pause/resume, failed-save drafts, safe content and narrow layout');
}finally{await browser?.close();await new Promise(resolve=>server.close(resolve));}
