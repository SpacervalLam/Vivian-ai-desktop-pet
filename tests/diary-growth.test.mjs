// Exercise the production diary review and persistence code without the desktop
// runtime or a paid provider. Only app services are replaced at their boundaries.
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const source = file => JSON.stringify(path.resolve('src-tauri/src', file).replaceAll('\\', '/'));
const generator = (await readFile('src-tauri/src/diary/intelligent_generator.rs', 'utf8')).replaceAll('\r\n', '\n');
const item = (text, marker) => {
  const start = text.indexOf(marker);
  const end = text.indexOf('\n}', start);
  assert.ok(start >= 0 && end > start, marker);
  return text.slice(start, end + 2);
};
const record = item(generator, 'pub struct InteractionRecord');
const recordImpl = item(generator, 'impl InteractionRecord');
const stripFence = item(generator, 'pub(crate) fn strip_markdown_code_fence');
const manager = (await readFile('src-tauri/src/config/manager.rs', 'utf8')).replaceAll('\r\n', '\n');
const routeDefaults = item(manager, 'impl TaskRouteConfig {');
const routeMigration = item(manager, 'fn ensure_task_routes(');
const dir = await mkdtemp(path.join(tmpdir(), 'vivian-diary-growth-'));
try {
  await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="diary-growth-contract"
version="0.0.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
sha2="0.10"
uuid={version="1",features=["v4"]}
parking_lot="0.12"
tracing="0.1"
once_cell="1"
tokio={version="1",features=["macros","rt"]}
[lib]
path="lib.rs"
`);
  await writeFile(path.join(dir, 'lib.rs'), `#![allow(dead_code)]
mod config {pub mod manager {
 #[derive(Clone,Default)] pub struct TaskRouteConfig {pub model:String,pub api_key:String,pub temperature:Option<f64>,pub max_tokens:Option<u32>}
 ${routeDefaults}
 pub struct AppConfig {pub routing_matrix:std::collections::HashMap<String,TaskRouteConfig>}
 impl Default for AppConfig {fn default()->Self {Self{routing_matrix:crate::providers::task_catalog::TASK_ROUTES.iter().map(|s|(s.id.clone(),TaskRouteConfig::for_task(&s.id))).collect()}}}
 ${routeMigration}
 #[test] fn evolution_route_defaults_and_legacy_model_migration_preserve_explicit_choices() {
  let mut cfg=AppConfig{routing_matrix:Default::default()};
  cfg.routing_matrix.insert("diary_growth".into(),TaskRouteConfig{model:"existing-model".into(),api_key:"fixture-key".into(),..Default::default()});
  ensure_task_routes(&mut cfg);
  assert_eq!(cfg.routing_matrix["persona_evolution"].model,"existing-model");
  assert_eq!(cfg.routing_matrix["persona_evolution"].api_key,"fixture-key");
  cfg.routing_matrix.get_mut("persona_evolution").unwrap().model="explicit-new-choice".into();
  ensure_task_routes(&mut cfg);assert_eq!(cfg.routing_matrix["persona_evolution"].model,"explicit-new-choice");
  let mut fresh=AppConfig{routing_matrix:Default::default()};ensure_task_routes(&mut fresh);
  assert_eq!(fresh.routing_matrix["persona_evolution"].temperature,Some(0.35));
  assert_eq!(fresh.routing_matrix["persona_evolution"].max_tokens,Some(6144));
 }
}}
mod error {
 #[derive(Debug)] pub enum VivianError {Memory(String),Other(String)}
 impl std::fmt::Display for VivianError {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {write!(f,"{self:?}")}}
 pub type VivianResult<T> = Result<T,VivianError>;
}
mod utils {
 pub mod path {
  pub fn get_character_data_dir(id:&str)->std::path::PathBuf {std::path::PathBuf::from(std::env::var_os("VIVIAN_TEST_DATA_DIR").unwrap()).join(id)}
 }
 pub mod fs {
  pub fn load_json_or_backup<T:serde::de::DeserializeOwned>(p:&std::path::Path)->Option<T>{serde_json::from_slice(&std::fs::read(p).ok()?).ok()}
  pub fn write_atomic(p:&std::path::Path,s:&str)->std::io::Result<()> {if let Some(dir)=p.parent(){std::fs::create_dir_all(dir)?;}std::fs::write(p,s)}
 }
}
mod types {pub mod response {
 #[derive(Debug,Clone)] pub struct ChatMessage {pub role:String,pub content:String}
 impl ChatMessage {
  pub fn system(content:impl Into<String>)->Self {Self{role:"system".into(),content:content.into()}}
  pub fn user(content:impl Into<String>)->Self {Self{role:"user".into(),content:content.into()}}
 }
}}
mod pipeline {pub mod prompt_modules {pub fn normalize_lang(s:&str)->&str {if matches!(s,"en"|"ja"){s}else{"zh"}}}}
mod memory {
 #[derive(Default)] pub struct MemoryManager(pub parking_lot::RwLock<std::collections::HashMap<String,String>>);
 impl MemoryManager {pub fn get_memory_content_by_id(&self,id:&str)->Option<String>{self.0.read().get(id).cloned()}}
 pub mod types {pub fn current_timestamp()->f64 {std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64()}}
}
mod persona {
 #[path=${source('persona/example_library.rs')}] pub mod example_library;
 #[path=${source('persona/evolution.rs')}] pub mod evolution;
 #[path=${source('persona/evolution_agent.rs')}] pub mod evolution_agent;
 #[derive(Clone,serde::Serialize)] pub struct Config {pub text:String}
 pub struct PersonaEngine {pub store:evolution::PersonaEvolutionStore,pub config:Config}
 impl PersonaEngine {
  pub fn new(_: &str)->crate::error::VivianResult<Self>{Ok(Self{store:evolution::PersonaEvolutionStore::fallback(),config:Config{text:"角色有自己的好奇心；用户手动设置：说话简洁".into()}})}
  pub fn get_language(&self)->String {"zh".into()}
  pub fn get_config(&self)->Config {self.config.clone()}
  pub fn dialogue_card(&self)->String {self.config.text.clone()}
  pub fn dialogue_preferences(&self)->String {String::new()}
  pub fn private_self_context(&self)->String {"自己的兴趣：研究故事结构".into()}
  pub fn evolution_snapshot(&self)->evolution::PersonaEvolution {self.store.snapshot()}
  pub fn diary_config_reference(&self)->String {example_library::fingerprint(&self.config)}
  pub fn reconcile_evolution(&self,memory:&crate::memory::MemoryManager) {
   self.store.reconcile(|e|memory.get_memory_content_by_id(&e.memory_id).is_some_and(|s|s.contains(&e.quote)));
  }
  pub fn revise_evolution(&self,kind:&str,scope:&str,text:&str,reason:&str,evidence:evolution::GrowthEvidence,explicit:bool,revises:Option<&str>)->bool {
   self.store.propose_revision(kind,scope,text,reason,evidence,explicit,revises)
  }
  pub fn apply_diary_review(&self,config:&str,revision:f64,id:&str,proposals:&[evolution::DiaryRevision],repairs:&[example_library::ExampleEdit])->crate::error::VivianResult<evolution::DiaryReviewOutcome>{
   assert_eq!(config,self.diary_config_reference());self.store.review_diary(revision,id,proposals,repairs)
  }
 }
 pub mod prompt_render {pub fn render_dialogue_card(config:&super::Config,_:&str,_:&[super::evolution::EvolutionEntry])->String {config.text.clone()}}
}
mod providers {
 pub use crate::brain::Router as ModelRouter;
 #[path=${source('providers/task_catalog.rs')}] pub mod task_catalog;
 pub mod base {
 pub struct LLMRequest {pub task:String,pub messages:Vec<crate::types::response::ChatMessage>,pub include_framework:bool}
 impl LLMRequest {
  pub fn new(task:&str,messages:Vec<crate::types::response::ChatMessage>)->Self {Self{task:task.into(),messages,include_framework:true}}
  pub fn without_framework_instructions(mut self)->Self {self.include_framework=false;self}
  pub fn with_json_schema(self,_:serde_json::Value)->Self {self}
  pub fn with_character_id(self,_:String)->Self {self}
 }
}}
mod brain {
 pub struct Router {pub response:String,pub requests:parking_lot::Mutex<Vec<crate::providers::base::LLMRequest>>}
 impl Router {pub async fn generate(&self,request:crate::providers::base::LLMRequest)->crate::error::VivianResult<String>{self.requests.lock().push(request);Ok(self.response.clone())}}
 pub struct Brain {pub persona:crate::persona::PersonaEngine,pub memory:crate::memory::MemoryManager,pub char_id:String,pub router:Router}
}
mod diary {
 pub mod intelligent_generator {
  #[derive(Debug,Clone,Default)] ${record}
  ${recordImpl}
  ${stripFence}
  #[derive(Default)] pub struct DailyContext {pub char_id:String,pub interactions:Vec<InteractionRecord>,pub last_diary_summary:String,pub cross_diary_context:String}
 }
 #[path=${source('diary/growth.rs')}] pub mod growth;
}
#[cfg(test)] mod integration {
 use super::*;
 #[tokio::test] async fn diary_call_can_create_voice_examples_without_personality_changes_or_copied_quotes() {
  let response=serde_json::json!({"proposals":[],"example_updates":[{"id":"grown-independent","reason":"补充角色不追问的自然表达","example":{
   "id":"grown-independent","scope":"comfort","cues":["不想讲"],"context_cues":[],"situation":"虚构示例：对方很疲惫。","user":"今天不想讲了",
   "response":{"text":"那就歇会儿，想说了我再听。","intent":"reply"}}}]}).to_string();
  let brain=brain::Brain{persona:persona::PersonaEngine::new("vivian").unwrap(),memory:memory::MemoryManager::default(),char_id:"vivian".into(),router:brain::Router{response,requests:Default::default()}};
  let ctx=diary::intelligent_generator::DailyContext{char_id:"vivian".into(),last_diary_summary:"过去的相处总结".into(),..Default::default()};
  let result=diary::growth::run_diary_growth(&brain,&ctx,"今天安静地待了一会儿。").await.unwrap();
  assert_eq!(result.examples_updated,1);assert_eq!(result.promoted,0);
  assert_eq!(brain.persona.evolution_snapshot().example_overrides.len(),1);
  let requests=brain.router.requests.lock();assert_eq!(requests.len(),1);assert_eq!(requests[0].task,"persona_evolution");
  assert!(!requests[0].include_framework);
  assert!(requests[0].messages[0].content.contains("独立的人设进化智能体 PersonaEvolutionAgent"));
  let prompt=&requests[0].messages[1].content;
  for expected in ["用户手动设置","current_persona","过去的相处总结","reference","待审视的模仿语料"] {assert!(prompt.contains(expected),"{expected}");}
 }
 #[tokio::test] async fn invented_diary_evidence_cannot_publish_persona_or_voice() {
  let response=serde_json::json!({"proposals":[{"scope":"comfort","kind":"personality","understanding":"改变倾听方式","source_quote":"日记声称已经学会倾听"}],"example_updates":[]}).to_string();
  let brain=brain::Brain{persona:persona::PersonaEngine::new("vivian").unwrap(),memory:memory::MemoryManager::default(),char_id:"vivian".into(),router:brain::Router{response,requests:Default::default()}};
  let ctx=diary::intelligent_generator::DailyContext{char_id:"vivian".into(),..Default::default()};
  assert!(diary::growth::run_diary_growth(&brain,&ctx,"日记声称已经学会倾听").await.is_err());
  assert_eq!(brain.persona.evolution_snapshot().updated_at,0.0);
 }
}
`);
  const run = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '--quiet', '--', '--test-threads=1'], {
    encoding:'utf8', timeout:180_000, maxBuffer:4 * 1024 * 1024,
    env:{...process.env, VIVIAN_TEST_DATA_DIR:path.join(dir, 'data')},
  });
  assert.equal(run.status, 0, run.stdout + run.stderr + (run.error?.message ?? ''));
  console.log(run.stdout.trim());
} finally {
  assert.equal(path.dirname(path.resolve(dir)), path.resolve(tmpdir()));
  assert.ok(path.basename(dir).startsWith('vivian-diary-growth-'));
  await rm(dir, {recursive:true, force:true});
}
