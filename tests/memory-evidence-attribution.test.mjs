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
const extractor=await readFile('src-tauri/src/memory/auto_extractor.rs','utf8');
const quoteCheck=extractor.match(/fn quote_matches_source\([\s\S]*?\n\}/)?.[0];assert.ok(quoteCheck);
const dir=await mkdtemp(path.join(tmpdir(),'vivian-communication-'));
try{
 const input=path.join(dir,'prompt.rs');
 await writeFile(input,`
use crate::types::response::ChatMessage;
mod messages {use serde::{Serialize,Deserialize};${communication}}
mod types{pub mod response{
#[derive(Clone,Default)]pub struct Sticker{pub label:String,pub meaning:String}
#[derive(Clone,Default)]pub struct Meta{pub communication:Option<crate::messages::CommunicationContext>,pub sticker:Option<Sticker>}
#[derive(Clone,Default)]pub struct ChatMessage{pub role:String,pub content:String,pub meta:Option<Meta>}
impl ChatMessage {pub fn is_memory_disabled(&self)->bool{self.role=="system"||self.role=="tool"} pub fn system(s:impl Into<String>)->Self{Self{role:"system".into(),content:s.into(),meta:None}}pub fn user(s:impl Into<String>)->Self{Self{role:"user".into(),content:s.into(),meta:None}}pub fn assistant(s:impl Into<String>)->Self{Self{role:"assistant".into(),content:s.into(),meta:None}}}
}}
${context}
${quoteCheck}
#[cfg(test)]mod tests{use super::*;#[test]fn role_user_does_not_make_roommate_speech_user_evidence(){
let mut speech=ChatMessage::user("我明天考试");let mut meta=crate::types::response::Meta::default();meta.communication=Some(crate::messages::CommunicationContext{speaker:Some("nana".into()),listener:Some("vivian".into()),current_character:Some("vivian".into()),knowledge_source:Some("heard".into())});speech.meta=Some(meta);assert!(!quote_matches_source(&[speech],"user","明天考试"));}
#[test]fn third_party_assistant_is_not_self_evidence(){
let mut speech=ChatMessage::assistant("我答应帮你");let mut meta=crate::types::response::Meta::default();meta.communication=Some(crate::messages::CommunicationContext{speaker:Some("nana".into()),listener:Some("user".into()),current_character:Some("vivian".into()),knowledge_source:Some("observed".into())});speech.meta=Some(meta);assert!(!quote_matches_source(&[speech],"self","答应帮你"));}
${tests.replaceAll('crate::messages::MessageMeta::default()', 'crate::types::response::Meta::default()')}}

`);
 const manifest=path.join(dir,'Cargo.toml');
 await writeFile(manifest,'[package]\nname="communication-context-fixture"\nversion="0.0.0"\nedition="2021"\n[dependencies]\nserde={version="1",features=["derive"]}\nserde_json="1"\n[[bin]]\nname="communication-context-fixture"\npath="prompt.rs"\n');
 const run=spawnSync('cargo',['test','--manifest-path',manifest,'--quiet'],{encoding:'utf8'});
 assert.equal(run.status,0,run.stdout+run.stderr);
 console.log(run.stdout.trim());
}finally{await rm(dir,{recursive:true,force:true});}
