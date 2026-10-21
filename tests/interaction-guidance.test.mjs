// Run the real optional reply helper against local router doubles; no account/model/app data.
import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
const root = process.cwd(), dir = path.join(root, 'tmp', 'interaction-guidance-tests');
await mkdir(path.join(dir, 'src'), { recursive:true });
const source = (await readFile('src-tauri/src/world/environment_agent.rs','utf8')).replaceAll('\r\n','\n');
let helper = source.match(/    pub async fn reply_guidance\([\s\S]*?\n    \}/)?.[0];
assert.ok(helper);
helper = helper.replaceAll('../../prompts/framework/', path.join(root,'src-tauri/prompts/framework').replaceAll('\\','/')+'/');
await writeFile(path.join(dir,'Cargo.toml'), `[package]
name="vivian-interaction-guidance-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
chrono="0.4"
tokio={version="1",features=["macros","rt","sync","time","test-util"]}
`);
await writeFile(path.join(dir,'src/lib.rs'), `#![allow(dead_code)]
use std::{sync::Arc,time::Duration};
#[path="${path.join(root,'src-tauri/src/world/interaction_guidance.rs').replaceAll('\\','/')}"] mod interaction_guidance;
use interaction_guidance::{Evidence,Suggestion};
mod types {pub mod response {
pub struct ChatMessage { pub role:String,pub content:String }
impl ChatMessage {pub fn system(s:impl Into<String>)->Self {Self{role:"system".into(),content:s.into()}} pub fn user(s:impl Into<String>)->Self{Self{role:"user".into(),content:s.into()}}}
}}
mod providers {
pub mod base { pub struct LLMRequest{pub task:String,pub messages:Vec<crate::types::response::ChatMessage>,pub character:String}
impl LLMRequest {pub fn new(task:&str,messages:Vec<crate::types::response::ChatMessage>)->Self{Self{task:task.into(),messages,character:String::new()}}
pub fn with_character_id(mut self,id:&str)->Self{self.character=id.into();self}
pub fn without_framework_instructions(self)->Self{self} pub fn with_temperature(self,_:f64)->Self{self} pub fn with_max_tokens(self,_:u32)->Self{self} pub fn with_reasoning(self,_:bool)->Self{self}}
}
pub struct ModelRouter{pub response:Option<String>,pub calls:std::sync::atomic::AtomicUsize,pub requests:std::sync::Mutex<Vec<base::LLMRequest>>}
impl ModelRouter {pub async fn generate(&self,request:base::LLMRequest)->Result<String,()> {
self.calls.fetch_add(1,std::sync::atomic::Ordering::Relaxed);self.requests.lock().unwrap().push(request);
match &self.response {Some(response)=>Ok(response.clone()),None=>std::future::pending().await}
}}
}
use providers::base::LLMRequest; use types::response::ChatMessage;
#[derive(Default)] struct EnvironmentAgent{planning:Arc<tokio::sync::Mutex<()>>}
impl EnvironmentAgent {${helper}}
#[cfg(test)] mod tests {use super::*;use serde_json::json;
fn router(response:Option<String>)->providers::ModelRouter {providers::ModelRouter{response,calls:0.into(),requests:std::sync::Mutex::new(vec![])}}
fn evidence()->Vec<Evidence>{vec![Evidence{id:"capture:latest".into(),at:chrono::Utc::now().timestamp() as f64,
character_id:"vivian".into(),kind:"knowledge_acquired".into(),text:"刚保存了关于漫画分镜的资料摘录，尚未分享".into(),shareable:true}]}
fn answer(id:&str)->String{json!({"guidance":{"listener_id":"user","source_ids":[id],"situation":"用户刚问起漫画房间的想法",
"direction":"可以接住这个问题，顺便挑刚收集到的一个分镜细节聊聊，留出让用户表达喜好的空隙"}}).to_string()}
#[tokio::test] async fn grounded_direction_reaches_the_role_as_optional_context_without_authored_speech(){
let agent=EnvironmentAgent::default();let router=router(Some(answer("capture:latest")));
let hint=agent.reply_guidance(&router,"vivian","user",json!({"listener_id":"user","current_utterance":"你想要什么样的房间？"}),evidence()).await.unwrap();
assert!(hint.contains("[SCENE CONTINUITY]"));assert!(hint.contains("分镜"));assert!(!hint.contains("环境智能体"));
let requests=router.requests.lock().unwrap();assert_eq!(requests[0].task,"environment_judge");assert_eq!(requests[0].character,"vivian");
assert_eq!(requests[0].messages[1].role,"user");assert!(requests[0].messages[1].content.contains("你想要什么样"));
}
#[tokio::test] async fn fabricated_sources_and_abstention_do_not_block_or_replace_a_reply(){
for response in [answer("imaginary"),"{\\"guidance\\":null}".into(),"bad response".into()] {
let router=router(Some(response));assert!(EnvironmentAgent::default().reply_guidance(&router,"vivian","user",json!({}),evidence()).await.is_none());}}
#[tokio::test(start_paused=true)] async fn stalled_helper_times_out_and_busy_planning_never_waits(){
let agent=EnvironmentAgent::default();let stalled=router(None);let before=tokio::time::Instant::now();
assert!(agent.reply_guidance(&stalled,"vivian","user",json!({}),evidence()).await.is_none());
assert!(before.elapsed()<=Duration::from_millis(2501));
let permit=agent.planning.lock().await;let waiting=router(Some(answer("capture:latest")));
assert!(agent.reply_guidance(&waiting,"vivian","user",json!({}),evidence()).await.is_none());
assert_eq!(waiting.calls.load(std::sync::atomic::Ordering::Relaxed),0);drop(permit);
}
}
`);
const result = spawnSync('cargo',['test','--offline','--manifest-path',path.join(dir,'Cargo.toml'),'-j','1'], {
  stdio:'inherit',env:{...process.env,CARGO_INCREMENTAL:'0'},
});
if (result.error) throw result.error;
process.exitCode=result.status ?? 1;
