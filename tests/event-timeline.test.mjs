// Real ledger, SQLite persistence and recall policies; no desktop or model calls.
import { mkdir, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
const root = process.cwd();
const source = file => path.join(root, 'src-tauri/src', file).replaceAll('\\', '/');
const dir = path.join(root, 'tmp/event-timeline-tests');
await mkdir(path.join(dir, 'src'), { recursive: true });
await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-event-timeline-tests"
version="0.1.0"
edition="2021"
[dependencies]
vivian-storage-contracts={path="${path.join(root,'src-tauri/storage-contracts').replaceAll('\\','/')}"}
serde={version="1",features=["derive"]}
serde_json="1"
parking_lot="0.12"
once_cell="1"
rusqlite={version="0.32",features=["bundled","backup"]}
chrono="0.4"
rand="0.9"
uuid={version="1",features=["v4"]}
tracing="0.1"
tokio={version="1",features=["macros","rt-multi-thread","time","sync"]}
tempfile="3"
sha2="0.10"
`);
await writeFile(path.join(dir,'src/lib.rs'), `#![allow(dead_code)]
extern crate self as tauri;
#[derive(Clone)]pub struct AppHandle;
pub trait Emitter {fn emit<T:serde::Serialize>(&self,_:&str,_:T)->Result<(),String>{Ok(())}}
impl Emitter for AppHandle{}
pub mod async_runtime {pub fn spawn<F:std::future::Future+Send+'static>(f:F)->tokio::task::JoinHandle<F::Output> where F::Output:Send+'static {
 static RUNTIME:std::sync::OnceLock<tokio::runtime::Runtime>=std::sync::OnceLock::new();
 RUNTIME.get_or_init(||tokio::runtime::Runtime::new().unwrap()).spawn(f)
}}
pub use vivian_storage_contracts::error;
pub mod utils {
 pub use vivian_storage_contracts::utils::sqlite;
 pub mod path {pub fn get_companion_shared_dir()->std::path::PathBuf {
  static ROOT:std::sync::OnceLock<tempfile::TempDir>=std::sync::OnceLock::new();
  ROOT.get_or_init(||tempfile::tempdir().unwrap()).path().join("shared")
 }}
}
pub mod providers {
 pub mod base {pub struct LLMRequest;impl LLMRequest{pub fn new(_:&str,_:Vec<crate::types::response::ChatMessage>)->Self{Self}}}
 pub mod router {pub struct ModelRouter;impl ModelRouter {pub async fn generate(&self,_:super::base::LLMRequest)->crate::error::VivianResult<String>{Err(crate::error::VivianError::Memory("offline fixture".into()))}}}
}
pub mod types {pub mod response {pub struct ChatMessage;impl ChatMessage{pub fn user(_:impl Into<String>)->Self{Self}}}}
pub mod pipeline {pub mod prompt_modules {
 pub fn normalize_lang(lang:&str)->&'static str{match lang{"en"=>"en","ja"=>"ja",_=>"zh"}}
 pub fn section_heading(_:&str,_:&str)->String{"最近环境事件".into()}
}}
pub mod i18n{pub fn get_language()->String{"zh".into()}}
pub mod memory {
 #[path="${source('memory/event_time.rs')}"] pub mod event_time;
 #[path="${source('memory/unified_event_ledger.rs')}"] pub mod unified_event_ledger;
}
pub mod discovery {
 #[path="${source('discovery/learning_interest.rs')}"] pub mod learning_interest;
 #[path="${source('discovery/learning_events.rs')}"] pub mod learning_events;
}
#[cfg(test)] mod learning_episodes {
 use crate::{discovery::{learning_events::LearningEpisode,learning_interest::{PlannedSearch,LearningAnchor}},memory::unified_event_ledger::unified_event_ledger};
 fn plan()->PlannedSearch {PlannedSearch{query:"游戏叙事留白比较".into(),question:"留白如何改变玩家的理解？".into(),personal_motive:"我想比较喜欢的叙事方式".into(),anchor:LearningAnchor::new("memory:receipt","memory","这是实际记忆里关于游戏叙事留白的讨论")}}
 #[test] fn actual_learning_stages_are_private_linked_and_collapsed_in_scene_context() {
  let mut episode=LearningEpisode::start("learning-owner",&plan());episode.read(2);episode.saved("saved-id","一条实际保存的知识");
  let ledger=unified_event_ledger();let raw=ledger.recent_events_visible_to("learning-owner",10);
  assert_eq!(raw.len(),3);assert!(raw.iter().all(|e|e.timeline.caused_by.as_deref()==Some("memory:receipt")));
  let scene=ledger.recent_scene_events_visible_to("learning-owner",10);assert_eq!(scene.len(),1);
  assert_eq!(scene[0].timeline.phase.as_deref(),Some("learned"));assert_eq!(scene[0].timeline.sequence,3);
  assert_eq!(scene[0].timeline.memory_id.as_deref(),Some("saved-id"));assert!(scene[0].timeline.ended_at.is_some());
  assert!(ledger.recent_events_visible_to("unrelated-pet",10).is_empty());
 }
 #[test] fn cancellation_closes_the_episode_without_inventing_a_completed_read() {
  {let _episode=LearningEpisode::start("cancel-owner",&plan());}
  let scene=unified_event_ledger().recent_scene_events_visible_to("cancel-owner",10);
  assert_eq!(scene.len(),1);assert_eq!(scene[0].timeline.phase.as_deref(),Some("interrupted"));assert!(scene[0].timeline.memory_id.is_none());
 }
}
`);
const run = spawnSync('cargo',['test','--offline','--manifest-path',path.join(dir,'Cargo.toml'),'-j','1','--','--test-threads=1'], {
 cwd:root,stdio:'inherit',env:{...process.env,CARGO_INCREMENTAL:'0'},
});
if(run.error) throw run.error;
process.exitCode=run.status??1;
