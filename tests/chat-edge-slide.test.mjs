import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
const source=await readFile('src-tauri/src/edge_menu.rs','utf8');
const body=source.match(/fn slide_window_in\([\s\S]*?\n\}/)?.[0];assert.ok(body);
const showChat=source.match(/pub fn show_chat\([\s\S]*?\n\}/)?.[0];assert.ok(showChat);
const moveFrame=source.match(/fn move_slide_frame\([\s\S]*?\n\}/)?.[0];assert.ok(moveFrame);
const parkMenu=source.match(/fn park_menu\([\s\S]*?\n\}/)?.[0];assert.ok(parkMenu);
const dir=await mkdtemp(path.join(tmpdir(),'vivian-menu-slide-'));
try{
 const input=path.join(dir,'slide.rs'),exe=path.join(dir,'slide.exe');
 const policy=path.resolve('src-tauri/src/desktop_menu_policy.rs').replaceAll('\\','/');
 await writeFile(input,`
#![allow(dead_code)]
use std::{sync::{Arc,Mutex,atomic::{AtomicBool,AtomicU64,Ordering}},thread,time::{Duration,Instant}};
#[path=${JSON.stringify(policy)}]mod desktop_menu_policy;
static STOP:AtomicBool=AtomicBool::new(false);
static MENU_MOTION_GENERATION:AtomicU64=AtomicU64::new(0);
static CHAT_GENERATION:AtomicU64=AtomicU64::new(0);
static CHAT_MOTION_GENERATION:AtomicU64=AtomicU64::new(0);
static CHAT_ENTERING:AtomicBool=AtomicBool::new(false);
static CAPTURING:AtomicBool=AtomicBool::new(false);
mod commands{pub mod desktop_assistant{pub fn screen_capture_in_progress()->bool{crate::CAPTURING.load(std::sync::atomic::Ordering::Acquire)}}pub mod window{pub fn thaw_webview(_: &crate::WebviewWindow){}}}
mod tauri{pub struct PhysicalPosition{pub x:i32,pub y:i32}impl PhysicalPosition{pub fn new(x:i32,y:i32)->Self{Self{x,y}}}}
struct AppHandle{capture_at_dispatch:bool,right:f64,scale:f64}
impl AppHandle{fn run_on_main_thread(&self,f:impl FnOnce()+Send+'static)->Result<(),String>{if self.capture_at_dispatch{CAPTURING.store(true,Ordering::Release);}f();Ok(())}}
struct Monitor{x:i32,width:u32}impl Monitor{fn position(&self)->tauri::PhysicalPosition{tauri::PhysicalPosition::new(self.x,0)}fn size(&self)->Size{Size{width:self.width}}}
#[derive(Default)]struct State{position:i32,moves:Vec<i32>,first_visible:Option<i32>,shows:u32,visible:bool}
#[derive(Clone)]struct WebviewWindow{state:Arc<Mutex<State>>,app:Arc<AppHandle>,cancel_on_show:bool}
impl WebviewWindow{
 fn available_monitors(&self)->Result<Vec<Monitor>,String>{Ok(vec![Monitor{x:-1920,width:1920},Monitor{x:0,width:1920},Monitor{x:1920,width:1280}])}
 fn is_visible(&self)->Result<bool,String>{Ok(self.state.lock().unwrap().visible)}
 fn set_ignore_cursor_events(&self,ignore:bool)->Result<(),String>{assert!(!ignore);Ok(())}
 fn outer_size(&self)->Result<Size,String>{Ok(Size{width:390})}
 fn outer_position(&self)->Result<tauri::PhysicalPosition,String>{Ok(tauri::PhysicalPosition::new(self.state.lock().unwrap().position,100))}
 fn app_handle(&self)->&AppHandle{&self.app}
 fn set_position(&self,p:tauri::PhysicalPosition)->Result<(),String>{assert_eq!(p.y,100);let mut state=self.state.lock().unwrap();state.position=p.x;state.moves.push(p.x);Ok(())}
 fn show(&self)->Result<(),String>{let mut state=self.state.lock().unwrap();state.first_visible=Some(state.position);state.shows+=1;state.visible=true;if self.cancel_on_show{MENU_MOTION_GENERATION.fetch_add(1,Ordering::AcqRel);}Ok(())}
}
${moveFrame}
${body}
${parkMenu}
${showChat}
struct Size{width:u32}
fn geometry(app:&AppHandle)->Result<(f64,f64,f64,f64,f64),String>{Ok((app.right,100.,390.,800.,app.scale))}
fn window(capture_at_dispatch:bool,cancel_on_show:bool)->WebviewWindow{WebviewWindow{state:Arc::new(Mutex::new(State::default())),app:Arc::new(AppHandle{capture_at_dispatch,right:1920.,scale:1.}),cancel_on_show}}
fn main(){
 let chat=window(false,false);show_chat(&chat).unwrap();show_chat(&chat).unwrap();
 let until=Instant::now()+Duration::from_secs(2);while CHAT_ENTERING.load(Ordering::Acquire)&&Instant::now()<until{thread::sleep(Duration::from_millis(10));}
 assert!(!CHAT_ENTERING.load(Ordering::Acquire));assert_eq!(chat.state.lock().unwrap().first_visible,Some(1920));assert_eq!(chat.state.lock().unwrap().position,1530);assert_eq!(chat.state.lock().unwrap().shows,1,"repeated navigation during entry does not start another animation");
 let count=chat.state.lock().unwrap().moves.len();show_chat(&chat).unwrap();assert_eq!(chat.state.lock().unwrap().moves.len(),count,"focusing visible chat never moves it");
 let mut hidpi=window(false,false);hidpi.app=Arc::new(AppHandle{capture_at_dispatch:false,right:1536.,scale:1.25});show_chat(&hidpi).unwrap();let until=Instant::now()+Duration::from_secs(2);while CHAT_ENTERING.load(Ordering::Acquire)&&Instant::now()<until{thread::sleep(Duration::from_millis(10));}assert_eq!(hidpi.state.lock().unwrap().first_visible,Some(1920));assert_eq!(hidpi.state.lock().unwrap().position,1530,"DPI-scaled geometry maps to physical right edge");
 let menu=window(false,false);slide_window_in(&menu,1920,1860,100,&MENU_MOTION_GENERATION);
 let state=menu.state.lock().unwrap();assert_eq!(state.first_visible,Some(1920),"first visible frame must be outside the screen");assert_eq!(state.position,1860);assert_eq!(state.shows,1);assert!(state.moves.len()>4);assert!(state.moves.windows(2).all(|pair|pair[1]<=pair[0]));drop(state);
 let resident=window(false,false);resident.state.lock().unwrap().visible=true;slide_window_in(&resident,1920,1860,100,&MENU_MOTION_GENERATION);assert_eq!(resident.state.lock().unwrap().shows,0,"resident menu never calls show during entry");assert_eq!(resident.state.lock().unwrap().position,1860);
 park_menu(&resident).unwrap();assert_eq!(resident.state.lock().unwrap().position,3216,"parking is beyond every monitor, including adjacent displays");assert_eq!(resident.state.lock().unwrap().shows,0,"dismissal does not hide or show the resident window");
 let warming=window(false,false);park_menu(&warming).unwrap();assert_eq!(warming.state.lock().unwrap().first_visible,Some(3216),"initial compositor warmup happens outside the whole desktop");
 let capture_park=window(true,false);park_menu(&capture_park).unwrap();assert_eq!(capture_park.state.lock().unwrap().shows,0,"capture hide barrier also covers initial parking");CAPTURING.store(false,Ordering::Release);
 let negative=window(false,false);slide_window_in(&negative,0,-60,100,&MENU_MOTION_GENERATION);assert_eq!(negative.state.lock().unwrap().first_visible,Some(0));assert_eq!(negative.state.lock().unwrap().position,-60);
 let racing_capture=window(true,false);slide_window_in(&racing_capture,1920,1860,100,&MENU_MOTION_GENERATION);assert_eq!(racing_capture.state.lock().unwrap().shows,0,"capture beginning during renderer resume prevents menu reappearance");assert_eq!(racing_capture.state.lock().unwrap().position,0);CAPTURING.store(false,Ordering::Release);
 let cancelled=window(false,true);slide_window_in(&cancelled,1920,1860,100,&MENU_MOTION_GENERATION);assert_eq!(cancelled.state.lock().unwrap().shows,1);assert_eq!(cancelled.state.lock().unwrap().moves,vec![1920],"cancel never undoes dismissal geometry");
 STOP.store(true,Ordering::Release);let stopped=window(false,false);slide_window_in(&stopped,1920,1860,100,&MENU_MOTION_GENERATION);assert_eq!(stopped.state.lock().unwrap().shows,0);
}
`);
 const compiled=spawnSync('rustc',['--edition=2021',input,'-o',exe],{encoding:'utf8'});assert.equal(compiled.status,0,compiled.stderr||String(compiled.error));
 const result=spawnSync(exe,[],{encoding:'utf8'});assert.equal(result.status,0,result.stderr);
 console.log('Native chat/menu slide, chat focus/DPI: offscreen first frame, monotonic entry, negative monitor, screenshot race, cancellation and shutdown passed');
}finally{await rm(dir,{recursive:true,force:true});}
