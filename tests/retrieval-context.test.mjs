import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

// Compile the production rewriter and recall-query builder. Only the transport,
// storage count and application carrier types are replaced; no network or user data.
const read = async file => (await readFile(file, 'utf8')).replaceAll('\r\n', '\n');
const context = await read('src-tauri/src/pipeline/message_context.rs');
const helpers = context.slice(context.indexOf('pub fn is_short_followup'), context.indexOf('pub fn split_prompt_context'));
assert.ok(helpers.includes('pub fn recent_retrieval_context'));
const memory = await read('src-tauri/src/pipeline/steps/memory.rs');
const queryStart = memory.indexOf('    fn build_retrieval_query(');
const queryEnd = memory.indexOf('\n    }', queryStart) + 6;
assert.ok(queryStart >= 0 && queryEnd > queryStart);
const messages = await read('src-tauri/src/messages.rs');
const routing = messages.slice(messages.lastIndexOf('#[derive', messages.indexOf('pub enum MessageSource')), messages.indexOf('/// 消息元数据'));
const prompts = await read('src-tauri/src/pipeline/prompt_modules.rs');
const normalizeStart = prompts.indexOf('pub fn normalize_lang(');
const normalizeEnd = prompts.indexOf('\n}', normalizeStart) + 2;
assert.ok(normalizeStart >= 0 && normalizeEnd > normalizeStart);
const rewriterPath = path.resolve('src-tauri/src/pipeline/steps/query_rewrite.rs').replaceAll('\\', '/');
const testsPath = path.resolve('tests/fixtures/retrieval-context-tests.rs').replaceAll('\\', '/');
const dir = await mkdtemp(path.join(tmpdir(), 'vivian-retrieval-context-'));
try {
  await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-retrieval-context-contracts"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
async-trait="0.1"
parking_lot="0.12"
lru="0.12"
tracing="0.1"
tokio={version="1",features=["rt-multi-thread","macros","time"]}
vivian-storage-contracts={path="${path.resolve('src-tauri/storage-contracts').replaceAll('\\','/')}"}
[lib]
path="lib.rs"
`);
  await writeFile(path.join(dir, 'lib.rs'), `#![allow(dead_code)]
mod error {pub type VivianResult<T>=Result<T,String>;}
mod i18n {pub fn get_language()->String{"zh".into()}}
mod messages {
 use serde::{Serialize,Deserialize};
 ${routing}
 #[derive(Clone,Default,Serialize,Deserialize)] pub struct MessageMeta {
  #[serde(default)] pub source:MessageSource,
  pub communication:Option<CommunicationContext>,
 }
}
mod types {pub mod response {
 #[derive(Clone,Default,serde::Serialize,serde::Deserialize)] pub struct ChatMessage {
  pub role:String,pub content:String,#[serde(default)]pub meta:Option<crate::messages::MessageMeta>,
 }
 impl ChatMessage {
  pub fn system(text:impl Into<String>)->Self{Self{role:"system".into(),content:text.into(),meta:None}}
  pub fn user(text:impl Into<String>)->Self{Self{role:"user".into(),content:text.into(),meta:None}}
  pub fn assistant(text:impl Into<String>)->Self{Self{role:"assistant".into(),content:text.into(),meta:None}}
 }
}}
mod memory {pub use vivian_storage_contracts::recall_policy;pub struct MemoryManager;impl MemoryManager {pub fn entry_count(&self)->usize{20}}}
mod providers {
 pub mod base {
  pub struct LLMRequest {pub task_type:String,pub messages:Vec<crate::types::response::ChatMessage>,pub include_framework_instructions:Option<bool>}
  impl LLMRequest {
   pub fn new(task:&str,messages:Vec<crate::types::response::ChatMessage>)->Self{Self{task_type:task.into(),messages,include_framework_instructions:None}}
   pub fn without_framework_instructions(mut self)->Self{self.include_framework_instructions=Some(false);self}
  }
 }
 #[derive(Default)] pub struct ModelRouter {pub requests:parking_lot::Mutex<Vec<base::LLMRequest>>,pub replies:parking_lot::Mutex<std::collections::VecDeque<Result<String,String>>>}
 impl ModelRouter {
  pub async fn generate(&self,request:base::LLMRequest)->Result<String,String>{
   self.requests.lock().push(request);self.replies.lock().pop_front().unwrap_or_else(||Err("unexpected model call".into()))
  }
 }
}
mod pipeline {
 pub mod base {
  pub struct RunnableConfig;
  #[async_trait::async_trait] pub trait Runnable:Send+Sync {
   async fn ainvoke(&self,input:serde_json::Value,config:Option<RunnableConfig>)->crate::error::VivianResult<serde_json::Value>;
  }
 }
 pub mod state {
  #[derive(Default,serde::Serialize,serde::Deserialize)] #[serde(default)] pub struct PipelineState {
   pub user_input:String,pub resolved_user_input:String,pub intent:String,pub is_command:bool,
   pub messages:Vec<crate::types::response::ChatMessage>,pub metadata:serde_json::Value,
  }
  impl PipelineState {pub fn from_json(v:serde_json::Value)->Self{serde_json::from_value(v).unwrap()}pub fn to_json(&self)->serde_json::Value{serde_json::to_value(self).unwrap()}}
 }
 pub mod prompt_modules {${prompts.slice(normalizeStart, normalizeEnd)}}
 pub mod message_context {use crate::types::response::ChatMessage;${helpers}}
}
#[path="${rewriterPath}"]mod query_rewrite;
use pipeline::state::PipelineState;
struct RecallProbe;
impl RecallProbe {${memory.slice(queryStart, queryEnd)}}
#[cfg(test)]#[path="${testsPath}"]mod contracts;
`);
  const run = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '--quiet'], {
    encoding: 'utf8', timeout: 180000,
    env: { ...process.env, CARGO_TARGET_DIR: path.join(tmpdir(), 'vivian-retrieval-context-target') },
  });
  assert.ifError(run.error);
  assert.equal(run.status, 0, run.stdout + run.stderr);
  console.log(run.stdout.trim());
} finally {
  assert.equal(path.dirname(path.resolve(dir)), path.resolve(tmpdir()));
  await rm(dir, { recursive: true, force: true });
}
