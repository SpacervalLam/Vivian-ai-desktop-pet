// Run production activity, wakeup and work-notice policies without desktop/model services.
import { readFile, writeFile, mkdir, copyFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
const dir = path.join(root, 'tmp', 'autonomous-activity-tests');
const source = name => path.join(root, 'src-tauri/src', name).replaceAll('\\', '/');
await mkdir(path.join(dir, 'src'), { recursive: true });
await copyFile(path.join(root, 'src-tauri/Cargo.lock'), path.join(dir, 'Cargo.lock'));
await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-autonomous-activity-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
chrono="0.4"
parking_lot="0.12"
once_cell="1"
uuid={version="1",features=["v4"]}
tracing="0.1"
tokio={version="1",features=["macros","rt-multi-thread","time","sync"]}
`);
let presence = await readFile(source('presence/mod.rs'), 'utf8');
presence = presence.replace(/^pub(?:\(crate\))? mod (?:background_tasks|meme_acquisition|platform_learning|activity_budget|token_budget);\r?\n/gm, '')
  .replace(/pub use background_tasks::\{[\s\S]*?\};\r?\n/, '')
  .replace(/^pub use meme_acquisition::.*\r?\n/gm, '')
  .replace('pub mod config;', `#[path="${source('presence/config.rs')}"] pub mod config;`);
await writeFile(path.join(dir, 'src', 'presence.rs'), presence);
const background = await readFile(source('presence/background_tasks.rs'), 'utf8');
const guard = background.slice(background.indexOf('/// Owns task completion'), background.indexOf('/// 启动 Busy 知识采集任务'));
if (!guard.includes('struct PresenceTaskGuard')) throw new Error('Missing production completion guard');
const agent = await readFile(source('brain/coding_agent.rs'), 'utf8');
const guidance = agent.slice(agent.indexOf('    pub fn has_running_work_for'), agent.indexOf('    /// 发送用户消息并驱动 agent loop'));
if (!guidance.includes('queue_companion_guidance')) throw new Error('Missing production guidance handler');
const bridge = await readFile(source('tools/builtin/work_agent_tools.rs'), 'utf8');
const bridgeHelpers = bridge.slice(bridge.indexOf('fn latest_work_summary('), bridge.indexOf('// ===== delegate_to_work_agent'));
const proactive = await readFile(source('proactive/mod.rs'), 'utf8');
const delivery = proactive.slice(proactive.indexOf('    fn push_action(&self,'), proactive.indexOf('    /// 推进触发器冷却时间'));
if (!delivery.includes('record_delivered')) throw new Error('Missing production delivery attribution');
await writeFile(path.join(dir, 'src', 'lib.rs'), `#![allow(dead_code,unused_imports)]
extern crate self as tauri;
use std::sync::Arc;
use serde_json::json;
#[derive(Clone,Default)] pub struct AppHandle(Arc<parking_lot::Mutex<Vec<(String,serde_json::Value)>>>);
pub trait Emitter { fn emit<T:serde::Serialize + Clone>(&self, name:&str, data:T) -> Result<(),()>; }
impl Emitter for AppHandle { fn emit<T:serde::Serialize+Clone>(&self,name:&str,data:T)->Result<(),()> {
 self.0.lock().push((name.into(),serde_json::to_value(data).unwrap())); Ok(())
} }
pub mod error {
 #[derive(Debug)] pub enum VivianError { Other(String), Memory(String) }
 impl std::fmt::Display for VivianError { fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result { write!(f,"{:?}",self) } }
 pub type VivianResult<T> = Result<T,VivianError>;
}
pub mod utils {
 pub mod path {
  pub fn get_user_data_dir()->std::path::PathBuf { std::env::var("VIVIAN_ACTIVITY_TEST_DATA").unwrap().into() }
  pub fn get_character_data_dir(id:&str)->std::path::PathBuf { get_user_data_dir().join(id) }
  pub fn get_companion_data_dir(id:&str)->std::path::PathBuf { get_character_data_dir(id) }
  pub fn ensure_dir(path:&std::path::Path)->std::io::Result<()> { std::fs::create_dir_all(path) }
 }
 pub mod fs {
  pub fn atomic_write(path:&std::path::Path,bytes:&[u8])->std::io::Result<()> { if let Some(p)=path.parent(){std::fs::create_dir_all(p)?;} std::fs::write(path,bytes) }
  pub fn write_atomic(path:&std::path::Path,text:&str)->std::io::Result<()> { atomic_write(path,text.as_bytes()) }
 }
 #[path="${source('utils/cancel_token.rs')}"] pub mod cancel_token;
}
pub mod memory {
 pub mod manager { pub struct MemoryManager; }
 pub mod types { pub fn current_timestamp()->f64 { chrono::Utc::now().timestamp() as f64 } }
}
pub mod commands { pub mod inspector { pub fn is_viewing_work_page(_:&crate::AppHandle)->bool { false } } }
#[path="${source('presence/activity_budget.rs')}"] pub mod activity_budget;
#[path="${source('proactive/wakeup.rs')}"] pub mod wakeup;
pub mod presence;
pub use presence::PresenceConfig;
pub mod brain {
 pub mod work_question {
  #[derive(Clone)] pub struct WorkQuestionRequest { pub question_id:u64 }
  pub struct Registry;
  pub fn global_work_question_registry()->Registry { Registry }
  impl Registry { pub fn pending_for_char(&self,_:&str)->Vec<WorkQuestionRequest>{ vec![] } }
 }
 #[path="${source('brain/work_notices.rs')}"] pub mod work_notices;
 pub mod coding_event_log { pub fn record(_:&str,_:Option<&str>,_:&str,_:serde_json::Value){} }
 pub mod coding_agent {
  use super::*;
  use std::collections::HashMap;
  use serde_json::{Value,json};
  #[derive(PartialEq)] pub enum CodingStatus { Running, Idle }
  #[derive(PartialEq)] pub enum CodingRole { User, Assistant }
  pub struct TaskResult {pub summary:String}
  pub struct Task { pub result:Option<TaskResult>,pub start_message:usize }
  pub struct CodingMessage { pub role:CodingRole,pub content:String,pub images:Option<()>,pub file_refs:Option<()>,pub widgets:Option<()>,pub interjected:Option<bool>,pub guided:Option<bool>,pub tool_name:Option<String>,pub tool_arguments:Option<()>,pub tool_success:Option<bool>,pub tool_call_id:Option<String>,pub timestamp:i64 }
  pub struct CodingSession { pub char_id:String,pub mode:String,pub status:CodingStatus,pub last_reply_preview:Option<String>,pub task:Option<Task>,pub messages:Vec<CodingMessage>,pub updated_at:i64 }
  ${bridgeHelpers}
  #[derive(Default)] pub struct CodingAgentService { pub sessions:parking_lot::RwLock<HashMap<String,CodingSession>>,pub saves:std::sync::atomic::AtomicUsize }
  impl CodingAgentService {
   fn persist(&self) { self.saves.fetch_add(1,std::sync::atomic::Ordering::SeqCst); }
   ${guidance}
  }
  #[test] fn invalid_continuation_cannot_silently_create_a_duplicate_task() {
   assert!(continuation_session(&json!({"task":"new"})).unwrap().is_none());
   assert_eq!(continuation_session(&json!({"session_id":" task-1 "})).unwrap(),Some("task-1"));
   for args in [json!({"session_id":""}),json!({"session_id":42}),json!({"session_id":null}),json!({"session_id":"task-1","mode":"standard"}),json!({"session_id":"task-1","working_directory":"G:/other"})] {
    assert!(continuation_session(&args).is_err());
   }
  }
  #[test] fn summary_preview_is_bounded_and_full_details_share_the_same_task_boundary() {
   let summary="有效结果。".repeat(1200);
   let mut session=CodingSession {char_id:"vivian".into(),mode:"standard".into(),status:CodingStatus::Idle,last_reply_preview:Some("上一轮结果".into()),task:Some(Task{result:Some(TaskResult{summary:summary.clone()}),start_message:0}),messages:vec![],updated_at:0};
   assert_eq!(latest_work_summary(&session).unwrap().chars().count(),4000);
   assert_eq!(full_work_summary(&session).unwrap(),summary);
   session.task=Some(Task{result:None,start_message:0});
   assert!(full_work_summary(&session).is_none());
   session.task=None; session.last_reply_preview=Some(summary.clone());
   assert_eq!(latest_work_summary(&session).unwrap().chars().count(),4000);
   assert_eq!(full_work_summary(&session).unwrap(),summary);
  }
  #[test] fn guidance_is_owned_queued_once_and_does_not_start_a_second_task() {
   let service=CodingAgentService::default();
   service.sessions.write().insert("task".into(),CodingSession {char_id:"vivian".into(),mode:"standard".into(),status:CodingStatus::Running,last_reply_preview:Some("上一轮已经完成".into()),task:None,messages:vec![],updated_at:0});
   service.sessions.write().get_mut("task").unwrap().task=Some(Task{result:None,start_message:0});
   assert!(latest_work_summary(&service.sessions.read()["task"]).is_none(),"the previous run's preview is not the current result");
   assert!(service.has_running_work_for("vivian")); assert!(!service.has_running_work_for("nana"));
   assert!(service.queue_companion_guidance("nana","task","覆盖内容").is_err());
   assert!(service.queue_companion_guidance("vivian","task"," ").is_err());
   assert!(service.queue_companion_guidance("vivian","task","补充弹幕来源").unwrap());
   {let s=service.sessions.read();let task=&s["task"];assert_eq!(task.messages.len(),1);assert_eq!(task.messages[0].guided,Some(true));assert_eq!(task.messages[0].content,"补充弹幕来源");}
   assert_eq!(service.sessions.read().len(),1);
   service.sessions.write().get_mut("task").unwrap().task=Some(Task{result:Some(TaskResult{summary:"完成".into()}),start_message:0});
   assert!(service.queue_companion_guidance("vivian","task","收尾时补充").is_err());
   service.sessions.write().get_mut("task").unwrap().status=CodingStatus::Idle;
   assert!(!service.queue_companion_guidance("vivian","task","下一轮").unwrap());
  }
 }
}
pub mod character_behavior {
 pub struct SpeechDesire { pub reset_on_speak:bool,pub initial_desire:f64 }
 pub struct Behavior { pub speech_desire:SpeechDesire }
 pub fn get_behavior(_:&str)->Behavior { Behavior {speech_desire:SpeechDesire{reset_on_speak:true,initial_desire:0.2}} }
}
pub mod delivery_fixture {
 use super::*;
 use parking_lot::RwLock;
 #[derive(Debug,Copy,Clone,PartialEq)] enum DeliveryChannel { Bubble }
 #[derive(Clone)] struct ProactiveAction {work_notice_id:Option<String>,wakeup_id:Option<String>,content:String,delivery_channel:DeliveryChannel,trigger:String}
 #[derive(Copy,Clone)] enum ProactiveTrigger { Spontaneous,WelcomeBack,HourlyGreeting,IdleGreeting,Icebreaker }
 impl ProactiveTrigger {
  fn all()-> &'static [Self] { &[Self::Spontaneous,Self::WelcomeBack,Self::HourlyGreeting,Self::IdleGreeting,Self::Icebreaker] }
  fn as_str(&self)-> &'static str { match self { Self::Spontaneous=>"spontaneous",Self::WelcomeBack=>"welcome_back",Self::HourlyGreeting=>"hourly_greeting",Self::IdleGreeting=>"idle_greeting",Self::Icebreaker=>"icebreaker" } }
 }
 #[derive(Default)] struct State {consecutive_interruptions:u32,last_interruption_at:f64,pending_proactive_at:f64}
 struct Config {backoff_max_level:u32}
 #[derive(Default)] struct Learner(std::sync::atomic::AtomicUsize);
 impl Learner {fn record_trigger_fired(&self,_:ProactiveTrigger){self.0.fetch_add(1,std::sync::atomic::Ordering::SeqCst);}}
 struct Orchestrator {pending_messages:RwLock<Vec<ProactiveAction>>,recent_sent_contents:RwLock<Vec<(String,DeliveryChannel)>>,config:RwLock<Config>,state:RwLock<State>,preference_learner:Learner,speech_desire:RwLock<f64>,char_id:String}
 impl Default for Orchestrator {fn default()->Self {Self{pending_messages:RwLock::new(vec![]),recent_sent_contents:RwLock::new(vec![]),config:RwLock::new(Config{backoff_max_level:5}),state:RwLock::new(State::default()),preference_learner:Learner::default(),speech_desire:RwLock::new(0.9),char_id:"test".into()}}}
 impl Orchestrator { ${delivery} }
 fn action()->ProactiveAction {ProactiveAction{work_notice_id:None,wakeup_id:None,content:"下午好".into(),delivery_channel:DeliveryChannel::Bubble,trigger:"hourly_greeting".into()}}
 #[test] fn queueing_and_dropping_unseen_messages_does_not_teach_rejection_or_suppress_retries() {
  let orch=Orchestrator::default();let action=action();
  orch.push_action(action.clone(),ProactiveTrigger::HourlyGreeting);
  assert_eq!(orch.state.read().consecutive_interruptions,0);assert_eq!(*orch.speech_desire.read(),0.9);
  orch.pending_messages.write().clear();
  orch.push_action(action.clone(),ProactiveTrigger::HourlyGreeting);assert_eq!(orch.pending_messages.read().len(),1);
  orch.pending_messages.write().clear();orch.record_delivered(&action,100.0);
  assert_eq!(orch.state.read().consecutive_interruptions,1);assert_eq!(orch.state.read().pending_proactive_at,100.0);assert_eq!(*orch.speech_desire.read(),0.2);
  orch.push_action(action,ProactiveTrigger::HourlyGreeting);assert!(orch.pending_messages.read().is_empty());
 }
 #[test] fn work_results_and_promises_do_not_count_as_ignored_small_talk() {
  let orch=Orchestrator::default();let mut work=action();work.work_notice_id=Some("notice".into());
  orch.record_delivered(&work,100.0);let mut wake=action();wake.wakeup_id=Some("wakeup".into());orch.record_delivered(&wake,101.0);
  assert_eq!(orch.state.read().consecutive_interruptions,0);assert_eq!(orch.state.read().pending_proactive_at,0.0);
  assert_eq!(orch.preference_learner.0.load(std::sync::atomic::Ordering::SeqCst),0);
 }
 #[test] fn concurrent_duplicate_actions_only_enter_the_pending_queue_once() {
  let orch=Arc::new(Orchestrator::default());let threads:Vec<_>=(0..8).map(|_|{let orch=orch.clone();std::thread::spawn(move||orch.push_action(action(),ProactiveTrigger::Spontaneous))}).collect();
  for thread in threads {thread.join().unwrap();} assert_eq!(orch.pending_messages.read().len(),1);
 }
}
pub mod background_guard {
 use super::*;
 use presence::PresenceManager;
 use memory::manager::MemoryManager;
 fn emit_presence_changed(_:&AppHandle,_:&str,_:&presence::PresenceEvent){}
 async fn write_presence_log(_:&MemoryManager,_:&str,_:&presence::PresenceEvent){}
 ${guard}
 #[tokio::test] async fn abort_before_first_poll_closes_the_activity_and_honors_a_pending_wake() {
  let manager=Arc::new(PresenceManager::new_with_temp_dir("aborted-guard").unwrap());
  manager.set_busy_task_spawner(Arc::new(||{}));
  manager.transition(presence::PresenceState::Busy,presence::PresenceChangeReason::UserLeft).unwrap();
  manager.transition(presence::PresenceState::Online,presence::PresenceChangeReason::UserInteraction);
  let app=AppHandle::default();let memory=Arc::new(MemoryManager);
  let guard=PresenceTaskGuard::new("test","knowledge_acquisition",&app,&manager,&memory);
  let task=tokio::spawn(async move {let _guard=guard;std::future::pending::<()>().await;});
  task.abort();let _=task.await;
  assert!(!manager.is_task_in_progress());assert_eq!(manager.current(),presence::PresenceState::Online);
  assert_eq!(app.0.lock().iter().filter(|(name,_)|name=="presence:task_finished").count(),1);
 }
 #[tokio::test] async fn finishing_normally_then_dropping_does_not_send_a_second_completion() {
  let app=AppHandle::default();let mut guard=PresenceTaskGuard::detached("nana","cognition",&app);
  guard.finish(json!({"status":"finished"}));drop(guard);
  assert_eq!(app.0.lock().len(),1);
 }
}
`);
const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1', '--', '--test-threads=1'], {
  cwd: root, stdio: 'inherit', timeout: 180000,
  env: { ...process.env, VIVIAN_ACTIVITY_TEST_DATA: path.join(dir, 'data'), CARGO_INCREMENTAL: '0', CARGO_PROFILE_DEV_DEBUG: '0' },
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
