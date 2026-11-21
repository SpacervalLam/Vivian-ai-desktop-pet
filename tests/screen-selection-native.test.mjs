import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import path from 'node:path';
const source=await readFile('src-tauri/src/screen_selection.rs','utf8');
const helper=source.match(/pub async fn hide_capture_windows\([\s\S]*?\n\}/)?.[0];assert.ok(helper);
const entryHelper=source.match(/pub async fn hide_capture_entry\([\s\S]*?\n\}/)?.[0];assert.ok(entryHelper);
const directory=await mkdtemp(path.join(tmpdir(),'vivian-capture-hide-'));
try{
 const input=path.join(directory,'test.rs'),executable=path.join(directory,'test.exe');
 await writeFile(input,`
#![allow(non_snake_case,private_interfaces)]
use std::{sync::{Arc,Mutex},future::Future,task::{Context,Poll,Wake,Waker},pin::Pin};
static LOG:Mutex<Vec<&str>>=Mutex::new(Vec::new());
mod oneshot {
 use super::*;
 pub struct Sender<T>(Arc<Mutex<Option<T>>>);pub struct Receiver<T>(Arc<Mutex<Option<T>>>);
 pub fn channel<T>()->(Sender<T>,Receiver<T>){let cell=Arc::new(Mutex::new(None));(Sender(cell.clone()),Receiver(cell))}
 impl<T> Sender<T>{pub fn send(self,value:T)->Result<(),T>{*self.0.lock().unwrap()=Some(value);Ok(())}}
 impl<T> Future for Receiver<T>{type Output=Result<T,()>;fn poll(self:Pin<&mut Self>,_:&mut Context<'_>)->Poll<Self::Output>{match self.0.lock().unwrap().take(){Some(v)=>Poll::Ready(Ok(v)),None=>Poll::Pending}}}
}
mod windows{pub mod Win32{pub mod Graphics{pub mod Dwm{pub unsafe fn DwmFlush()->Result<(),String>{crate::LOG.lock().unwrap().push("flush");Ok(())}}}}}
#[derive(Clone)]
struct AppHandle{pending:Arc<Mutex<Option<Box<dyn FnOnce()+Send>>>>}
impl AppHandle{fn get_webview_window(&self,label:&str)->Option<WebviewWindow>{Some(WebviewWindow{label:if label=="chat"{"chat"}else{"edge_menu"},fail:false})}fn run_on_main_thread(&self,f:impl FnOnce()+Send+'static)->Result<(),String>{*self.pending.lock().unwrap()=Some(Box::new(f));Ok(())}fn run(&self){self.pending.lock().unwrap().take().unwrap()();}}
#[derive(Clone)]
struct WebviewWindow{label:&'static str,fail:bool}
impl WebviewWindow{fn is_visible(&self)->Result<bool,String>{Ok(true)}fn show(&self)->Result<(),String>{LOG.lock().unwrap().push("restore");Ok(())}fn hide(&self)->Result<(),String>{LOG.lock().unwrap().push(self.label);if self.fail{Err("hide failed".into())}else{Ok(())}}}
fn disable_transitions(_: &WebviewWindow)->Result<(),String>{LOG.lock().unwrap().push("disable_animation");Ok(())}
${helper}
mod edge_menu{pub fn suspend_for_capture(){crate::LOG.lock().unwrap().push("cancel_motion");}}
${entryHelper}
struct Noop;impl Wake for Noop{fn wake(self:Arc<Self>){}}
fn main(){
 let waker=Waker::from(Arc::new(Noop));let mut context=Context::from_waker(&waker);
 let app=AppHandle{pending:Arc::new(Mutex::new(None))};
 let mut future=Box::pin(hide_capture_windows(&app,vec![WebviewWindow{label:"edge_menu",fail:false},WebviewWindow{label:"chat",fail:false}]));
 assert!(matches!(future.as_mut().poll(&mut context),Poll::Pending));assert!(LOG.lock().unwrap().is_empty(),"capture must wait for the main-thread work");
 app.run();assert!(matches!(future.as_mut().poll(&mut context),Poll::Ready(Ok(()))));
 assert_eq!(*LOG.lock().unwrap(),vec!["disable_animation","edge_menu","disable_animation","chat","flush"]);
 LOG.lock().unwrap().clear();
 let mut failed=Box::pin(hide_capture_windows(&app,vec![WebviewWindow{label:"edge_menu",fail:true}]));
 assert!(matches!(failed.as_mut().poll(&mut context),Poll::Pending));app.run();assert!(matches!(failed.as_mut().poll(&mut context),Poll::Ready(Err(_))));
 assert!(!LOG.lock().unwrap().contains(&"flush"),"failed hiding aborts instead of taking a dirty screenshot");
 LOG.lock().unwrap().clear();
 let mut entry=Box::pin(hide_capture_entry(&app));assert!(matches!(entry.as_mut().poll(&mut context),Poll::Pending));
 app.run();let Poll::Ready(Ok(restored))=entry.as_mut().poll(&mut context) else {panic!("entry hide incomplete")};
 assert_eq!(restored.len(),1);assert_eq!(restored[0].label,"chat");
 assert_eq!(*LOG.lock().unwrap(),vec!["cancel_motion","disable_animation","chat","disable_animation","edge_menu","flush"]);
 let mut empty=Box::pin(hide_capture_windows(&app,vec![]));assert!(matches!(empty.as_mut().poll(&mut context),Poll::Ready(Ok(()))));
}
`);
 const compiled=spawnSync('rustc',['--edition=2021',input,'-o',executable],{encoding:'utf8'});assert.equal(compiled.status,0,compiled.stderr||String(compiled.error));
 const result=spawnSync(executable,[],{encoding:'utf8'});assert.equal(result.status,0,result.stderr);
 const entry=await readFile('src-tauri/src/commands/desktop_assistant.rs','utf8');
 assert.ok(entry.indexOf('hide_capture_entry(app).await?')<entry.indexOf('screen_selection::select_region(app).await?'));
 console.log('Capture ordering: UI-thread completion, per-window animation disable, compositor flush and abort on hiding failure passed');
}finally{await rm(directory,{recursive:true,force:true});}
