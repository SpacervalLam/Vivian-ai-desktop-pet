import { mkdir, writeFile, copyFile, readFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const dir = path.join(root, 'tmp', 'thought-conversation-tests');
const source = name => path.join(root, 'src-tauri/src', name).replaceAll('\\', '/');
await mkdir(path.join(dir, 'src'), { recursive: true });
await copyFile(path.join(root, 'src-tauri/Cargo.lock'), path.join(dir, 'Cargo.lock'));
await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-thought-conversation-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
once_cell="1"
parking_lot="0.12"
tracing="0.1"
tempfile="3"
tokio={version="1",features=["rt","macros","sync","time"]}
`);

const synthesis = (await readFile(source('mind/thought_synthesis.rs'), 'utf8')).replaceAll('\r\n', '\n');
const productionFunction = name => {
  const start = synthesis.indexOf(`fn ${name}(`);
  const end = synthesis.indexOf('\n///', start);
  if (start < 0 || end < 0) throw new Error(`Missing production function ${name}`);
  return synthesis.slice(start, end);
};
const proactive = (await readFile(source('proactive/mod.rs'), 'utf8')).replaceAll('\r\n', '\n');
const cognitive = (await readFile(source('brain/cognitive_tick.rs'), 'utf8')).replaceAll('\r\n', '\n');
const mindSource = (await readFile(source('mind/mind.rs'), 'utf8')).replaceAll('\r\n', '\n');
const mindMethod = name => {
  const method = mindSource.match(new RegExp(`    (?:pub )?fn ${name}\\([\\s\\S]*?\\n    \\}`))?.[0];
  if (!method) throw new Error(`Missing actual Mind method ${name}`);
  return method;
};
const cognitiveMethod = name => {
  const method = cognitive.match(new RegExp(`    (?:pub )?fn ${name}\\([\\s\\S]*?\\n    \\}`))?.[0];
  if (!method) throw new Error(`Missing actual cognitive dispatch ${name}`);
  return method;
};
const refreshStart = cognitive.indexOf('fn should_refresh_thought(');
const refreshEnd = cognitive.indexOf('\n#[cfg(test)]', refreshStart);
if (refreshStart < 0 || refreshEnd < 0) throw new Error('Missing production thought refresh policy');
const opportunityStart = proactive.indexOf('    fn thought_opportunity(');
const opportunityEnd = proactive.indexOf('\n    }', opportunityStart) + '\n    }'.length;
if (opportunityStart < 0 || opportunityEnd <= opportunityStart) throw new Error('Missing production thought timing policy');

await writeFile(path.join(dir, 'src/lib.rs'), `#![allow(dead_code)]
#[path="${source('mind/conversation.rs')}"] mod conversation;
#[path="${source('mind/continuation.rs')}"] mod continuation;
#[path="${source('brain/resident_tasks.rs')}"] mod resident_tasks;
#[path="${source('mind/thought_threads.rs')}"] mod thought_threads;
#[path="${source('mind/peer_learning.rs')}"] mod peer_learning;
#[path="${source('memory/provenance.rs')}"] mod provenance;
#[cfg(test)] #[path="${path.join(root, 'tests/fixtures/personal-thought-tests.rs').replaceAll('\\', '/')}"] mod personal_thought_tests;
#[path="${source('proactive/triggers.rs')}"] mod triggers;
#[path="${source('proactive/generation_retry.rs')}"] mod generation_retry;
#[path="${source('world/interaction_guidance.rs')}"] mod interaction_guidance;
#[path="${source('world/topic_window.rs')}"] mod topic_window;
#[path="${source('world/environment_policy.rs')}"] mod environment_policy;
#[cfg(test)] mod mind { pub mod conversation {pub use crate::conversation::*;} }
mod utils {pub mod fs {
 pub fn write_atomic(path:&std::path::Path,text:&str)->std::io::Result<()> {
  if let Some(parent)=path.parent(){std::fs::create_dir_all(parent)?;} std::fs::write(path,text)
 }
}}
#[cfg(test)] mod peer_receipts {
 use std::sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering}};
 use parking_lot::RwLock;
 use crate::conversation::ConversationOpportunities;
 struct Mind {
  char_id:String, persistence_dir:std::path::PathBuf,
  conversation_opportunities:Arc<RwLock<ConversationOpportunities>>,
  peer_learning:Arc<RwLock<crate::peer_learning::PeerLearning>>,
  thought_context_version:AtomicU64,thought_refresh_requested:Arc<AtomicBool>,cognition_wake:Arc<tokio::sync::Notify>,
 }
 impl Mind {
  ${mindMethod('save_thought_threads')}
  ${mindMethod('save_peer_learning')}
  ${mindMethod('receive_peer_statement')}
  ${mindMethod('activate_peer_fallbacks')}
  ${mindMethod('claim_due_continuation')}
  ${mindMethod('retry_continuation')}
 }
 fn instance(path:&std::path::Path)->Mind {
  Mind {char_id:"vivian".into(),persistence_dir:path.into(),conversation_opportunities:Arc::new(RwLock::new(ConversationOpportunities::default())),
   peer_learning:Arc::new(RwLock::new(crate::peer_learning::PeerLearning::default())),
   thought_context_version:AtomicU64::new(0),thought_refresh_requested:Arc::new(AtomicBool::new(false)),cognition_wake:Arc::new(tokio::sync::Notify::new())}
 }
 #[test] fn claiming_a_due_step_is_durable_and_a_failed_save_cannot_start_thinking(){
  let dir=tempfile::tempdir().unwrap();let blocked=dir.path().join("mind");std::fs::write(&blocked,"blocked").unwrap();
  let mind=instance(&blocked);
  let update:crate::thought_threads::ThreadUpdate=serde_json::from_value(serde_json::json!({"key":"mechanism","topic":"理解机制",
   "personal_relevance":"喜欢理解机制","stance":"还未想清楚其中的区别","open_question":"","next_step":"reflect","status":"active","origin":"persona","evidence":"研究机制"})).unwrap();
  let mut opportunities=mind.conversation_opportunities.write();
  opportunities.threads.apply(&[update],&crate::thought_threads::ThoughtGrounding{persona:"喜欢研究机制",user_quotes:&[],world:&[],scene:&[],reflections:&[]},100);
  opportunities.threads.plan_continuation(Some(&crate::continuation::ContinuationRequest{concern_key:"mechanism".into(),after_seconds:60,purpose:"比较两种机制".into()}),100);
  let before=opportunities.threads.continuation.clone();drop(opportunities);
  assert!(!mind.claim_due_continuation(160));
  assert_eq!(mind.conversation_opportunities.read().threads.continuation,before);
  assert!(!mind.thought_refresh_requested.load(Ordering::Relaxed));
  std::fs::remove_file(blocked).unwrap();assert!(mind.claim_due_continuation(161));assert!(!mind.claim_due_continuation(162));
  let restored:crate::thought_threads::ThoughtThreads=serde_json::from_str(&std::fs::read_to_string(mind.persistence_dir.join("thought_threads.json")).unwrap()).unwrap();
  assert_eq!(restored.continuation.status()["next_thought"]["in_flight"],true);
  assert_eq!(restored.continuation.status()["next_thought"]["remaining_self_steps"],1);
  mind.receive_peer_statement("nana","new-material","这两种机制的区别也许在反馈路径",None,163);
  assert!(mind.conversation_opportunities.read().threads.continuation.status()["next_thought"].is_null());
  assert!(!mind.claim_due_continuation(300));
 }
 #[test] fn real_received_utterances_persist_once_invalidate_old_decisions_and_wake_private_thought() {
  let dir=tempfile::tempdir().unwrap();let mind=instance(dir.path());
  mind.receive_peer_statement("nana","actual","先观察再选择，也许更容易比较",Some("thought:choice:1".into()),100);
  assert_eq!(mind.thought_context_version.load(Ordering::SeqCst),1);
  assert!(mind.thought_refresh_requested.load(Ordering::Relaxed));
  assert_eq!(mind.conversation_opportunities.read().events()[0].id,"peer_answer:thought:choice:1:actual");
  let saved:crate::peer_learning::PeerLearning=serde_json::from_str(&std::fs::read_to_string(dir.path().join("peer_learning.json")).unwrap()).unwrap();
  assert_eq!(saved.context()["received_peer_statements"][0]["id"],"actual");
  mind.receive_peer_statement("nana","actual","先观察再选择，也许更容易比较",None,101);
  assert_eq!(mind.thought_context_version.load(Ordering::SeqCst),1);
 }
 #[test] fn fallback_becomes_one_private_decision_opportunity_and_survives_restart() {
  use crate::thought_threads::{ThoughtGrounding,ThreadUpdate,ThoughtThreads};
  let dir=tempfile::tempdir().unwrap();let mind=instance(dir.path());
  let update:ThreadUpdate=serde_json::from_value(serde_json::json!({"key":"choice","topic":"选择的方法","personal_relevance":"我在琢磨选择方法",
   "stance":"也许可以先观察","open_question":"观察怎样帮助选择","next_step":"ask_user","status":"active","origin":"persona","evidence":"研究机制",
   "question_scope":"general","peer_fallback":true})).unwrap();
  let mut opportunities=mind.conversation_opportunities.write();
  opportunities.threads.apply(&[update],&ThoughtGrounding{persona:"喜欢研究机制",user_quotes:&[],world:&[],scene:&[],reflections:&[]},100);
  let id=opportunities.threads.snapshot(100,1)[0].event_id();
  opportunities.threads.delivered(&id,crate::conversation::ConversationIntent::AskUser,104);drop(opportunities);
  mind.activate_peer_fallbacks(283);assert_eq!(mind.thought_context_version.load(Ordering::SeqCst),0);
  mind.activate_peer_fallbacks(284);mind.activate_peer_fallbacks(285);
  assert_eq!(mind.thought_context_version.load(Ordering::SeqCst),1);
  assert!(mind.conversation_opportunities.read().fresh(285.0).is_none());
  let restored:ThoughtThreads=serde_json::from_str(&std::fs::read_to_string(dir.path().join("thought_threads.json")).unwrap()).unwrap();
  assert!(restored.offers(285)[0].fact.contains("ask_peer"));
 }
}
mod error { #[derive(Debug)] pub enum VivianError {Other(String)} }
type VivianResult<T> = Result<T, error::VivianError>;
use conversation::{ThoughtSynthesisOutput, ConversationIntent, ConversationSource, default_social_urge};
${productionFunction('extract_text_from_possible_json')}
${productionFunction('parse_thought_output')}
${cognitive.slice(refreshStart, refreshEnd)}
mod companion_quiet {pub fn active()->bool{false}}
#[cfg(test)] mod cognitive_dispatch {
 use parking_lot::Mutex;
 use std::sync::atomic::{AtomicBool,AtomicUsize,Ordering};
 use crate::VivianResult;
 #[derive(Clone,Default)] struct PhaseDecision {executed:bool,produced:bool}
 #[derive(Clone,Default)] struct CognitiveTickResult {
  world_ingest:PhaseDecision,self_update:PhaseDecision,observe:PhaseDecision,think:PhaseDecision,act:PhaseDecision,speak:PhaseDecision,
  produced_user_message:bool,
 }
 struct Brain {private:AtomicUsize,speech:AtomicUsize}
 struct TickContext {now:f64}
 #[derive(Default)] struct CognitiveTickRunner {
  resident_owned:AtomicBool,private_pulse:Mutex<()>,last_private_result:Mutex<CognitiveTickResult>,
 }
 impl CognitiveTickRunner {
  ${cognitiveMethod('run')}
  ${cognitiveMethod('resident_active')}
  ${cognitiveMethod('run_background')}
  ${cognitiveMethod('run_private')}
  fn phase_world_ingest(&self,brain:&Brain,_:&TickContext)->PhaseDecision {brain.private.fetch_add(1,Ordering::SeqCst);PhaseDecision::default()}
  fn phase_self_update(&self,_:&Brain,_:&TickContext)->PhaseDecision {PhaseDecision::default()}
  fn phase_observe(&self,_:&Brain,_:&TickContext,_:f64)->PhaseDecision {PhaseDecision::default()}
  fn phase_think(&self,_:&Brain,_:&TickContext,_:f64)->PhaseDecision {PhaseDecision::default()}
  fn phase_act(&self,_:&Brain,_:&TickContext,_:f64)->PhaseDecision {PhaseDecision::default()}
  fn phase_speak(&self,brain:&Brain,_:&TickContext,_:bool)->VivianResult<PhaseDecision> {brain.speech.fetch_add(1,Ordering::SeqCst);Ok(PhaseDecision::default())}
 }
 #[test] fn resident_cognition_advances_without_ui_and_presentation_does_not_repeat_it(){
  let runner=CognitiveTickRunner::default();let brain=Brain{private:AtomicUsize::new(0),speech:AtomicUsize::new(0)};
  let ctx=TickContext{now:1000.0};
  runner.run_background(&brain,&ctx);runner.run_background(&brain,&ctx);
  assert_eq!(brain.private.load(Ordering::SeqCst),2);assert_eq!(brain.speech.load(Ordering::SeqCst),0);
  runner.run(&brain,&ctx).unwrap();runner.run(&brain,&ctx).unwrap();
  assert_eq!(brain.private.load(Ordering::SeqCst),2);assert_eq!(brain.speech.load(Ordering::SeqCst),2);
 }
 #[test] fn independent_characters_do_not_share_private_throttle_or_ownership(){
  let vivian=CognitiveTickRunner::default();let nana=CognitiveTickRunner::default();
  let brain=Brain{private:AtomicUsize::new(0),speech:AtomicUsize::new(0)};let ctx=TickContext{now:1000.0};
  let _blocked=vivian.private_pulse.lock();
  nana.run_background(&brain,&ctx);
  assert_eq!(brain.private.load(Ordering::SeqCst),1);assert!(!vivian.resident_active());assert!(nana.resident_active());
 }
}
#[cfg(test)] mod timing {
 use super::*;
 use parking_lot::RwLock;
 struct Mind(RwLock<conversation::ConversationOpportunities>);
 impl Mind {fn fresh_conversation_opportunity(&self,now:f64)->Option<conversation::ConversationOpportunity>{self.0.read().fresh(now)}}
 struct ProactiveState {last_interaction_time:f64}
 #[derive(Clone)] struct TickContext {now:f64,idle_seconds:f64,user_present:bool,user_contact_blocked:bool,is_user_chatting:bool,lay_low:bool}
 impl TickContext {fn can_address_user(&self)->bool{self.user_present && !self.user_contact_blocked}}
 struct Orchestrator {mind:RwLock<Option<Mind>>,since_own_speech:Option<f64>}
 impl Orchestrator {
  fn seconds_since_own_utterance(&self,_:f64)->Option<f64>{self.since_own_speech}
  ${proactive.slice(opportunityStart, opportunityEnd)}
 }
 fn fixture()->(Orchestrator,TickContext,ProactiveState){
  let mut opportunities=conversation::ConversationOpportunities::default();
  opportunities.replace(Some(conversation::ConversationOpportunity{intent:ConversationIntent::Greet,source:ConversationSource::None,
    topic:"轻声问候".into(),evidence:String::new(),reason:"确认这次相遇".into(),meaning_key:"encounter_acknowledgment".into(),event_id:String::new(),generated_at:1000.0}));
  (Orchestrator{mind:RwLock::new(Some(Mind(RwLock::new(opportunities)))),since_own_speech:Some(600.0)},
   TickContext{now:1010.0,idle_seconds:400.0,user_present:true,user_contact_blocked:false,is_user_chatting:false,lay_low:false},
   ProactiveState{last_interaction_time:600.0})
 }
 #[test] fn grounded_intent_can_speak_beyond_the_old_four_minute_idle_window(){
  let (orchestrator,ctx,state)=fixture();assert!(orchestrator.thought_opportunity(&ctx,&state).is_some());
 }
 #[test] fn new_input_active_chat_and_contact_closure_veto_but_recent_unrelated_speech_does_not(){
  let (mut orchestrator,ctx,mut state)=fixture();
  state.last_interaction_time=1005.0;assert!(orchestrator.thought_opportunity(&ctx,&state).is_none());
  state.last_interaction_time=600.0;
  let mut changed=ctx.clone();changed.is_user_chatting=true;assert!(orchestrator.thought_opportunity(&changed,&state).is_none());
  changed=ctx.clone();changed.user_contact_blocked=true;assert!(orchestrator.thought_opportunity(&changed,&state).is_none());
  changed=ctx.clone();changed.lay_low=true;assert!(orchestrator.thought_opportunity(&changed,&state).is_none());
  orchestrator.since_own_speech=Some(1.0);assert!(orchestrator.thought_opportunity(&ctx,&state).is_some());
  changed=ctx.clone();changed.idle_seconds=0.0;assert!(orchestrator.thought_opportunity(&changed,&state).is_some());
  changed.now=1800.0;assert!(orchestrator.thought_opportunity(&changed,&state).is_some());
 }
}
#[cfg(test)] mod integration {
 use super::*;
 use triggers::ProactiveTrigger;
 #[test] fn meaningful_events_refresh_without_waiting_ninety_seconds_and_an_unchanged_scene_does_not_poll_each_tick() {
  assert!(should_refresh_thought(0.0,true,true));
  assert!(!should_refresh_thought(10.0,false,true));
  assert!(should_refresh_thought(10.0,true,false), "a real peer event can wake thought while the user is away");
  assert!(should_refresh_thought(900.0,false,true));
 }
 #[test] fn malformed_unknown_and_legacy_model_outputs_cannot_trigger_speech() {
  for raw in ["想安静待着", r#"{"thought":"想聊聊","social_urge":1.0}"#,
    r#"{"thought":"想聊聊","conversation_intent":"invalid","social_urge":1.0}"#] {
   let output = parse_thought_output(raw,60).unwrap();
   assert_eq!(output.conversation_intent,ConversationIntent::Silent);
   assert!(output.validated_opportunity(&[],&[],&[],true,1000.0).is_none());
  }
 }
 #[test] fn explicit_greeting_survives_parsing_without_a_user_quote() {
  let raw=r#"{"thought":"陪你待着","social_urge":0.5,"conversation_intent":"greet","conversation_source":"none","conversation_topic":"轻声问候","conversation_evidence":"","conversation_reason":"确认这次相遇","conversation_key":"acknowledge_encounter","conversation_event_id":""}"#;
  assert!(parse_thought_output(raw,60).unwrap().validated_opportunity(&[],&[],&[],true,1000.0).is_some());
 }
 #[test] fn everyday_conversation_uses_thoughts_while_arrivals_actions_and_callbacks_keep_their_paths() {
  for trigger in [ProactiveTrigger::HourlyGreeting, ProactiveTrigger::IdleGreeting,
    ProactiveTrigger::Icebreaker, ProactiveTrigger::Spontaneous, ProactiveTrigger::MoodDriven,
    ProactiveTrigger::TopicExtension, ProactiveTrigger::MemoryRecall, ProactiveTrigger::WindowTrigger,
    ProactiveTrigger::WelcomeBack, ProactiveTrigger::HealthReminder] {
   assert!(trigger.is_optional_conversation());
  }
  for trigger in [ProactiveTrigger::TeasingResponse,
    ProactiveTrigger::CrossCharacterReply, ProactiveTrigger::WorkNotice,
    ProactiveTrigger::LateNight, ProactiveTrigger::MusicChanged] {
   assert!(!trigger.is_optional_conversation());
  }
 }
}
`);
const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1'], {
  cwd: root, stdio: 'inherit', timeout: 180000,
  env: { ...process.env, CARGO_INCREMENTAL: '0', CARGO_PROFILE_DEV_DEBUG: '0' },
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
