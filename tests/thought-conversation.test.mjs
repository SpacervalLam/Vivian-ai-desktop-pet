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
const refreshStart = cognitive.indexOf('fn should_refresh_thought(');
const refreshEnd = cognitive.indexOf('\n#[cfg(test)]', refreshStart);
if (refreshStart < 0 || refreshEnd < 0) throw new Error('Missing production thought refresh policy');
const opportunityStart = proactive.indexOf('    fn thought_opportunity(');
const opportunityEnd = proactive.indexOf('\n    }', opportunityStart) + '\n    }'.length;
if (opportunityStart < 0 || opportunityEnd <= opportunityStart) throw new Error('Missing production thought timing policy');

await writeFile(path.join(dir, 'src/lib.rs'), `#![allow(dead_code)]
#[path="${source('mind/conversation.rs')}"] mod conversation;
#[path="${source('proactive/triggers.rs')}"] mod triggers;
#[path="${source('proactive/generation_retry.rs')}"] mod generation_retry;
#[path="${source('world/interaction_guidance.rs')}"] mod interaction_guidance;
#[path="${source('world/environment_policy.rs')}"] mod environment_policy;
#[cfg(test)] mod mind { pub mod conversation {pub use crate::conversation::*;} }
mod error { #[derive(Debug)] pub enum VivianError {Other(String)} }
type VivianResult<T> = Result<T, error::VivianError>;
use conversation::{ThoughtSynthesisOutput, ConversationIntent, ConversationSource, default_social_urge};
${productionFunction('extract_text_from_possible_json')}
${productionFunction('parse_thought_output')}
${cognitive.slice(refreshStart, refreshEnd)}
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
  assert!(!should_refresh_thought(10.0,true,false));
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
