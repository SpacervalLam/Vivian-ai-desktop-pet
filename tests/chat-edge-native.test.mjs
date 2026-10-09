import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
const source=await readFile('src-tauri/src/edge_menu.rs','utf8');
const body=source.match(/pub fn outside_pointer_down\([\s\S]*?\n\}/)?.[0]; assert.ok(body);
const dir=await mkdtemp(path.join(tmpdir(),'vivian-edge-dismiss-'));
try {
 const input=path.join(dir,'check.rs'), exe=path.join(dir,'check.exe');
 await writeFile(input,`
use std::sync::atomic::{AtomicBool,Ordering};
static DISMISS_CHAT:AtomicBool=AtomicBool::new(false);
static CAPTURE:AtomicBool=AtomicBool::new(false);
static DESTROYED:AtomicBool=AtomicBool::new(false);
mod commands { pub mod desktop_assistant { pub fn screen_capture_in_progress()->bool {crate::CAPTURE.load(std::sync::atomic::Ordering::Acquire)} } }
struct Position {x:i32,y:i32} struct Size {width:u32,height:u32}
struct WebviewWindow {x:i32,y:i32,visible:bool,fail:bool}
impl WebviewWindow {
 fn outer_position(&self)->Result<Position,()> {if self.fail {Err(())} else {Ok(Position{x:self.x,y:self.y})}}
 fn outer_size(&self)->Result<Size,()> {Ok(Size{width:60,height:800})}
 fn is_visible(&self)->Result<bool,()> {Ok(self.visible)}
 fn destroy(&self)->Result<(),()> {DESTROYED.store(true,Ordering::Release);Ok(())}
}
struct AppHandle {chat:Option<WebviewWindow>,menu:Option<WebviewWindow>}
impl AppHandle {fn get_webview_window(&self,label:&str)->Option<&WebviewWindow> {if label=="chat" {self.chat.as_ref()} else {self.menu.as_ref()}}}
${body.replace('pub fn outside_pointer_down','fn outside_pointer_down')}
fn main() {
 let mut app=AppHandle{chat:Some(WebviewWindow{x:-200,y:50,visible:true,fail:false}),menu:Some(WebviewWindow{x:0,y:50,visible:true,fail:false})};
 outside_pointer_down(&app,400,300);assert!(!DESTROYED.load(Ordering::Acquire));
 DISMISS_CHAT.store(true,Ordering::Release);
 outside_pointer_down(&app,-190,100);assert!(!DESTROYED.load(Ordering::Acquire));
 outside_pointer_down(&app,30,100);assert!(!DESTROYED.load(Ordering::Acquire));
 CAPTURE.store(true,Ordering::Release);outside_pointer_down(&app,400,300);assert!(!DESTROYED.load(Ordering::Acquire));
 CAPTURE.store(false,Ordering::Release);app.chat.as_mut().unwrap().fail=true;outside_pointer_down(&app,400,300);assert!(!DESTROYED.load(Ordering::Acquire));
 app.chat.as_mut().unwrap().fail=false;outside_pointer_down(&app,400,300);assert!(DESTROYED.load(Ordering::Acquire));
 DESTROYED.store(false,Ordering::Release);app.chat.as_mut().unwrap().visible=false;outside_pointer_down(&app,400,300);assert!(!DESTROYED.load(Ordering::Acquire));
}
`);
 const compile=spawnSync('rustc',['--edition=2021',input,'-o',exe],{encoding:'utf8'}); assert.equal(compile.status,0,compile.stderr||String(compile.error));
 const run=spawnSync(exe,[],{encoding:'utf8'});assert.equal(run.status,0,run.stderr);
 console.log('Native dismissal: negative coordinates, inside chat/menu, capture, bad geometry, hidden windows and outside destruction passed');
}finally{ await rm(dir,{recursive:true,force:true}); }
