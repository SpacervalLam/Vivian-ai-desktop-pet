import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

// Run the actual session types, context builder and settlement methods without
// linking Tauri. Only platform/provider adapters are substituted. Desktop type
// compatibility is checked separately with cargo check --tests.
const source = (await readFile('src-tauri/src/brain/coding_agent.rs', 'utf8')).replaceAll('\r\n', '\n');
const inputService = (await readFile('src-tauri/src/brain/coding_input_service.rs', 'utf8')).replaceAll('\r\n', '\n');
const child = (await readFile('src-tauri/src/brain/coding_subagent.rs', 'utf8')).replaceAll('\r\n', '\n');
const jobTools = (await readFile('src-tauri/src/tools/builtin/work_subagent_tools.rs', 'utf8')).replaceAll('\r\n', '\n');
const jobToolImpl = item(jobTools, 'impl Tool for WorkJobTool {');

function endOf(text, start, block = true) {
  let depth = 0, quoted = false, line = false, comment = 0;
  for (let i = start; i < text.length; i++) {
    const c = text[i], next = text[i + 1];
    if (line) { if (c === '\n') line = false; continue; }
    if (comment) {
      if (c === '/' && next === '*') { comment++; i++; }
      else if (c === '*' && next === '/') { comment--; i++; }
      continue;
    }
    if (quoted) { if (c === '\\') i++; else if (c === '"') quoted = false; continue; }
    if (c === '/' && next === '/') { line = true; i++; continue; }
    if (c === '/' && next === '*') { comment = 1; i++; continue; }
    if (c === '"') { quoted = true; continue; }
    if (!block && c === ';') return i + 1;
    if (c === '{') depth++;
    if (c === '}' && --depth === 0) return i + 1;
  }
  throw new Error(`Unterminated Rust item at ${start}`);
}
function item(text, marker, derived = false) {
  const declaration = text.indexOf(marker);
  assert.ok(declaration >= 0, `Missing production item: ${marker}`);
  const start = derived ? text.lastIndexOf('#[derive', declaration) : declaration;
  return text.slice(start, endOf(text, text.indexOf('{', declaration)));
}
const fn = name => item(source, `fn ${name}(`);
const pubFn = name => item(source, `pub fn ${name}(`);
const constant = name => {
  const start = source.indexOf(`const ${name}:`);
  assert.ok(start >= 0);
  return source.slice(start, endOf(source, start, false));
};
const modulePath = name => JSON.stringify(path.resolve(`src-tauri/src/brain/${name}.rs`).replaceAll('\\', '/'));
const types = ['PendingInput', 'CodingImage', 'CodingWidget', 'CodingFileRef', 'CodingMessage', 'CodingTokenUsage',
  'CodingStats', 'ExtraWorkspace', 'CodingSession', 'WorkTodo', 'FileChange', 'CodingFileChangeView']
  .map(name => item(source, `pub struct ${name} {`, true)).join('\n');
const requestView = item(source, 'struct WorkRequestView {');
const enums = ['CodingRole', 'CodingStatus'].map(name => item(source, `pub enum ${name} {`, true)).join('\n');
const tests = ['work_task_scope_survives_compaction_and_user_interjections',
  'model_receipt_preview_is_bounded_without_discarding_host_evidence',
  'work_context_changes_preserve_the_actual_system_prefix',
  'task_results_distinguish_text_returns_cancellation_and_restart',
  'rendered_snapshot_is_included_in_compaction_pressure']
  .map(name => `#[test]\n${fn(name)}`).join('\n');
