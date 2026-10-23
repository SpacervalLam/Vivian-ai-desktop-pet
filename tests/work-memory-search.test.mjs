import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
const source=(await readFile('src-tauri/src/tools/builtin/session_query_tools.rs','utf8')).replaceAll('\r\n','\n');
const helpers=source.slice(source.indexOf('fn recent_session_files('),source.indexOf('// ============================================================================'));
const callStart=source.indexOf('    async fn call(&self, args: Value, ctx: &ToolUseContext)');
const call=source.slice(callStart,source.indexOf('    fn is_read_only',callStart)).replace('async fn call(&self,','fn search(');
const findStart=source.indexOf('fn find_session(');
const find=source.slice(findStart,source.indexOf('// ============================================================================',findStart));
const dir=await mkdtemp(path.join(tmpdir(),'vivian-memory-search-'));
try {
await writeFile(path.join(dir,'lib.rs'),`#![allow(dead_code)]
use serde_json::{json,Value};use std::path::PathBuf;
const MAX_SESSIONS_SCAN:usize=200;
static DATA:std::sync::LazyLock<tempfile::TempDir>=std::sync::LazyLock::new(||tempfile::tempdir().unwrap());
fn sessions_dir()->PathBuf{DATA.path().into()}
struct ToolUseContext{session_id:String,char_id:String,working_directory:String}
struct ToolResult{data:Value}
impl ToolResult{fn standard_success(_:&str,data:Option<Value>)->Self{Self{data:data.unwrap()}}}
mod brain{pub mod coding_session_store{
 pub fn body_path(dir:&std::path::Path,id:&str)->std::path::PathBuf{dir.join(format!("{id}.json"))}
 pub fn load_heads<T:serde::de::DeserializeOwned>(dir:&std::path::Path)->Result<Option<Vec<(String,T)>>,String>{
  let mut heads=Vec::new();for e in std::fs::read_dir(dir).unwrap(){let e=e.unwrap();let bytes=std::fs::read(e.path()).unwrap();let value:serde_json::Value=serde_json::from_slice(&bytes).unwrap();heads.push((value["session_id"].as_str().unwrap().into(),serde_json::from_slice(&bytes).unwrap()));}Ok(Some(heads))
 }
}}
${helpers}
${call}
${find}
#[test]fn full_history_paging_long_unicode_and_role_isolation(){
 let ctx=ToolUseContext{session_id:"current".into(),char_id:"vivian".into(),working_directory:String::new()};
 for n in 0..250{
  let sid=format!("session-{n:03}");
  let content=if n==0{format!("{}历史证据", "中文".repeat(3000))}else{"unrelated".into()};
  let doc=json!({"session_id":sid,"char_id":"vivian","messages":[{"role":"assistant","content":content}]});
  let path=sessions_dir().join(format!("{sid}.json"));std::fs::write(&path,doc.to_string()).unwrap();
  std::fs::File::options().write(true).open(path).unwrap().set_modified(std::time::UNIX_EPOCH+std::time::Duration::from_secs(1000+n)).unwrap();
 }
 let mut args=json!({"query":"历史证据","max_sessions":40});let mut found=Vec::new();let mut pages=0;
 loop{let result=search(args.clone(),&ctx).data;found.extend(result["hits"].as_array().unwrap().clone());pages+=1;
  if result["next_cursor"].is_null(){break;}
  args["session_offset"]=result["next_cursor"]["session_offset"].clone();args["message_offset"]=result["next_cursor"]["message_offset"].clone();assert!(pages<10);
 }
 assert_eq!(pages,7);assert_eq!(found.len(),1);assert_eq!(found[0]["session_id"],"session-000");assert!(found[0]["snippet"].as_str().unwrap().contains("历史证据"));
 assert!(find_session("session-000","current","vivian").is_some());assert!(find_session("session-000","current","other").is_none());
 for e in std::fs::read_dir(sessions_dir()).unwrap(){std::fs::remove_file(e.unwrap().path()).unwrap();}
 for (sid,char_id) in [("owned","vivian"),("foreign","other")]{std::fs::write(sessions_dir().join(format!("{sid}.json")),json!({"session_id":sid,"char_id":char_id,"messages":(0..5).map(|n|json!({"role":"assistant","content":format!("needle {n}")})).collect::<Vec<_>>()} ).to_string()).unwrap();}
 let mut args=json!({"query":"needle","limit":2});let mut indices=Vec::new();
 loop{let data=search(args.clone(),&ctx).data;for hit in data["hits"].as_array().unwrap(){assert_eq!(hit["session_id"],"owned");indices.push(hit["message_index"].as_u64().unwrap());}
  if data["next_cursor"].is_null(){break;}args["session_offset"]=data["next_cursor"]["session_offset"].clone();args["message_offset"]=data["next_cursor"]["message_offset"].clone();
 }
 assert_eq!(indices,vec![0,1,2,3,4]);
 for text in ["中文🙂历史证据", "İİİneedle中文"]{assert!(!snippet(text,"needle",1).is_empty());}
}
`);
await writeFile(path.join(dir,'Cargo.toml'),`[package]\nname="vivian-memory-search-contracts"\nversion="0.0.0"\nedition="2021"\n[lib]\npath="lib.rs"\n[dependencies]\nserde={version="1",features=["derive"]}\nserde_json="1"\ntracing="0.1"\ntempfile="3"\n`);
const result=spawnSync('cargo',['test','--offline','--quiet','--manifest-path',path.join(dir,'Cargo.toml'),'--target-dir',path.resolve('tmp/work-task-contract-target')],{encoding:'utf8'});
assert.equal(result.status,0,result.stdout+result.stderr);console.log('Memory search: 250 sessions, cursors within long conversations, long Unicode messages, direct old-session reads and role isolation passed.');
} finally{await rm(dir,{recursive:true,force:true});}
