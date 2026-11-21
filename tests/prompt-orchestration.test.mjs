import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
const source=(await readFile('src-tauri/src/pipeline/message_context.rs','utf8')).replaceAll('\r\n','\n');
const context=source.slice(source.indexOf('pub fn communication_note'),source.indexOf('#[cfg(test)]'));
const tests=source.slice(source.indexOf('    #[test]\n    fn literal_prefix'),source.lastIndexOf('\n}'));
const metaSource=await readFile('src-tauri/src/messages.rs','utf8');
const communication=metaSource.slice(metaSource.indexOf('#[derive',metaSource.indexOf('/// Trusted routing')),metaSource.indexOf('/// 消息元数据'));
const companion=await readFile('src-tauri/src/pipeline/companion_prompt.rs','utf8');
const positions=companion.match(/enum Position \{[^\n]+\}/)?.[0];assert.ok(positions);
const extractedAssembly=companion.match(/    pub fn messages_with_communication\([\s\S]*?\n    \}/)?.[0];assert.ok(extractedAssembly);
const assembly=extractedAssembly.replaceAll('super::spoken_reply::PRIVATE_OPERATION','PRIVATE_OPERATION');
const privateRule=companion.match(/fn private_operation\([\s\S]*?\n\}/)?.[0];assert.ok(privateRule);
const spoken=await readFile('src-tauri/src/pipeline/spoken_reply.rs','utf8');
const privateMarker=spoken.match(/const PRIVATE_OPERATION: &str = ([^\n]+);/)?.[1];assert.ok(privateMarker);
const boundary=JSON.stringify(await readFile('src-tauri/prompts/execution/companion-data-boundary.en.md','utf8'));
const promptTime=path.resolve('src-tauri/src/utils/prompt_time.rs').replaceAll('\\','/');
const dir=await mkdtemp(path.join(tmpdir(),'vivian-communication-'));
try{
 const input=path.join(dir,'prompt.rs');
 await writeFile(input,`
use crate::types::response::ChatMessage;
use chrono::{DateTime,Local};
#[path=${JSON.stringify(promptTime)}] pub mod prompt_time;
mod utils{pub use crate::prompt_time;}
use utils::prompt_time::{format_duration,format_local_time,format_relative_age_at};
mod pipeline{pub mod prompt_modules{pub fn normalize_lang(lang:&str)->&str{let l=lang.trim().to_ascii_lowercase();if l.starts_with("en"){"en"}else if l.starts_with("ja"){"ja"}else{"zh"}}}}
use pipeline::prompt_modules::normalize_lang;
mod i18n{pub fn get_language()->String{"zh".into()}}
mod messages {use serde::{Serialize,Deserialize};${communication}}
mod types{pub mod response{
#[derive(Clone,Default)]pub struct Sticker{pub label:String,pub meaning:String}
#[derive(Clone,Default)]pub struct Meta{pub communication:Option<crate::messages::CommunicationContext>,pub sticker:Option<Sticker>}
impl std::fmt::Debug for Meta{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("Meta")}}
#[derive(Clone,Default,Debug)]pub struct ChatMessage{pub role:String,pub content:String,pub timestamp:Option<chrono::DateTime<chrono::Local>>,pub meta:Option<Meta>}
impl ChatMessage {pub fn system(s:impl Into<String>)->Self{Self{role:"system".into(),content:s.into(),timestamp:None,meta:None}}pub fn user(s:impl Into<String>)->Self{Self{role:"user".into(),content:s.into(),timestamp:None,meta:None}}pub fn assistant(s:impl Into<String>)->Self{Self{role:"assistant".into(),content:s.into(),timestamp:None,meta:None}}}
}}
${context}
mod message_context{pub use crate::{append_current_turn,append_history,append_history_with_time,temporal_anchor_note};}
const DATA_BOUNDARY:&str=${boundary};
#[derive(PartialEq)]${positions}
struct Block{id:String,position:Position,content:String}
struct CompanionPrompt{blocks:Vec<Block>,language:String}
const PRIVATE_OPERATION:&str=${privateMarker};
${privateRule}
impl CompanionPrompt{fn contents(&self,position:Position)->Vec<&str>{self.blocks.iter().filter(|b|b.position==position).map(|b|b.content.as_str()).collect()}${assembly}}
#[cfg(test)]mod tests{use super::*;#[test]fn history_precedes_dynamic_context_and_current_input_stays_original(){
let p=CompanionPrompt{blocks:vec![(Position::Main,"rules".into()),(Position::Example,"fictional example".into()),(Position::Context,"retrieved context".into()),(Position::PostHistory,"final protocol".into())].into_iter().enumerate().map(|(i,(position,content))|Block{id:format!("fixture_{i}"),position,content}).collect(),language:String::new()};
let history=vec![ChatMessage::user("old user"),ChatMessage::assistant("old answer")];
let input="[Nana says to me] 用户自己输入的文本";
let messages=p.messages_with_communication(&history,input,false,None,None);
assert_eq!(messages[1].content,"old user");assert_eq!(messages[2].role,"assistant");assert!(messages[3].content.contains("retrieved context"));assert_eq!(messages[4].content,"fictional example");let turn=messages.len()-2;assert_eq!(messages[turn].content,input);assert_eq!(messages[turn].role,"user");assert_eq!(messages.iter().filter(|m|m.content.contains(input)).count(),1);assert_eq!(messages.last().unwrap().content,"final protocol");}
#[test]fn missing_participant_is_not_guessed(){let c=crate::messages::CommunicationContext::default();let note=communication_note(&c);assert!(note.contains("未记录"));assert!(!note.contains("用户 →"));}
#[test]fn routing_note_is_brief_and_readable(){let c=crate::messages::CommunicationContext{speaker:Some("user".into()),listener:Some("nana".into()),current_character:Some("vivian".into()),knowledge_source:Some("observed".into())};let note=communication_note(&c);assert!(note.contains("用户 → Nana"));assert!(note.contains("不是向 Vivian 提问"));assert!(!note.contains("knowledge_source"));assert!(note.chars().count()<120);}
${tests.replaceAll('crate::messages::MessageMeta::default()', 'crate::types::response::Meta::default()')}}

`);
 const manifest=path.join(dir,'Cargo.toml');
 await writeFile(manifest,'[package]\nname="communication-context-fixture"\nversion="0.0.0"\nedition="2021"\n[dependencies]\nserde={version="1",features=["derive"]}\nserde_json="1"\nchrono="0.4"\n[[bin]]\nname="communication-context-fixture"\npath="prompt.rs"\n');
 const run=spawnSync('cargo',['test','--manifest-path',manifest,'--quiet'],{encoding:'utf8',env:{...process.env,CARGO_PROFILE_DEV_DEBUG:'0',CARGO_BUILD_JOBS:'2'}});
 assert.equal(run.status,0,run.stdout+run.stderr);
 console.log(run.stdout.trim());
}finally{await rm(dir,{recursive:true,force:true});}
