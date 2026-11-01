import {mkdir,writeFile,copyFile} from 'node:fs/promises';
import {spawnSync} from 'node:child_process';
import path from 'node:path';
const root=process.cwd(), dir=path.join(root,'tmp/personal-companion-tests');
await mkdir(path.join(dir,'src'),{recursive:true});
await copyFile(path.join(root,'src-tauri/Cargo.lock'),path.join(dir,'Cargo.lock'));
await writeFile(path.join(dir,'Cargo.toml'),`[package]
name="vivian-personal-companion-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
parking_lot="0.12"
tempfile="3"
chrono={version="0.4",features=["serde"]}
rand="0.8"
tracing="0.1"
`);
await writeFile(path.join(dir,'src/lib.rs'),`#![allow(dead_code)]
mod utils {pub mod fs {pub fn write_atomic(path:&std::path::Path,text:&str)->std::io::Result<()>{std::fs::write(path,text)}}}
#[path="${path.join(root,'src-tauri/src/mind/personal.rs').replaceAll('\\','/')}"]mod personal;
mod error {#[derive(Debug)]pub enum VivianError{Other(String)}pub type VivianResult<T>=Result<T,VivianError>;}
#[path="${path.join(root,'src-tauri/src/mind/user_goals.rs').replaceAll('\\','/')}"]mod user_goals;
#[cfg(test)]mod scenarios{
use super::personal::*;use serde_json::{json,Value};
fn ev()->Evidence{Evidence{id:"real_user_turn".into(),kind:"user".into(),quote:"下周面试，请先帮我准备材料".into(),at:100}}
fn edit(v:Value)->Edit{serde_json::from_value(v).unwrap()}
fn record(store:&Personal,kind:&str,topic:&str,deps:Vec<String>)->Value{store.apply(edit(json!({"action":"record","kind":kind,"topic":topic,"text":format!("真实记录内容 {topic}"),"depends_on":deps,"next_step":"整理材料","completion_criteria":"用户确认材料齐全"})),ev(),100).unwrap()}
#[test]fn affairs_survive_days_and_restart_and_due_review_does_not_become_repeated_speech(){
let dir=tempfile::tempdir().unwrap();let path=dir.path().join("personal.json");let store=Personal::shared(path.clone());
let r=store.apply(edit(json!({"action":"record","kind":"affair","topic":"面试","text":"准备面试","next_step":"整理材料","completion_criteria":"材料齐全","follow_at":200})),ev(),100).unwrap();
assert!(!store.review_due(199).unwrap());assert!(store.review_due(200).unwrap());assert!(!store.review_due(201).unwrap());drop(store);
let store=Personal::shared(path);assert!(!store.review_due(300).unwrap());assert_eq!(store.view(300)["due"][0],r["id"]);assert_eq!(store.plan(300)["decisions"][0]["suggested_action"],"prepare");
}
#[test]fn corrections_invalidate_all_derived_interpretations_and_plans(){
let dir=tempfile::tempdir().unwrap();let s=Personal::shared(dir.path().join("p"));let a=record(&s,"understanding","旧工作",vec![]);let b=record(&s,"method","旧流程",vec![a["id"].as_str().unwrap().into()]);record(&s,"affair","准备",vec![b["id"].as_str().unwrap().into()]);
s.apply(edit(json!({"action":"correct","id":a["id"],"kind":"understanding","topic":"新工作","text":"已换工作"})),ev(),200).unwrap();let v=s.view(200);assert_eq!(v["records"].as_array().unwrap().len(),1);assert_eq!(v["metrics"]["invalidated"],3);
}
#[test]fn user_silence_never_authorizes_peer_answers_or_completion(){
let dir=tempfile::tempdir().unwrap();let s=Personal::shared(dir.path().join("p"));let r=s.apply(edit(json!({"action":"record","kind":"affair","topic":"选择","text":"等用户选择","next_step":"等待答复","waiting_for":"user","completion_criteria":"用户做出选择"})),ev(),100).unwrap();assert_eq!(s.plan(99999)["decisions"][0]["suggested_action"],"wait_user");
let mut peer=ev();peer.kind="peer_report".into();assert!(s.apply(edit(json!({"action":"transition","id":r["id"],"state":"completed"})),peer,200).is_err());
}
#[test]fn methods_and_boundaries_require_actual_user_feedback(){
let dir=tempfile::tempdir().unwrap();let s=Personal::shared(dir.path().join("p"));let mut peer=ev();peer.kind="peer_report".into();
assert!(s.apply(edit(json!({"action":"record","kind":"method","topic":"报告","text":"先写结论"})),peer,100).is_err());
let m=record(&s,"method","报告",vec![]);assert_eq!(s.context(101)["records"][0]["id"],m["id"]);
}
#[test]fn private_experiences_stay_private_while_explicitly_shared_interests_are_visible(){
let dir=tempfile::tempdir().unwrap();let s=Personal::shared(dir.path().join("p"));record(&s,"experience","私人回忆",vec![]);assert!(s.peer_context()["records"].as_array().unwrap().is_empty());assert!(!s.permits_peer_text("真实记录内容 私人回忆"));
s.apply(edit(json!({"action":"record","kind":"interest","topic":"音乐","text":"喜欢讨论音乐","scope":"shareable"})),ev(),100).unwrap();assert_eq!(s.peer_context()["records"].as_array().unwrap().len(),1);
}
#[test]fn quiet_boundary_affects_contact_and_new_user_choice_replaces_it(){
let dir=tempfile::tempdir().unwrap();let s=Personal::shared(dir.path().join("p"));
for (mode,expected) in [("quiet",false),("discuss",true)]{s.apply(edit(json!({"action":"record","kind":"boundary","topic":"*","text":format!("用户选择 {mode}"),"response_mode":mode})),ev(),100).unwrap();assert_eq!(s.contact_allowed("新话题"),expected);}
}
#[test]fn forgetting_erases_dependent_content_and_cancelled_affairs_stop_plans(){
let dir=tempfile::tempdir().unwrap();let s=Personal::shared(dir.path().join("p"));let a=record(&s,"experience","秘密经历",vec![]);record(&s,"understanding","推断",vec![a["id"].as_str().unwrap().into()]);
s.apply(edit(json!({"action":"forget","id":a["id"]})),ev(),200).unwrap();let raw=std::fs::read_to_string(dir.path().join("p")).unwrap();assert!(!raw.contains("秘密经历"));assert!(!raw.contains("真实记录内容 推断"));
let b=record(&s,"affair","面试",vec![]);s.apply(edit(json!({"action":"transition","id":b["id"],"state":"cancelled"})),ev(),300).unwrap();assert!(s.plan(400)["decisions"].as_array().unwrap().is_empty());
}
#[test]fn echo_extraction_is_idempotent_and_missing_dependencies_are_rejected(){
let dir=tempfile::tempdir().unwrap();let s=Personal::shared(dir.path().join("p"));let a=record(&s,"interest","音乐",vec![]);let b=record(&s,"interest","音乐",vec![]);assert_eq!(a,b);assert_eq!(s.view(200)["records"].as_array().unwrap().len(),1);
assert!(s.apply(edit(json!({"action":"record","kind":"understanding","topic":"无来源","text":"错误","depends_on":["invented"]})),ev(),200).is_err());
}
#[test]fn failed_saves_and_corrupt_files_preserve_previous_truth(){
let dir=tempfile::tempdir().unwrap();let blocked=dir.path().join("file");std::fs::write(&blocked,"original").unwrap();let s=Personal::shared(blocked.join("p"));assert!(s.apply(edit(json!({"action":"record","kind":"interest","topic":"a","text":"b"})),ev(),200).is_err());assert!(s.view(200)["records"].as_array().unwrap().is_empty());
let bad=dir.path().join("corrupt");std::fs::write(&bad,"bad json").unwrap();let s=Personal::shared(bad.clone());assert!(s.clear().is_err());assert_eq!(std::fs::read_to_string(bad).unwrap(),"bad json");
}
#[test]fn rebuilds_share_a_writer_and_invalidated_external_evidence_cascades(){
let dir=tempfile::tempdir().unwrap();let path=dir.path().join("p");let s=Personal::shared(path.clone());let other=Personal::shared(path);assert!(std::sync::Arc::ptr_eq(&s,&other));record(&s,"experience","经历",vec![]);s.invalidate_source("real_user_turn").unwrap();assert!(other.view(200)["records"].as_array().unwrap().is_empty());
}
}
`);
const r=spawnSync('cargo',['test','--offline','--manifest-path',path.join(dir,'Cargo.toml'),'-j','1'],{stdio:'inherit',timeout:180000,env:{...process.env,CARGO_INCREMENTAL:'0',CARGO_PROFILE_DEV_DEBUG:'0'}});
if(r.error)throw r.error;process.exitCode=r.status??1;
