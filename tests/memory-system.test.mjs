// Run real Rust grouping, taxonomy, chunk planner and pipeline code with local I/O/LLM doubles.
// The harness never opens application data or contacts a model provider.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const dir = path.join(root, 'tmp', 'memory-system-tests');
await mkdir(path.join(dir, 'src'), { recursive: true });
const rustPath = (file) => path.join(root, file).replaceAll('\\', '/');
const cross = await readFile(path.join(root, 'src-tauri/src/cross_character.rs'), 'utf8');
const history = await readFile(path.join(root, 'src-tauri/src/dialogue/mod.rs'), 'utf8');
// Include the current production prefix parser and HistoryEntry definition, rather than copies.
const strip = cross.slice(cross.indexOf('fn strip_memory_anchor('), cross.indexOf('/// 清洗用于组装'));
const prefix = cross.slice(cross.indexOf('pub fn parse_any_speaker_prefix('), cross.indexOf('/// 解析 `[X says to me]`'));
const entry = history.slice(history.indexOf('pub struct HistoryEntry {'), history.indexOf('\n/// 对话缓冲区 flush', history.indexOf('pub struct HistoryEntry {')));
await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-memory-system-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
uuid={version="1",features=["v4"]}
schemars="0.8"
sha2="0.10"
tracing="0.1"
tokio={version="1",features=["macros","rt-multi-thread","sync"]}
parking_lot="0.12"
`);
let source = String.raw`
#![allow(dead_code)]
#[cfg(test)] use std::sync::Arc;
pub mod error {
    #[derive(Debug)] pub enum VivianError { Memory(String), Json(serde_json::Error) }
    impl From<serde_json::Error> for VivianError { fn from(e:serde_json::Error)->Self{Self::Json(e)} }
    impl From<std::io::Error> for VivianError { fn from(e:std::io::Error)->Self{Self::Memory(e.to_string())} }
    pub type VivianResult<T> = Result<T,VivianError>;
}
pub mod utils { pub mod fs {
    pub fn load_json_or_backup<T:serde::de::DeserializeOwned>(p:&std::path::Path)->Option<T>{std::fs::read_to_string(p).ok().and_then(|s|serde_json::from_str(&s).ok())}
    pub fn write_atomic(p:&std::path::Path,text:&str)->std::io::Result<()>{if let Some(parent)=p.parent(){std::fs::create_dir_all(parent)?;}std::fs::write(p,text)}
} }
pub mod config { pub mod manager {
    #[derive(Clone)] pub struct ConsolidationConfig { pub stage1_idle_timeout_sec:f64, pub stage1_short_term_threshold:usize }
} }
pub mod types { pub mod response {
    pub struct ChatMessage { pub content:String }
    impl ChatMessage { pub fn user(text:impl Into<String>)->Self{Self{content:text.into()}} }
} }
pub mod providers {
    use std::collections::VecDeque;
    use parking_lot::Mutex;
    pub mod base {
        pub struct LLMRequest { pub messages:Vec<crate::types::response::ChatMessage> }
        impl LLMRequest {
            pub fn new(_: &str,messages:Vec<crate::types::response::ChatMessage>)->Self{Self{messages}}
            pub fn with_json_schema(self,_:serde_json::Value)->Self{self}
            pub fn with_character_id(self,_:String)->Self{self}
            pub fn without_framework_instructions(self)->Self{self}
        }
    }
    #[derive(Default)] pub struct ModelRouter { pub outputs:Mutex<VecDeque<String>>, pub prompts:Mutex<Vec<String>> }
    impl ModelRouter {
        pub async fn generate(&self,req:base::LLMRequest)->crate::error::VivianResult<String> {
            self.prompts.lock().push(req.messages[0].content.clone());
            Ok(self.outputs.lock().pop_front().unwrap_or_else(|| String::from(r#"{"title":"交流","summary":"确认了约定。","topics":["约定"],"importance":0.7,"events":[]}"#)))
        }
    }
}
pub mod dialogue {
    #[derive(Debug,Clone,serde::Serialize,serde::Deserialize)] __HISTORY_ENTRY__
    pub struct DialogueManager { pub entries:parking_lot::RwLock<Vec<HistoryEntry>>,
        pub conversation_boundaries:crate::memory::conversation_semantics::ConversationBoundaryStore }
    impl DialogueManager {
        pub fn new(entries:Vec<HistoryEntry>)->Self{Self{entries:parking_lot::RwLock::new(entries),conversation_boundaries:crate::memory::conversation_semantics::ConversationBoundaryStore::new(std::env::temp_dir().join(format!("vivian-grouping-test-{}.json",uuid::Uuid::new_v4())))}}
        pub fn get_all_history(&self)->crate::error::VivianResult<Vec<HistoryEntry>>{Ok(self.entries.read().clone())}
        pub fn memory_conversations(&self)->crate::error::VivianResult<Vec<crate::memory::conversations::ConversationRecord>>{
            let (_,d)=self.conversation_boundaries.snapshot();Ok(crate::memory::conversations::project_conversations(&self.entries.read(),"vivian",&d).0)}
        pub fn notify_conversation_grouping_changed(&self){}
    }
}
pub mod cross_character { __STRIP__ __PREFIX__ }
pub mod memory {
    __MODULES__
    pub mod manager {
        use super::types::{MemoryItem,Granularity};
        #[derive(Default)] pub struct MemoryManager { pub items:parking_lot::RwLock<Vec<MemoryItem>> }
        impl MemoryManager {
            pub fn char_id(&self)->&str{"vivian"}
            pub fn reconcile_conversation_projection(&self,_:&[super::conversations::ConversationRecord])->crate::error::VivianResult<()>{Ok(())}
            pub fn check_index_drift_and_rebuild(&self)->Option<usize>{None}
            pub async fn get_all_memories(&self)->crate::error::VivianResult<Vec<MemoryItem>>{Ok(self.items.read().clone())}
            pub async fn upsert_session_summary(&self,id:&str,text:&str,importance:f64,meta:serde_json::Value)->crate::error::VivianResult<MemoryItem>{
                let mut all=self.items.write(); let stable=format!("summary:{id}");
                let existing=all.iter().position(|m|m.id==stable);
                let mut item=existing.map(|i|all[i].clone()).unwrap_or_else(||MemoryItem::new(text.into(),Granularity::Summary,importance));
                item.id=stable;item.content=text.into();item.metadata=meta;item.memory_type="session_summary".into();
                if let Some(i)=existing{all[i]=item.clone();}else{all.push(item.clone());} Ok(item)
            }
        }
    }
}
#[cfg(test)] mod integration {
    use super::*;
    use memory::{conversations::build_conversations, pipeline::ConsolidationPipeline, manager::MemoryManager, types::current_timestamp};
    fn record(id:&str,sid:&str,ts:f64,text:&str)->dialogue::HistoryEntry {
        dialogue::HistoryEntry{id:id.into(),role:"user".into(),content:text.into(),timestamp:ts,session_id:Some(sid.into()),metadata:serde_json::json!({})}
    }
    fn pipeline(history:Arc<dialogue::DialogueManager>, router:Arc<providers::ModelRouter>)->ConsolidationPipeline {
        ConsolidationPipeline::new(router,config::manager::ConsolidationConfig{stage1_idle_timeout_sec:1.0,stage1_short_term_threshold:3},history)
    }
    #[tokio::test] async fn event_catalogue_links_two_sessions_without_duplicate_progress_on_retry() {
        let now=current_timestamp()-100.0;
        let mut later=record("completed","second",now+1.0,"桌宠项目今天完成了，谢谢你陪我");
        later.metadata=serde_json::json!({"conversation_boundary":"new"});
        let rows=vec![record("planned","first",now,"周末我们一起完成桌宠项目吧"),later];
        let sessions=build_conversations(&rows,"vivian");
        let history=Arc::new(dialogue::DialogueManager::new(rows));
        let router=Arc::new(providers::ModelRouter::default());let p=pipeline(history,router.clone());let m=MemoryManager::default();
        router.outputs.lock().push_back(serde_json::json!({"title":"项目约定","summary":"约定周末完成项目","topics":[],"importance":0.8,
            "events":[{"existing_event_id":null,"title":"完成桌宠项目","phase":"planned","detail":"约定周末共同完成桌宠项目",
                "source_message_id":"planned","source_quote":"一起完成桌宠项目","confidence":0.95}]}).to_string());
        let first=p.summarize_session(&m,&sessions[0].id).await.unwrap();
        let event_id=first.metadata["summary_parts"][0]["events"][0]["id"].as_str().unwrap();
        router.outputs.lock().push_back(serde_json::json!({"title":"项目完成","summary":"用户确认项目完成","topics":[],"importance":0.8,
            "events":[{"existing_event_id":event_id,"title":"完成桌宠项目","phase":"completed","detail":"用户确认项目完成并感谢陪伴",
                "source_message_id":"completed","source_quote":"桌宠项目今天完成了","confidence":0.96}]}).to_string());
        let second=p.summarize_session(&m,&sessions[1].id).await.unwrap();
        assert_eq!(second.metadata["summary_parts"][0]["events"][0]["id"],event_id);
        assert!(router.prompts.lock()[1].contains(event_id));
        assert_eq!(second.metadata["event_schema_version"],1);
        p.summarize_session(&m,&sessions[1].id).await.unwrap();
        assert_eq!(router.prompts.lock().len(),2);
        assert_eq!(m.items.read().len(),2);
    }
    #[tokio::test] async fn identical_topic_in_different_sessions_keeps_two_summaries() {
        let now=current_timestamp()-100.0;
        let mut b=record("b","s2",now+1.0,"明天看电影");b.metadata=serde_json::json!({"conversation_boundary":"new"});
        let rows=vec![record("a","s1",now,"明天看电影"),b];
        let sessions=build_conversations(&rows,"vivian");
        let history=Arc::new(dialogue::DialogueManager::new(rows));
        let router=Arc::new(providers::ModelRouter::default()); let p=pipeline(history,router.clone()); let m=MemoryManager::default();
        let first=p.summarize_session(&m,&sessions[0].id).await.unwrap();
        let second=p.summarize_session(&m,&sessions[1].id).await.unwrap();
        assert_ne!(first.id,second.id);assert_eq!(m.items.read().len(),2);
        p.summarize_session(&m,&sessions[0].id).await.unwrap();
        assert_eq!(router.prompts.lock().len(),2); // unchanged originals cost no model request
        assert_eq!(first.metadata["source_message_ids"],serde_json::json!(["a"]));
        assert_eq!(second.metadata["source_message_ids"],serde_json::json!(["b"]));
    }
    #[tokio::test] async fn semantic_review_merges_delayed_reply_before_summary_and_is_cached() {
        let now=current_timestamp();
        let rows=vec![record("question","runtime-a",now-30000.0,"周几见面？"),
            record("answer","runtime-b",now-100.0,"周五"),record("ack","runtime-c",now-90.0,"那我们周五见")];
        assert_eq!(build_conversations(&rows,"vivian").len(),2);
        let history=Arc::new(dialogue::DialogueManager::new(rows));
        let router=Arc::new(providers::ModelRouter::default());
        router.outputs.lock().push_back(r#"{"decisions":[{"message_id":"answer","relation":"continue","confidence":0.96,"reason":"延迟回答上个问题"}]}"#.into());
        let p=pipeline(history.clone(),router.clone());let m=MemoryManager::default();
        assert_eq!(p.run(&m).await.unwrap().stage1_summaries,1);
        assert_eq!(history.memory_conversations().unwrap().len(),1);
        assert_eq!(m.items.read().len(),1);
        assert_eq!(router.prompts.lock().len(),2);
        let restarted=pipeline(history,router.clone());restarted.run(&m).await.unwrap();
        assert_eq!(router.prompts.lock().len(),2);
    }
    #[tokio::test] async fn failed_review_keeps_originals_and_never_summarizes_uncertain_boundaries() {
        let now=current_timestamp();
        let rows=vec![record("q","r1",now-4000.0,"周几见？"),record("a","r2",now-100.0,"周五"),record("b","r3",now-90.0,"说好了")];
        let history=Arc::new(dialogue::DialogueManager::new(rows));
        let router=Arc::new(providers::ModelRouter::default());router.outputs.lock().push_back(" ".into());
        let p=pipeline(history.clone(),router.clone());let m=MemoryManager::default();
        assert_eq!(p.run(&m).await.unwrap().stage1_summaries,0);
        assert!(history.conversation_boundaries.snapshot().1.is_empty());
        assert_eq!(history.get_all_history().unwrap().len(),3); assert!(m.items.read().is_empty());
        p.run(&m).await.unwrap();assert_eq!(router.prompts.lock().len(),1);
    }
    #[tokio::test] async fn blank_response_keeps_checkpoint_and_explicit_null_is_remembered() {
        let rows=vec![record("a","s",1.0,"你好")];let id=build_conversations(&rows,"vivian")[0].id.clone();
        let history=Arc::new(dialogue::DialogueManager::new(rows));
        let router=Arc::new(providers::ModelRouter::default()); router.outputs.lock().extend([
            "      ".into(),r#"{"title":"寒暄","summary":null,"topics":[],"importance":0.1,"events":[]}"#.into()]);
        let p=pipeline(history,router.clone());let m=MemoryManager::default();
        assert!(p.summarize_session(&m,&id).await.is_err()); assert!(m.items.read().is_empty());
        let result=p.summarize_session(&m,&id).await.unwrap();assert_eq!(result.metadata["summary_status"],"no_content");
        p.summarize_session(&m,&id).await.unwrap();assert_eq!(router.prompts.lock().len(),2);
    }
    #[tokio::test] async fn interruption_resumes_raw_chunks_and_preserves_stable_id() {
        let rows=vec![record("long","s",1.0,&"原始🙂".repeat(20_000))];
        let id=build_conversations(&rows,"vivian")[0].id.clone();
        let history=Arc::new(dialogue::DialogueManager::new(rows));
        let router=Arc::new(providers::ModelRouter::default()); router.outputs.lock().extend([
            r#"{"title":"约定","summary":"摘要专用标记","topics":[],"importance":0.7,"events":[]}"#.into(),"bad-json".into()]);
        let p=pipeline(history.clone(),router.clone());let m=MemoryManager::default();
        assert!(p.summarize_session(&m,&id).await.is_err());
        let first=m.items.read()[0].clone();assert_eq!(first.metadata["completed_parts"],1);
        assert_eq!(first.metadata["summary_status"],"partial");assert_eq!(first.metadata["source_message_ids"],serde_json::json!([]));
        let resumed=pipeline(history,router.clone());
        let result=resumed.summarize_session(&m,&id).await.unwrap();assert_eq!(first.id,result.id);
        assert_eq!(m.items.read().len(),1);
        assert!(router.prompts.lock().iter().all(|prompt|!prompt.contains("摘要专用标记")));
        let count=result.metadata["completed_parts"].as_u64().unwrap(); assert!(count>1);
        let final_result=resumed.summarize_session(&m,&id).await.unwrap();assert_eq!(final_result.metadata["summary_status"],"complete");
        assert_eq!(final_result.metadata["source_message_ids"],serde_json::json!(["long"]));
    }
}
`;
const modules = ['types', 'kinds', 'conversations', 'conversation_semantics', 'session_summary', 'companion_policy', 'pipeline'];
source = source.replace('__HISTORY_ENTRY__', entry).replace('__STRIP__', strip).replace('__PREFIX__', prefix)
  .replace('__MODULES__', modules.map((name) => `#[path="${rustPath(`src-tauri/src/memory/${name}.rs`)}"] pub mod ${name};`).join('\n'));
source += `
#[cfg(test)] mod golden_companion_policy {
    use crate::memory::{types::{MemoryItem,Granularity},kinds,companion_policy};
    #[test] fn user_facts_originals_and_private_thoughts_follow_fixture_contracts() {
        let cases:serde_json::Value=serde_json::from_str(include_str!("${rustPath('tests/fixtures/companion-policy.json')}" )).unwrap();
        let now=10_000_000.0;
        for case in cases.as_array().unwrap() {
            let mut item=MemoryItem::new(case["content"].as_str().unwrap().into(),Granularity::Summary,0.8);
            item.memory_type=case["memory_type"].as_str().unwrap().into();
            item.metadata=case["metadata"].clone();
            item.timestamp=now-case["age_hours"].as_f64().unwrap()*3600.0;
            kinds::initialize_record(&mut item);
            assert_eq!(companion_policy::is_recallable(&item,now),case["recall"].as_bool().unwrap(),"recall: {}",case["id"]);
            assert_eq!(companion_policy::is_durable_fact(&item),case["durable_fact"].as_bool().unwrap(),"fact: {}",case["id"]);
        }
    }
}
`;
await writeFile(path.join(dir, 'src/lib.rs'), source);
// CARGO_INCREMENTAL=0: rustc 1.96.0 panics in rmeta::encoder when it reuses the
// incremental cache of these generated crates ("the compiler unexpectedly
// panicked"), which made the second and later runs of this harness fail.
const result = spawnSync('cargo', ['test', ...(process.env.VIVIAN_TEST_OFFLINE === '0' ? [] : ['--offline']), '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1'], { cwd: root, stdio: 'inherit', env: { ...process.env, CARGO_INCREMENTAL: '0' } });
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
