import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

// Run the actual session types, context builder and settlement methods without
// linking Tauri. Only platform/provider adapters are substituted. Desktop type
// compatibility is checked separately with cargo check --tests.
const source = (await readFile('src-tauri/src/brain/coding_agent.rs', 'utf8')).replaceAll('\r\n', '\n');
const child = (await readFile('src-tauri/src/brain/coding_subagent.rs', 'utf8')).replaceAll('\r\n', '\n');

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
const types = ['CodingImage', 'CodingWidget', 'CodingFileRef', 'CodingMessage', 'CodingTokenUsage',
  'CodingStats', 'ExtraWorkspace', 'CodingSession', 'WorkTodo', 'FileChange', 'CodingFileChangeView']
  .map(name => item(source, `pub struct ${name} {`, true)).join('\n');
const enums = ['CodingRole', 'CodingStatus'].map(name => item(source, `pub enum ${name} {`, true)).join('\n');
const tests = ['work_task_scope_survives_compaction_and_user_interjections',
  'work_context_changes_preserve_the_actual_system_prefix',
  'task_results_distinguish_text_returns_cancellation_and_restart',
  'rendered_snapshot_is_included_in_compaction_pressure']
  .map(name => `#[test]\n${fn(name)}`).join('\n');
const tokenSource = await readFile('src-tauri/src/utils/token_estimate.rs', 'utf8');
const directory = await mkdtemp(path.join(tmpdir(), 'vivian-work-task-contract-'));
try {
  await writeFile(path.join(directory, 'lib.rs'), `#![allow(dead_code, unused_imports)]
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
pub mod utils { pub mod token_estimate { const BYTES_PER_TOKEN:usize=4; ${item(tokenSource, 'pub fn estimate_tokens(')} } }
pub mod tools {
  pub mod types {
    #[derive(Clone,Default)] pub struct ToolUseContext;
    #[derive(Clone)] pub struct ToolResult {pub success:bool,pub data:Option<serde_json::Value>,pub error:Option<String>}
  }
  pub struct ToolSystem {pub outcomes:std::collections::HashMap<String,types::ToolResult>}
  pub mod executor {
    pub async fn execute_tool_use(name:&str,_:serde_json::Value,tools:&super::ToolSystem,_:&super::types::ToolUseContext,_:Option<()>)->super::types::ToolResult {
      tools.outcomes.get(name).expect("fixture outcome").clone()
    }
  }
}
pub mod brain {
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
      let tools=std::sync::Arc::new(crate::tools::ToolSystem {outcomes:std::collections::HashMap::from([
        ("pass".into(),crate::tools::types::ToolResult {success:true,error:None,data:Some(serde_json::json!({"data":{"exit_code":0}}))}),
        ("fail".into(),crate::tools::types::ToolResult {success:true,error:None,data:Some(serde_json::json!({"data":{"exit_code":1}}))}),
      ])});
      let step=|tool:&str,parallel| workflow::WorkflowStep {tool:tool.into(),arguments:serde_json::json!({}),parallel};
      let result=workflow::run_workflow("regression",vec![step("fail",true),step("pass",true),step("fail",false)],&tools,&crate::tools::types::ToolUseContext).await;
      assert_eq!(result.total,3); assert_eq!(result.succeeded,1); assert_eq!(result.failed,2);
      assert!(!work_contract::execution_succeeded(true,Some(&serde_json::to_value(result).unwrap())));
    }
  }
  pub mod coding_memory_records {pub fn pinned_rules(raw:&str)->String {raw.into()}}
  pub mod coding_agent {
    use std::collections::{BTreeMap,HashMap}; use parking_lot::RwLock;
    use serde::{Serialize,Deserialize}; use std::sync::Arc;
    use crate::types::response::{ChatMessage,MessageToolCall};
    use super::work_contract::{self,Artifact,Evidence,TaskResult,TaskStatus,WorkTask};
    ${types}\n${enums}
    ${['default_context_window_tokens', 'default_mode', 'default_permission', 'default_reasoning_level', 'default_history_loaded', 'default_work_todo_status'].map(fn).join('\n')}
    ${item(source, 'impl WorkTodo {')}
    ${fn('normalize_session')}\n${fn('restored_session')}
    ${fn('task_receipts')}\n${fn('unfinished_todos')}\n${fn('task_artifacts')}
    ${fn('web_capability_section')}
    ${['MAX_HISTORY_MESSAGES','TOOL_RESULT_MAX_CHARS','PLAN_MODE_POLICY','WORK_TODO_STATUSES'].map(constant).join('\n')}
    fn read_project_memory_raw(_:&str)->Option<String> {None}
    fn read_project_memory_navigation(_:&str)->Option<String> {None}
    fn project_memory_path(wd:&str)->std::path::PathBuf {std::path::Path::new(wd).join(".vivian/memory.md")}
    fn intact_history_boundary(_:&[CodingMessage],split:usize)->usize {split}
    fn prune_tool_result(content:&str,max:usize)->String {content.chars().take(max).collect()}
    fn session_cache_from_loaded(sessions:BTreeMap<String,CodingSession>)->BTreeMap<String,CodingSession> {sessions}
    struct CodingAgentService {active_runs:Arc<super::coding_cancel::Runs>,sessions:RwLock<BTreeMap<String,CodingSession>>}
    impl CodingAgentService {
      fn system_prompt(_:&str,_:&str,_:&[ExtraWorkspace],_:&str)->String {"fixed work policy".into()}
      ${pubFn('render_work_plan')}
      ${fn('build_llm_messages')}
      ${fn('estimate_pending_context')}
      ${fn('open_task_work')}
      ${fn('validate_task_return')}
      ${fn('settle_unvalidated_task')}
    }
    #[cfg(test)] mod tests {use super::*; ${fn('message')} ${fn('test_session')} ${tests} }
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
parking_lot="0.12"
once_cell="1"
tracing="0.1"
tempfile="3"
tokio={version="1",features=["macros","rt-multi-thread","sync","time"]}
`);
  const result = spawnSync('cargo', ['test', '--offline', '--quiet', '--manifest-path', path.join(directory, 'Cargo.toml'),
    '--target-dir', path.resolve('tmp/work-task-contract-target')], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  console.log('Actual work context, task scope, settlement and background job contracts:');
  console.log(result.stdout.trim());
} finally {
  await rm(directory, { recursive: true, force: true });
}
