import assert from 'node:assert/strict';
import {mkdir,readFile,writeFile,copyFile} from 'node:fs/promises';
import {spawnSync} from 'node:child_process';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
const root=fileURLToPath(new URL('../',import.meta.url));
const source=name=>path.join(root,'src-tauri/src',name).replaceAll('\\','/');
const dir=path.join(root,'tmp','environment-perception-tests');
await mkdir(path.join(dir,'src'),{recursive:true});
await copyFile(path.join(root,'src-tauri/Cargo.lock'),path.join(dir,'Cargo.lock'));
const proactive=(await readFile(source('proactive/mod.rs'),'utf8')).replaceAll('\r\n','\n');
const feedback=await readFile(source('companion_runtime.rs'),'utf8');
const commands=(await readFile(source('commands/proactive.rs'),'utf8')).replaceAll('\r\n','\n');
const desktop=(await readFile(source('commands/desktop_assistant.rs'),'utf8')).replaceAll('\r\n','\n');
const native=(await readFile(source('desktop_native/windows.rs'),'utf8')).replaceAll('\r\n','\n');
const delivery=commands.match(/fn deliverable_proactive_action\([\s\S]*?\n\}/)?.[0];
const ownScreenshot=proactive.match(/    fn spawn_screen_peek_task\([\s\S]*?\n    \}/)?.[0];
const addressedScreenshot=desktop.match(/pub\(crate\) async fn analyze_snapshot\([\s\S]*?\n\}/)?.[0];
const passive=native.match(/    fn perception\([\s\S]*?\n    \}/)?.[0];
assert.ok(delivery&&ownScreenshot&&addressedScreenshot&&passive,'actual production routes must be tested');
assert.doesNotMatch(proactive,/fn maybe_system_pressure_reminder\(/);
assert.doesNotMatch(feedback,/messages\.push\(\("pressure"/);
assert.doesNotMatch(ownScreenshot,/perception_runtime|environment\.perception|visual_report/,'pet-owned screenshots must return to the pet');
assert.doesNotMatch(addressedScreenshot,/perception_runtime|environment\.perception|visual_report/,'addressed screenshot results must not be diverted');
assert.doesNotMatch(passive,/window_png|\.perform\(|SetForegroundWindow|SendInput/,'passive collection may not capture or operate windows');
await writeFile(path.join(dir,'Cargo.toml'),`[package]
name="vivian-environment-perception-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
parking_lot="0.12"
regex="1"
`);
await writeFile(path.join(dir,'src/lib.rs'),`#![allow(dead_code)]
#[path="${source('world/perception.rs')}"] mod perception;
mod proactive {
 pub struct ProactiveAction{pub trigger:String,pub work_notice_id:Option<String>,pub content:String,pub content_type:ContentType,pub value_score:Option<f64>}
 pub enum ContentType{Text,Share} pub const SHARE_VALUE_THRESHOLD:f64=0.6;
}
mod brain {pub mod work_notices {pub struct Store; pub fn global()->Store{Store} impl Store{pub fn has_alert(&self,_:&str)->bool{true}}}}
${delivery}
#[cfg(test)] mod integration {
 use super::*;use perception::*;use serde_json::json;use std::sync::atomic::Ordering;
 #[test]fn legacy_settings_have_bounded_defaults_and_sensitive_sources_remain_independent(){
  let config:PerceptionConfig=serde_json::from_str("{}").unwrap();assert!(config.enabled&&config.foreground_text&&config.browser_context&&config.system_resources);assert!(!config.clipboard_text);
  assert!(config.excludes("KeePass.EXE"));assert!(config.excludes("BITWARDEN.exe"));assert!(!config.excludes("editor.exe"));
  let bounded=PerceptionConfig{sample_seconds:0,text_limit:usize::MAX,..config}.bounded();assert_eq!(bounded.sample_seconds,10);assert_eq!(bounded.text_limit,6000);
  assert!(browser_process("CHROME.EXE"));assert!(!browser_process("notepad.exe"));
 }
 #[test]fn disabled_text_and_password_focus_cannot_expose_document_contents(){
  let raw=json!({"accessibility":{"document_text":"private document","selected_text":["private selection"],"elements":[{"password":true,"focused":true,"offscreen":false,"value":"hidden secret"}]}});
  let blocked=accessible_context(&raw,&PerceptionConfig::default());assert_eq!(blocked["status"],"blocked");assert!(!blocked.to_string().contains("private document"));
  let disabled=accessible_context(&json!({"accessibility":{"document_text":"private document","selected_text":["selection"],"elements":[]}}),&PerceptionConfig{foreground_text:false,browser_context:false,..Default::default()});
  assert!(disabled["document_excerpt"].is_null());assert_eq!(disabled["selected_text"],json!([]));
 }
 #[test]fn stale_foreign_and_resource_sources_cannot_ground_character_questions(){
  let store=PerceptionStore::default();let epoch=store.configure(PerceptionConfig::default());
  let mut reading=Reading::new("vision:vivian:game_window_snapshot","model_interpretation",100,30,json!({"description":"an actual supplied image"}));reading.owner=Some("vivian".into());store.report(epoch,reading,None,false);
  store.report(epoch,Reading::new("accessibility","observed",100,30,json!({"content":{"status":"observed","document_excerpt":"the actual current document","selected_text":["a real selection"]}})),None,false);
  store.report(epoch,Reading::new("resources:processes","observed",100,30,json!({"top_memory_processes":["secret resource data"]})),None,false);
  let own=store.context(110,Some("vivian"),false);let evidence=grounded_texts(&own,"vivian",110);assert!(evidence.contains(&"the actual current document"));assert!(evidence.contains(&"a real selection"));assert!(evidence.contains(&"an actual supplied image"));assert!(!own.to_string().contains("secret resource data"));
  let other=store.context(110,Some("nana"),false);assert!(!grounded_texts(&other,"nana",110).contains(&"an actual supplied image"));
  assert!(grounded_texts(&store.context(131,Some("vivian"),false),"vivian",131).is_empty());
 }
 #[test]fn switching_documents_or_losing_foreground_discards_old_text_immediately(){
  let store=PerceptionStore::default();let epoch=store.configure(PerceptionConfig::default());
  store.foreground(epoch,json!({"pid":1,"process":"editor","title":"first"}),100);
  store.report(epoch,Reading::new("accessibility","observed",100,100,json!({"content":{"status":"observed","document_excerpt":"old document"}})),None,false);
  store.foreground(epoch,json!({"pid":1,"process":"editor","title":"second"}),101);assert!(!store.context(101,None,false).to_string().contains("old document"));
  store.foreground_unavailable(epoch,102,"password manager focused");let context=store.context(102,None,false);assert!(!context.to_string().contains("second"));assert!(!context.to_string().contains("old document"));
 }
 #[test]fn missing_samples_break_sustained_pressure_and_recovery_is_a_new_environment_event(){
  let store=PerceptionStore::default();let epoch=store.configure(PerceptionConfig::default());let high=json!({"memory_percent":95,"cpu_percent":10});
  store.resource_report(epoch,100,100,high.clone(),true);store.resource_report(epoch,110,110,json!(null),true);
  store.resource_report(epoch,120,120,high.clone(),true);store.resource_report(epoch,130,130,high.clone(),true);assert!(store.changes(130).is_empty());
  store.resource_report(epoch,140,140,high.clone(),true);assert_eq!(store.changes(140).len(),1);assert!(store.memory_pressure_at(140));
  store.resource_report(epoch,150,150,json!({"memory_percent":70,"cpu_percent":10}),true);assert_eq!(store.changes(150).len(),2);assert!(!store.memory_pressure_at(150));
 }
 #[test]fn fresh_context_revisions_cancel_old_plans_but_metric_jitter_does_not_invent_events(){
  let store=PerceptionStore::default();let epoch=store.configure(PerceptionConfig::default());
  store.report(epoch,Reading::new("accessibility","observed",100,40,json!({"text":"first"})),None,false);
  let (old_revision,_)=store.context_with_revision(100,None,true);
  store.report(epoch,Reading::new("accessibility","observed",101,40,json!({"text":"second"})),None,false);assert_ne!(old_revision,store.revision.load(Ordering::Acquire));assert!(store.changes(101).is_empty());
  store.resource_report(epoch,100,100,json!({"memory_percent":50,"cpu_percent":10}),true);let stable=store.revision.load(Ordering::Acquire);
  store.resource_report(epoch,110,110,json!({"memory_percent":51,"cpu_percent":11}),true);assert_eq!(stable,store.revision.load(Ordering::Acquire));
 }
 #[test]fn legacy_resource_actions_are_not_deliverable_but_pet_requested_screen_analysis_is(){
  let action=|trigger:&str|proactive::ProactiveAction{trigger:trigger.into(),work_notice_id:None,content:"non-empty content".into(),content_type:proactive::ContentType::Text,value_score:None};
  assert!(!deliverable_proactive_action(&action("system_pressure")));assert!(!deliverable_proactive_action(&action("pressure")));
  assert!(deliverable_proactive_action(&action("screen_peek")));assert!(deliverable_proactive_action(&action("spontaneous")));
 }
}
`);
const result=spawnSync('cargo',['test','--offline','--manifest-path',path.join(dir,'Cargo.toml'),'-j','1'],{
 cwd:root,stdio:'inherit',timeout:180000,env:{...process.env,CARGO_INCREMENTAL:'0',CARGO_PROFILE_DEV_DEBUG:'0'},
});
if(result.error)throw result.error;
process.exitCode=result.status??1;
