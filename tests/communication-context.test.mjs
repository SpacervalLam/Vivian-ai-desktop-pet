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
mod messages {use serde::{Serialize,Deserialize};${communication}}
mod types{pub mod response{
#[derive(Clone,Default)]pub struct Sticker{pub label:String,pub meaning:String}
#[derive(Clone,Default)]pub struct Meta{pub communication:Option<crate::messages::CommunicationContext>,pub sticker:Option<Sticker>}
impl std::fmt::Debug for Meta{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("Meta")}}
#[derive(Clone,Default,Debug)]pub struct ChatMessage{pub role:String,pub content:String,pub timestamp:Option<chrono::DateTime<chrono::Local>>,pub meta:Option<Meta>}
impl ChatMessage {pub fn system(s:impl Into<String>)->Self{Self{role:"system".into(),content:s.into(),timestamp:None,meta:None}}pub fn user(s:impl Into<String>)->Self{Self{role:"user".into(),content:s.into(),timestamp:None,meta:None}}pub fn assistant(s:impl Into<String>)->Self{Self{role:"assistant".into(),content:s.into(),timestamp:None,meta:None}}}
}}
${context}
#[cfg(test)]mod tests{use super::*;${tests.replaceAll('crate::messages::MessageMeta::default()', 'crate::types::response::Meta::default()')}}

`);
 const manifest=path.join(dir,'Cargo.toml');
 await writeFile(manifest,'[package]\nname="communication-context-fixture"\nversion="0.0.0"\nedition="2021"\n[dependencies]\nserde={version="1",features=["derive"]}\nserde_json="1"\nchrono="0.4"\n[[bin]]\nname="communication-context-fixture"\npath="prompt.rs"\n');
 const run=spawnSync('cargo',['test','--manifest-path',manifest,'--quiet'],{encoding:'utf8',env:{...process.env,CARGO_PROFILE_DEV_DEBUG:'0',CARGO_BUILD_JOBS:'2'}});
 assert.equal(run.status,0,run.stdout+run.stderr);
 console.log(run.stdout.trim());
}finally{await rm(dir,{recursive:true,force:true});}