const tokenSource = await readFile('src-tauri/src/utils/token_estimate.rs', 'utf8');
const directory = await mkdtemp(path.join(tmpdir(), 'vivian-work-task-contract-'));
try {
  await writeFile(path.join(directory, 'lib.rs'), `#![allow(dead_code, unused_imports)]
#[path=${JSON.stringify(path.resolve('src-tauri/src/provider_preset_updates.rs').replaceAll('\\', '/'))}] pub mod provider_preset_updates;
#[cfg(test)] mod work_job_tool_contracts {
  use serde_json::{json,Value};
  use crate::brain::work_jobs::{global_work_job_registry,output_page,WorkJobStatus};
  struct ToolUseContext {session_id:String}
  struct ToolResult {success:bool,text:String,data:Option<Value>}
  impl ToolResult {
    fn standard_success(text:&str,data:Option<Value>)->Self {Self {success:true,text:text.into(),data}}
    fn standard_error(text:&str,_:Option<&str>,data:Option<Value>)->Self {Self {success:false,text:text.into(),data}}
  }
  struct ValidationResult {valid:bool}
  impl ValidationResult {
    fn success(_:Option<Value>)->Self {Self {valid:true}}
    fn failure(_:&str,_:u8)->Self {Self {valid:false}}
  }
  struct WorkJobTool;
  impl WorkJobTool {
    ${item(jobToolImpl, 'async fn validate_input(')}
    ${item(jobToolImpl, 'async fn call(')}
  }
  ${item(jobTools, 'fn truncate_for_display(')}
  fn context()->ToolUseContext {ToolUseContext {session_id:uuid::Uuid::new_v4().to_string()}}
  fn complete(id:&str,text:String) {
    use crate::brain::{coding_subagent::{SubagentOutcome,SubagentStop},work_contract::{unvalidated_return,TaskStatus}};
    global_work_job_registry().complete(id,SubagentOutcome {
      result:unvalidated_return(text.clone(),TaskStatus::Partial,vec!["verify".into()],vec![]),
      output:text,rounds:1,tool_calls:0,stop:SubagentStop::Completed,
    },false);
  }
  #[tokio::test] async fn another_session_cannot_read_or_cancel_a_job() {
    let owner=context();let other=context();let jobs=global_work_job_registry();
    let id=jobs.create(owner.session_id.clone(),"private task".into(),1).unwrap();
    for action in ["output","cancel"] {
      let result=WorkJobTool.call(json!({"action":action,"job_id":id}),&other).await;
      assert!(!result.success); assert!(!result.text.contains("private task"));
    }
    assert_eq!(jobs.get(&id).unwrap().status,WorkJobStatus::Running);
    jobs.cancel(&id);
  }
  #[tokio::test] async fn tool_pages_preserve_unicode_and_the_complete_structured_result() {
    let ctx=context();let jobs=global_work_job_registry();
    let id=jobs.create(ctx.session_id.clone(),"inspect".into(),1).unwrap();
    let original="中文🦀e\\u{301}\\n".repeat(1000);complete(&id,original.clone());
    for section in ["output","task_result"] {
      let mut text=String::new();let mut offset=0;
      loop {
        let result=WorkJobTool.call(json!({"action":"output","job_id":id,"section":section,"offset":offset,"limit":113}),&ctx).await;
        assert!(result.success);
        text.push_str(result.text.split("\\n\\n[结果尚未读完").next().unwrap());
        let data=result.data.unwrap();
        match data["next_offset"].as_u64() {Some(next)=>offset=next,None=>break}
      }
      if section=="output" {assert_eq!(text,original);}
      else {let value:Value=serde_json::from_str(&text).unwrap();assert_eq!(value["summary"],original);assert_eq!(value["status"],"partial");}
    }
  }
  #[tokio::test] async fn list_pages_are_compact_and_never_embed_the_results() {
    let ctx=context();let jobs=global_work_job_registry();
    for _ in 0..35 {
      let id=jobs.create(ctx.session_id.clone(),"inspect".into(),1).unwrap();complete(&id,"PRIVATE_LARGE_BODY".repeat(500));
    }
    let first=WorkJobTool.call(json!({"action":"list"}),&ctx).await.data.unwrap();
    assert_eq!(first["jobs"].as_array().unwrap().len(),32);assert_eq!(first["total"],35);
    assert!(!first.to_string().contains("PRIVATE_LARGE_BODY"));
    let second=WorkJobTool.call(json!({"action":"list","offset":first["next_offset"]}),&ctx).await.data.unwrap();
    assert_eq!(second["jobs"].as_array().unwrap().len(),3);assert!(second["next_offset"].is_null());
  }
  #[tokio::test] async fn paging_arguments_are_validated_before_use() {
    let ctx=context();
    for args in [json!({"action":"output","job_id":"x","offset":-1}),json!({"action":"list","limit":0}),
      json!({"action":"list","limit":1.5}),json!({"action":"output","job_id":"x","section":"invalid"})] {
      assert!(!WorkJobTool.validate_input(&args,&ctx).await.valid);
    }
  }
}
// This fixture isolates work settlement; the advisor's persisted context is tested in the desktop crate.
pub mod plugin_advisor { pub fn context(_:&str)->String {String::new()} }
pub mod types { pub mod response {
  use serde::{Serialize, Deserialize};
  #[derive(Debug, Clone, Serialize, Deserialize)] pub struct MessageToolCall { pub id:String, pub name:String, pub arguments:serde_json::Value }
  pub struct MessageImage { pub media_type:String, pub data:String, pub url:Option<String>, pub detail:Option<String> }
  #[derive(Default)] pub struct ChatMessage { pub role:String, pub content:String, pub reasoning:Option<String>, pub tool_calls:Option<Vec<MessageToolCall>> }
  impl ChatMessage {
    fn message(role:&str, content:impl Into<String>)->Self { Self {role:role.into(),content:content.into(),..Default::default()} }
    pub fn system(content:impl Into<String>)->Self {Self::message("system",content)}
    pub fn user(content:impl Into<String>)->Self {Self::message("user",content)}
    pub fn assistant(content:impl Into<String>)->Self {Self::message("assistant",content)}
    pub fn user_with_images(content:impl Into<String>,_:Vec<MessageImage>)->Self {Self::user(content)}
    pub fn assistant_with_tool_calls(content:impl Into<String>,calls:Vec<MessageToolCall>)->Self {let mut m=Self::assistant(content);m.tool_calls=Some(calls);m}
    pub fn tool_result(content:impl Into<String>,_:impl Into<String>)->Self {Self::message("tool",content)}
  }
}}
pub mod providers {pub mod capabilities {pub fn default_context_window(_:&str)->u64 {128000}}}
pub mod config { pub mod personalization {
  // These contracts use an unconfigured office agent; production rendering has its own tests.
  pub fn prompt_section()->String {String::new()}
}}
pub mod browser_bridge { pub mod tools {
  pub struct Bridge; impl Bridge { pub fn is_connected(&self)->bool {false} }
  pub fn global_bridge()->Option<Bridge> {None}
}}
#[path=${JSON.stringify(path.resolve('src-tauri/src/utils/fs.rs').replaceAll('\\', '/'))}] pub mod work_fs;
pub mod utils {
  pub use crate::work_fs as fs;
  pub mod path {
    pub fn get_user_data_dir()->std::path::PathBuf {
      static DIR:once_cell::sync::Lazy<tempfile::TempDir>=once_cell::sync::Lazy::new(||tempfile::tempdir().unwrap());
      DIR.path().to_path_buf()
    }
  }
  pub mod token_estimate { const BYTES_PER_TOKEN:usize=4; ${item(tokenSource, 'pub fn estimate_tokens(')} }
}
pub mod tools {
  pub mod types {
    #[derive(Clone,Default)] pub struct ToolUseContext;
    #[derive(Clone)] pub struct ToolResult {pub success:bool,pub data:Option<serde_json::Value>,pub error:Option<String>}
  }
  pub struct ToolSystem {pub outcomes:std::collections::HashMap<String,types::ToolResult>,pub started:Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>}
  impl ToolSystem { pub fn can_parallelize_read(&self,name:&str)->bool {crate::brain::work_harness::parallel_read(name)} }
  pub mod executor {
    pub async fn execute_tool_use(name:&str,_:serde_json::Value,tools:&super::ToolSystem,_:&super::types::ToolUseContext,_:Option<()>)->super::types::ToolResult {
      if let Some(started)=&tools.started {started.fetch_add(1,std::sync::atomic::Ordering::SeqCst);}
      let result=tools.outcomes.get(name).expect("fixture outcome").clone();
      if result.data.as_ref().is_some_and(|v|v["pending"]==true) {std::future::pending::<()>().await;}
      result
    }
  }
}
pub mod brain {
  #[path=${modulePath('coding_input_queue')}] pub mod coding_input_queue;
  #[path=${modulePath('coding_io')}] pub mod coding_io;
  #[path=${modulePath('work_harness')}] pub mod work_harness;
  #[path=${modulePath('coding_cancel')}] pub mod coding_cancel;
  #[path=${modulePath('work_contract')}] pub mod work_contract;
  #[path=${modulePath('work_context')}] pub mod work_context;
  pub mod coding_subagent {
    use super::work_contract::TaskResult;
    ${item(child, 'pub enum SubagentStop {', true)}
    ${item(child, 'pub struct SubagentOutcome {', true)}
  }
  #[path=${modulePath('work_jobs')}] pub mod work_jobs;
  #[path=${modulePath('workflow')}] pub mod workflow;
  #[cfg(test)] mod workflow_tests {
    use super::*;
    #[tokio::test] async fn nonzero_exit_codes_fail_both_parallel_and_sequential_steps() {
      let tools=std::sync::Arc::new(crate::tools::ToolSystem {started:None,outcomes:std::collections::HashMap::from([
        ("pass".into(),crate::tools::types::ToolResult {success:true,error:None,data:Some(serde_json::json!({"data":{"exit_code":0}}))}),
        ("fail".into(),crate::tools::types::ToolResult {success:true,error:None,data:Some(serde_json::json!({"data":{"exit_code":1}}))}),
      ])});
      let step=|tool:&str,parallel| workflow::WorkflowStep {tool:tool.into(),arguments:serde_json::json!({}),parallel};
      let result=workflow::run_workflow("regression",vec![step("fail",true),step("pass",true),step("fail",false)],&tools,&crate::tools::types::ToolUseContext).await;
      assert_eq!(result.total,3); assert_eq!(result.succeeded,1); assert_eq!(result.failed,2);
      assert!(!work_contract::execution_succeeded(true,Some(&serde_json::to_value(result).unwrap())));
    }
    #[tokio::test] async fn cancelled_workflow_stops_the_scoped_pool_without_dispatching_later_reads() {
      use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
      let started=Arc::new(AtomicUsize::new(0));
      let tools=Arc::new(crate::tools::ToolSystem {started:Some(started.clone()),outcomes:std::collections::HashMap::from([
        ("read_file".into(),crate::tools::types::ToolResult {success:true,error:None,data:Some(serde_json::json!({"pending":true}))}),
      ])});
      let steps=(0..20).map(|_|workflow::WorkflowStep {tool:"read_file".into(),arguments:serde_json::json!({}),parallel:true}).collect();
      assert!(tokio::time::timeout(std::time::Duration::from_millis(20),workflow::run_workflow("cancel",steps,&tools,&crate::tools::types::ToolUseContext)).await.is_err());
      tokio::task::yield_now().await;
      assert_eq!(started.load(Ordering::SeqCst),work_harness::MAX_PARALLEL_READS);
    }
  }
  #[path=${modulePath('coding_memory_records')}] pub mod coding_memory_records;
  #[path=${modulePath('work_memory')}] pub mod work_memory;
  #[path=${modulePath('work_instructions')}] pub mod work_instructions;
  #[path=${modulePath('coding_memory_navigation')}] pub mod coding_memory_navigation;
  #[path=${modulePath('work_memory_cache')}] pub mod work_memory_cache;
  pub mod coding_agent {
    use std::collections::{BTreeMap,HashMap}; use parking_lot::RwLock;
    use serde::{Serialize,Deserialize}; use std::sync::Arc;
    use crate::types::response::{ChatMessage,MessageToolCall};
    use super::work_contract::{self,Artifact,Evidence,TaskResult,TaskStatus,WorkTask};
    use super::coding_input_queue::InputQueue;
    ${item(inputService, 'fn validate(')}
    ${types}\n${enums}\n${requestView}
    ${item(source, "pub struct WorkMemoryDocument {", true)}
    static MEMORY_POLICY_GATE: std::sync::LazyLock<parking_lot::Mutex<()>> = std::sync::LazyLock::new(Default::default);
    ${['default_context_window_tokens', 'default_mode', 'default_permission', 'default_reasoning_level', 'default_history_loaded', 'default_work_todo_status'].map(fn).join('\n')}
    ${item(source, 'impl WorkTodo {')}
    ${fn('normalize_session')}\n${fn('restored_session')}
    ${fn('task_receipts')}\n${fn('unfinished_todos')}\n${fn('task_artifacts')}\n${fn('should_pause_input_queue')}
    ${fn('web_capability_section')}
    ${['FILE_REF_MAX_COUNT','TOOL_RESULT_MAX_CHARS','PLAN_MODE_POLICY','WORK_TODO_STATUSES'].map(constant).join('\n')}
    static MEMORY_TEST_DATA: once_cell::sync::Lazy<tempfile::TempDir> = once_cell::sync::Lazy::new(||tempfile::tempdir().unwrap());
    fn get_user_data_dir()->std::path::PathBuf {MEMORY_TEST_DATA.path().to_path_buf()}
    ${fn('global_work_memory_path')}
    ${fn('memory_working_directory')}
    ${fn('instruction_targets')}
    fn read_project_memory_raw(_:&str)->Option<String> {None}
    fn read_project_memory_snapshot(_:&str)->Option<Arc<super::work_memory_cache::Snapshot>> {None}
    fn read_project_memory_navigation(_:&str)->Option<String> {None}
    fn project_memory_path(wd:&str)->std::path::PathBuf {std::path::Path::new(wd).join(".vivian/memory.md")}
    fn intact_history_boundary(_:&[CodingMessage],split:usize)->usize {split}
    fn prune_tool_result(content:&str,max:usize)->String {content.chars().take(max).collect()}
    fn session_cache_from_loaded(sessions:BTreeMap<String,CodingSession>)->BTreeMap<String,CodingSession> {sessions}
    struct CodingAgentService {active_runs:Arc<super::coding_cancel::Runs>,sessions:RwLock<BTreeMap<String,CodingSession>>}
    impl CodingAgentService {
      fn persist(&self) {}
      ${item(inputService, 'pub fn input_queue(')}
      ${item(inputService, 'pub fn change_input_queue(').replace('super::super::coding_io::flush()', 'super::coding_io::flush()')}
      fn get_session(&self,id:&str)->Option<CodingSession> {self.sessions.read().get(id).cloned()}
      ${pubFn('set_memory_policy')}
      ${pubFn('memory_documents')}
      ${fn('memory_document_path')}
      fn system_prompt(_:&str,_:&str,_:&[ExtraWorkspace],_:&str)->String {"fixed work policy".into()}
      ${pubFn('render_work_plan')}
      ${fn('build_llm_messages')}
      ${fn('estimate_pending_context')}
      ${fn('open_task_work')}
      ${fn('validate_task_return')}
      ${fn('settle_unvalidated_task')}
    }
    #[cfg(test)] mod tests {use super::*; ${fn('message')} ${fn('test_session')} ${tests}
      #[test] fn queue_pauses_for_interruptions_and_open_work_without_changing_task_verdicts() {
        let mut session=test_session("pause-policy","",0);
        session.task=Some(WorkTask::new("goal".into(),0));
        session.task.as_mut().unwrap().settle(work_contract::unvalidated_return("plain reply".into(),TaskStatus::Partial,vec!["not verified".into()],vec![]));
        assert!(!should_pause_input_queue(&session,CodingStatus::Idle,false));
        assert_eq!(session.task.as_ref().unwrap().status,TaskStatus::Partial);
        assert!(should_pause_input_queue(&session,CodingStatus::Canceled,false));
        assert!(should_pause_input_queue(&session,CodingStatus::Idle,true));
        session.resume_available=true;assert!(should_pause_input_queue(&session,CodingStatus::Idle,false));session.resume_available=false;
        session.task.as_mut().unwrap().result.as_mut().unwrap().contract_validated=true;
        assert!(should_pause_input_queue(&session,CodingStatus::Idle,false));
        session.task.as_mut().unwrap().result.as_mut().unwrap().status=TaskStatus::Completed;
        assert!(!should_pause_input_queue(&session,CodingStatus::Idle,false));
      }
      #[test] fn host_input_queue_keeps_attachments_edits_and_restart_pause() {
        let service=CodingAgentService {active_runs:Default::default(),sessions:RwLock::new(BTreeMap::from([("a".into(),test_session("a","",0)),("b".into(),test_session("b","",0))]))};
        let id=uuid::Uuid::new_v4().to_string();
        let payload=PendingInput {content:"initial".into(),images:vec![CodingImage {media_type:"image/png".into(),data:"BASE64".into(),name:Some("image".into())}],file_refs:vec![CodingFileRef {path:"src/a.rs".into(),content:None,error:None}]};
        service.change_input_queue("a","add",&id,payload.clone()).unwrap();
        service.change_input_queue("a","add",&id,payload.clone()).unwrap();
        assert_eq!(service.input_queue("a").unwrap().items.len(),1);
        service.change_input_queue("a","edit",&id,PendingInput {content:"edited".into(),images:vec![],file_refs:vec![]}).unwrap();
        service.change_input_queue("a","steer",&id,payload.clone()).unwrap();
        let queue=service.input_queue("a").unwrap();
        assert_eq!(queue.items[0].payload.content,"edited");
        assert_eq!(queue.items[0].payload.images[0].data,"BASE64");
        assert_eq!(queue.items[0].payload.file_refs[0].path,"src/a.rs");
        assert_eq!(queue.items[0].kind,super::super::coding_input_queue::InputKind::Steering);
        assert!(service.input_queue("b").unwrap().items.is_empty());
        let mut current=service.get_session("a").unwrap(); current.status=CodingStatus::Running;
        let decoded:CodingSession=serde_json::from_str(&serde_json::to_string(&current).unwrap()).unwrap();
        let restored=restored_session(decoded);
        assert!(restored.input_queue.paused && restored.input_queue.next().is_none());
        assert!(restored.resume_available);
        assert_eq!(restored.input_queue.items[0].payload.images[0].data,"BASE64");
        service.change_input_queue("a","remove",&id,payload.clone()).unwrap();
        assert!(service.change_input_queue("a","edit",&id,payload).is_err());
      }
      #[test] fn concurrent_queue_additions_preserve_every_unique_message() {
        let service=Arc::new(CodingAgentService {active_runs:Default::default(),sessions:RwLock::new(BTreeMap::from([("a".into(),test_session("a","",0))]))});
        let threads:Vec<_>=(0..16).map(|index| {let service=service.clone();std::thread::spawn(move || {
          let id=uuid::Uuid::new_v4().to_string();let payload=PendingInput {content:format!("item {index}"),images:vec![],file_refs:vec![]};
          service.change_input_queue("a","add",&id,payload.clone()).unwrap();
          service.change_input_queue("a","add",&id,payload).unwrap();
        })}).collect();
        for thread in threads {thread.join().unwrap();}
        let queue=service.input_queue("a").unwrap(); assert_eq!(queue.items.len(),16);
        assert_eq!(queue.items.iter().map(|item|&item.id).collect::<std::collections::HashSet<_>>().len(),16);
      }
      #[test] fn memory_controls_persist_independently_and_revoke_old_jobs() {
        let mut session=test_session("memory-controls","",0);
        session.role_memory_context="stale role memory".into();
        let service=CodingAgentService {active_runs:Arc::new(super::super::coding_cancel::Runs::default()),sessions:RwLock::new(BTreeMap::from([("memory-controls".into(),session)]))};
        service.set_memory_policy("memory-controls",super::super::work_memory::MemoryPolicy {use_memories:false,generate_memories:true,revision:0}).unwrap();
        let current=service.get_session("memory-controls").unwrap();
        assert!(!current.memory_policy.use_memories && current.memory_policy.generate_memories);
        assert!(current.role_memory_context.is_empty());
        assert!(!current.memory_policy.accepts_generation(0));
        let decoded:CodingSession=serde_json::from_str(&serde_json::to_string(&current).unwrap()).unwrap();
        assert_eq!(decoded.memory_policy,current.memory_policy);
        let messages=service.build_llm_messages("memory-controls","vivian","standard");
        assert!(!messages.iter().any(|m|m.content.contains("stale role memory")));
      }
      #[test] fn memory_editor_addresses_the_same_override_as_the_loader() {
        let dir=tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.override.md"),"explicit override").unwrap();
        let session=test_session("editor",dir.path().to_str().unwrap(),0);
        let service=CodingAgentService {active_runs:Arc::new(super::super::coding_cancel::Runs::default()),sessions:RwLock::new(BTreeMap::from([("editor".into(),session)]))};
        let docs=service.memory_documents("editor").unwrap();
        let rules=docs.iter().find(|d|d.id=="rules").unwrap();
        assert!(rules.path.ends_with("AGENTS.override.md"));
        assert_eq!(rules.content.as_deref(),Some("explicit override"));
        assert!(service.memory_document_path("editor","../../escape").is_err());
      }
    }
  }
}
`);
  await writeFile(path.join(directory, 'Cargo.toml'), `[package]
name="vivian-work-task-contract-fixture"
version="0.0.0"
edition="2021"
[lib]
path="lib.rs"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
sha2="0.10"
chrono="0.4"
futures="0.3"
parking_lot="0.12"
once_cell="1"
tracing="0.1"
tempfile="3"
uuid={version="1",features=["v4"]}
tokio={version="1",features=["macros","rt-multi-thread","sync","time"]}
[target.'cfg(windows)'.dependencies]
windows={version="0.61",features=["Win32_Storage_FileSystem"]}
`);
  const result = spawnSync('cargo', ['test', '--offline', '--quiet', '--manifest-path', path.join(directory, 'Cargo.toml'),
    '--target-dir', path.resolve('tmp/work-task-contract-target')], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  console.log('Actual work context, task scope, settlement, persistent jobs and job tool contracts:');
  console.log(result.stdout.trim());
} finally {
  await rm(directory, { recursive: true, force: true });
}
