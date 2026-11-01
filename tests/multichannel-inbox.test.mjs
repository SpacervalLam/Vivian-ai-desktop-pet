import { mkdir, writeFile, copyFile, readFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
const dir = path.join(root, 'tmp', 'multichannel-inbox-tests');
const source = name => path.join(root, 'src-tauri/src', name).replaceAll('\\', '/');
const runtime = (await readFile(source('brain/interaction_runtime.rs'),'utf8')).replaceAll('\r\n','\n');
const productionRuntime = name => {
 const found=runtime.match(new RegExp(`pub\\(crate\\) (?:async )?fn ${name}\\([\\s\\S]*?\\n\\}`))?.[0];
 if(!found)throw new Error(`Missing runtime ${name}`);return found;
};
const receipt=runtime.slice(runtime.indexOf('pub(crate) struct UserReceipt'),runtime.indexOf('pub(crate) fn receive_peer'));
const notices=(await readFile(source('brain/work_notices.rs'),'utf8')).replaceAll('\r\n','\n');
const reportId=notices.match(/fn report_message_id\([\s\S]*?\n\}/)?.[0];
const workFacts=notices.match(/    pub fn inbox_facts_for\([\s\S]*?\n    \}/)?.[0];
if(!reportId||!workFacts)throw new Error('Missing work transport helpers');
await mkdir(path.join(dir, 'src'), {recursive:true});
await copyFile(path.join(root,'src-tauri/Cargo.lock'),path.join(dir,'Cargo.lock'));
await writeFile(path.join(dir,'Cargo.toml'),`[package]
name="vivian-multichannel-inbox-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
chrono="0.4"
tracing="0.1"
parking_lot="0.12"
tempfile="3"
tokio={version="1",features=["rt","macros","sync","time"]}
`);
await writeFile(path.join(dir,'src/lib.rs'),`#![allow(dead_code)]
#[path="${source('mind/inbox.rs')}"] mod inbox;
mod mind {pub mod inbox {pub use crate::inbox::*;}}
mod utils { pub mod fs {
 pub fn load_json_or_backup<T:serde::de::DeserializeOwned>(path:&std::path::Path)->Option<T>{serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()}
 pub fn write_atomic(path:&std::path::Path,text:&str)->std::io::Result<()>{std::fs::write(path,text)}
}}
#[path="${source('utils/background_priority.rs')}"] mod background_priority;
#[path="${source('utils/session_coordinator.rs')}"] mod session_coordinator;
#[derive(Clone,Default)] pub struct AppHandle(Arc<parking_lot::Mutex<Vec<serde_json::Value>>>);
impl AppHandle {fn emit<T:serde::Serialize>(&self,_:&str,payload:T)->Result<(),()>{self.0.lock().push(serde_json::to_value(payload).unwrap());Ok(())}}
use std::sync::Arc;
mod tauri {pub use crate::AppHandle;}
mod state {
 use super::*;
 #[derive(Clone)] pub struct Character {pub brain:crate::brain::Brain}
 pub struct AppState {pub characters:std::collections::HashMap<String,Character>,pub busy:std::sync::atomic::AtomicBool}
 impl AppState {pub fn get_character(&self,id:Option<&str>)->Result<Character,String>{self.characters.get(id.unwrap()).cloned().ok_or("missing role".into())}}
}
mod brain {
 use super::*;
 #[derive(Clone)] pub struct Brain {pub char_id:String,pub mind:Arc<Mind>}
 pub struct Mind {pub inbox:Arc<inbox::Inbox>,pub cognition_wake:tokio::sync::Notify,pub heard:parking_lot::Mutex<Vec<(String,String)>>}
 impl Mind {
  pub fn prepared_peer_message_id(&self,_:&str)->Option<String>{Some("actual-question".into())}
  pub fn prepared_personal_event(&self,_:&str)->Option<String>{Some("thought:mechanism:2".into())}
  pub fn receive_peer_statement(&self,_:&str,id:&str,text:&str,_:Option<String>,_:i64){let mut heard=self.heard.lock();if !heard.iter().any(|(old,_)|old==id){heard.push((id.into(),text.into()));}}
 }
 pub mod runtime {
  use super::*;use crate::{tauri,state::AppState,inbox::{InboxMessage,MessageKind,MessageState}};
  ${receipt}
  ${productionRuntime('receive_peer')}
  ${productionRuntime('process_peer')}
 }
}
mod world {pub mod environment_agent {
 pub fn allows_peer_contact(state:&crate::state::AppState,_:&str,_:&str)->bool{!state.busy.load(std::sync::atomic::Ordering::SeqCst)}
}}
mod cross_character {
 use super::*;
 pub struct CrossCharacterRequest{pub source_id:String,pub target_id:String,pub message:String,pub source_message_id:Option<String>,pub stream_id:String}
 pub struct CrossCharacterReply{pub message_id:Option<String>,pub reply:String,pub conv_state:String}
 pub struct Bus {pub calls:parking_lot::Mutex<Vec<CrossCharacterRequest>>}
 pub static CROSS_CHARACTER_BUS:Bus=Bus{calls:parking_lot::Mutex::new(Vec::new())};
 impl Bus {pub async fn process_received(&self,_:&AppHandle,_:&Arc<state::AppState>,req:CrossCharacterRequest)->Result<CrossCharacterReply,String>{
  let reply=CrossCharacterReply{message_id:Some(format!("reply:{}",req.source_message_id.as_deref().unwrap())),reply:"这是原问题的一个回答".into(),conv_state:"active".into()};
  self.calls.lock().push(req);Ok(reply)
 }}
}
mod work_transport {
 #[derive(Clone)] pub struct WorkReport{pub session_id:String,pub title:String,pub body:String}
 pub struct Entry{pub char_id:String,pub report:WorkReport,pub at:f64}
 pub struct Alert{pub notice_id:String,pub session_id:String,pub title:String,pub body:String}
 pub struct AlertEntry{pub char_id:String,pub alert:Alert,pub at:f64}
 pub struct Store{pub entries:parking_lot::RwLock<Vec<Entry>>,pub alerts:parking_lot::RwLock<Vec<AlertEntry>>}
 ${reportId}
 impl Store{${workFacts}}
 #[test] fn simultaneous_results_have_stable_distinct_ids_and_transport_reads_do_not_ack_user_notices(){
  assert_eq!(report_message_id("task",100.0,"完成","结果A"),report_message_id("task",100.0,"完成","结果A"));
  assert_ne!(report_message_id("task",100.0,"完成","结果A"),report_message_id("task",100.0,"完成","结果B"));
  let store=Store{entries:parking_lot::RwLock::new(vec![Entry{char_id:"vivian".into(),at:100.0,report:WorkReport{session_id:"own-task".into(),title:"完成".into(),body:"自己的结果".into()}},
   Entry{char_id:"nana".into(),at:100.0,report:WorkReport{session_id:"other-task".into(),title:"完成".into(),body:"其他角色的结果".into()}}]),
   alerts:parking_lot::RwLock::new(vec![AlertEntry{char_id:"vivian".into(),at:101.0,alert:Alert{notice_id:"actual-callback".into(),session_id:"own-task".into(),title:"回调".into(),body:"须转达的结果".into()}}])};
  let facts=store.inbox_facts_for("vivian");assert_eq!(facts.len(),2);assert!(facts.iter().all(|(_,report,_)|report.session_id=="own-task"));
  assert_eq!(facts[1].0,"actual-callback");assert_eq!(store.entries.read().len(),2);assert_eq!(store.alerts.read().len(),1);
 }
}
mod memory {
 #[derive(Default)] pub struct MemoryManager(parking_lot::Mutex<Option<String>>);
 impl MemoryManager {pub fn get_session_id(&self)->Option<String>{self.0.lock().clone()} pub fn set_session_id(&self,id:Option<String>){*self.0.lock()=id;}}
}
mod dialogue {
 #[derive(Default)] pub struct DialogueManager{session:parking_lot::Mutex<Option<String>>,channel:parking_lot::Mutex<String>}
 impl DialogueManager {
 pub fn get_session_id(&self)->Option<String>{self.session.lock().clone()} pub fn set_session_id(&self,id:Option<String>){*self.session.lock()=id;}
 pub fn get_channel(&self)->String{self.channel.lock().clone()} pub fn set_channel(&self,channel:&str){*self.channel.lock()=channel.into();}
 }
}
#[cfg(test)] mod integration {
 use super::*; use std::sync::Arc; use inbox::{Inbox,InboxMessage,MessageKind,MessageState};
 fn message(id:&str,kind:MessageKind)->InboxMessage {InboxMessage::new(id.into(),match kind {MessageKind::User=>"user",MessageKind::WorkResult=>"work",_=>"nana"}.into(),
 "vivian".into(),match kind {MessageKind::User=>"wechat",MessageKind::WorkResult=>"work",_=>"cross_character"}.into(),format!("thread:{id}"),kind,"真实内容".into(),100)}
 #[test] fn each_pending_input_owns_its_receipt_and_timeouts_cannot_stick_or_erase_other_channels(){
  let coordinator=session_coordinator::SessionCoordinator::new();
  let first=coordinator.signal_user_input("vivian");let second=coordinator.signal_user_input("vivian");let nana=coordinator.signal_user_input("nana");
  first.admitted();assert!(coordinator.has_pending_user("vivian"));
  drop(second);assert!(!coordinator.has_pending_user("vivian"));assert!(coordinator.any_user_turn_active());
  drop(nana);assert!(!coordinator.any_user_turn_active());
 }
 #[tokio::test] async fn user_peer_and_work_are_received_while_a_reply_is_still_in_flight(){
  let dir=tempfile::tempdir().unwrap();let inbox=Arc::new(Inbox::load(dir.path().join("inbox.json")));
  let coordinator=session_coordinator::SessionCoordinator::new();let memory=Arc::new(memory::MemoryManager::default());let dialogue=Arc::new(dialogue::DialogueManager::default());
  let pending=coordinator.signal_user_input("vivian");let turn=coordinator.enter_user_turn("vivian","user-thread",&memory,&dialogue);pending.admitted();
  inbox.receive(message("user",MessageKind::User)).unwrap();inbox.claim("user",100).unwrap();
  let (finish,wait)=tokio::sync::oneshot::channel();let worker_inbox=inbox.clone();
  let worker=tokio::spawn(async move {wait.await.unwrap();worker_inbox.settle("user",MessageState::Completed,None).unwrap();});
  inbox.receive(message("peer",MessageKind::PeerRequest)).unwrap();inbox.receive(message("work",MessageKind::WorkResult)).unwrap();
  assert_eq!(inbox.status()["processing"],1);assert_eq!(inbox.status()["queued"],2);assert!(!worker.is_finished());
  assert!(coordinator.try_enter_proactive_turn("vivian",&memory,&dialogue).is_none());
  assert_eq!(inbox.get("work").unwrap().channel,"work");assert_eq!(inbox.get("peer").unwrap().sender,"nana");
  finish.send(()).unwrap();worker.await.unwrap();drop(turn);
  assert!(coordinator.try_enter_cross_turn("vivian","peer-thread",&memory,&dialogue).is_some());
  assert!(inbox.claim("peer",101).unwrap());
 }
 #[tokio::test] async fn cancellation_restores_the_channel_and_session_before_a_new_lane_runs(){
  let coordinator=Arc::new(session_coordinator::SessionCoordinator::new());let memory=Arc::new(memory::MemoryManager::default());let dialogue=Arc::new(dialogue::DialogueManager::default());
  dialogue.set_channel("wechat");memory.set_session_id(Some("before".into()));dialogue.set_session_id(Some("before".into()));
  let (started,ready)=tokio::sync::oneshot::channel();let c=coordinator.clone();let m=memory.clone();let d=dialogue.clone();
  let task=tokio::spawn(async move {let _channel=c.enter_channel(&d,"cross_character");let _turn=c.try_enter_cross_turn("vivian","peer",&m,&d).unwrap();started.send(()).unwrap();std::future::pending::<()>().await;});
  ready.await.unwrap();assert_eq!(dialogue.get_channel(),"cross_character");task.abort();let _=task.await;
  assert_eq!(dialogue.get_channel(),"wechat");assert_eq!(memory.get_session_id().as_deref(),Some("before"));assert!(coordinator.current_turn_kind("vivian").is_none());
 }
 #[test] fn an_old_completion_cannot_erase_a_new_turn_or_its_reply_route(){
  let c=session_coordinator::SessionCoordinator::new();let m=Arc::new(memory::MemoryManager::default());let d=Arc::new(dialogue::DialogueManager::default());
  let old=c.enter_user_turn("vivian","old",&m,&d);let new=c.enter_user_turn("vivian","new",&m,&d);drop(old);
  assert_eq!(d.get_session_id().as_deref(),Some("new"));assert!(c.any_user_turn_active());drop(new);
 }
 #[test] fn replies_retain_transport_parent_and_concern_without_becoming_another_request(){
  let dir=tempfile::tempdir().unwrap();let path=dir.path().join("inbox.json");let inbox=Inbox::load(path.clone());
  let mut reply=message("reply",MessageKind::PeerReply);reply.in_reply_to=Some("original-message".into());reply.concern_id=Some("thought:mechanism:2".into());
  inbox.receive(reply).unwrap();let restored=Inbox::load(path);
  assert!(restored.pending(MessageKind::PeerRequest).is_empty());let received=restored.pending(MessageKind::PeerReply).remove(0);
  assert_eq!(received.in_reply_to.as_deref(),Some("original-message"));assert_eq!(received.concern_id.as_deref(),Some("thought:mechanism:2"));
 }
 fn state(root:&std::path::Path)->Arc<crate::state::AppState>{
  Arc::new(crate::state::AppState{busy:std::sync::atomic::AtomicBool::new(true),characters:["vivian","nana"].into_iter().map(|id|{
   let brain=brain::Brain{char_id:id.into(),mind:Arc::new(brain::Mind{inbox:Arc::new(Inbox::load(root.join(format!("{id}.json")))),cognition_wake:tokio::sync::Notify::new(),heard:parking_lot::Mutex::new(Vec::new())})};
   (id.into(),crate::state::Character{brain})}).collect()})
 }
 #[tokio::test] async fn actual_transport_accepts_while_busy_then_routes_a_correlated_reply_without_a_reply_loop(){
  let dir=tempfile::tempdir().unwrap();let state=state(dir.path());let app=AppHandle::default();
  let request=||cross_character::CrossCharacterRequest{source_id:"nana".into(),target_id:"vivian".into(),message:"反馈如何影响选择".into(),source_message_id:None,stream_id:"source-stream".into()};
  let id=brain::runtime::receive_peer(&app,&state,&request()).unwrap();assert_eq!(id,"actual-question");
  brain::runtime::receive_peer(&app,&state,&request()).unwrap();
  let target=state.get_character(Some("vivian")).unwrap();assert_eq!(target.brain.mind.heard.lock().len(),1);assert_eq!(app.0.lock().len(),1);
  brain::runtime::process_peer(app.clone(),state.clone(),"vivian".into()).await;
  assert_eq!(target.brain.mind.inbox.get(&id).unwrap().state,MessageState::Queued);assert!(cross_character::CROSS_CHARACTER_BUS.calls.lock().is_empty());
  state.busy.store(false,std::sync::atomic::Ordering::SeqCst);
  brain::runtime::process_peer(app.clone(),state.clone(),"vivian".into()).await;
  assert_eq!(target.brain.mind.inbox.get(&id).unwrap().state,MessageState::Completed);
  let sender=state.get_character(Some("nana")).unwrap();let reply=sender.brain.mind.inbox.pending(MessageKind::PeerReply).remove(0);
  assert_eq!(reply.sender,"vivian");assert_eq!(reply.recipient,"nana");assert_eq!(reply.in_reply_to.as_deref(),Some(id.as_str()));assert_eq!(reply.concern_id.as_deref(),Some("thought:mechanism:2"));
  brain::runtime::process_peer(app,state,"nana".into()).await;assert_eq!(cross_character::CROSS_CHARACTER_BUS.calls.lock().len(),1);
 }
 #[test] fn actual_user_receipt_remains_owned_until_completion_or_caller_cancellation(){
  let dir=tempfile::tempdir().unwrap();let state=state(dir.path());let app=AppHandle::default();let brain=state.get_character(Some("vivian")).unwrap().brain;
  let guard=brain::runtime::receive_user(&app,&brain,"user-1","wechat","先收下这个问题").unwrap().unwrap();
  assert!(brain::runtime::receive_user(&app,&brain,"user-1","wechat","先收下这个问题").unwrap().is_none());
  guard.claim().unwrap();drop(guard);assert_eq!(brain.mind.inbox.get("user-1").unwrap().state,MessageState::Interrupted);
  let mut next=brain::runtime::receive_user(&app,&brain,"user-2","direct","换一个信道").unwrap().unwrap();next.claim().unwrap();next.finish(true);drop(next);
  assert_eq!(brain.mind.inbox.get("user-2").unwrap().channel,"direct");assert_eq!(brain.mind.inbox.get("user-2").unwrap().state,MessageState::Completed);
 }
}
`);
const result=spawnSync('cargo',['test','--offline','--manifest-path',path.join(dir,'Cargo.toml'),'-j','1'],{
 cwd:root,stdio:'inherit',timeout:180000,env:{...process.env,CARGO_INCREMENTAL:'0',CARGO_PROFILE_DEV_DEBUG:'0'},
});
if(result.error)throw result.error;
process.exitCode=result.status??1;
