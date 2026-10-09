import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
const src=await readFile('src-tauri/src/cross_character.rs','utf8');
const parse=src.match(/pub fn parse_any_speaker_prefix\([\s\S]*?\n\}/)?.[0];
const anchor=src.match(/fn strip_memory_anchor\([\s\S]*?\n\}/)?.[0];
const dialogue=await readFile('src-tauri/src/dialogue/mod.rs','utf8');
const history=dialogue.match(/pub struct HistoryEntry \{[\s\S]*?\n\}/)?.[0];
assert.ok(parse&&anchor&&history);
const dir=await mkdtemp(path.join(tmpdir(),'vivian-conversation-source-'));
try {
 const modules=['types','kinds','conversations','conversation_semantics'].map(name=>`#[path=${JSON.stringify(path.resolve('src-tauri/src/memory',name+'.rs').replaceAll('\\','/'))}]pub mod ${name};`).join('\n');
 await writeFile(path.join(dir,'lib.rs'),`${modules}
pub mod memory{pub use crate::{types,kinds,conversations,conversation_semantics};}
pub mod cross_character{${anchor}\n${parse}}
pub mod dialogue{#[derive(Clone,Debug,serde::Serialize,serde::Deserialize)]${history}}
#[path=${JSON.stringify(path.resolve('src-tauri/src/utils/fs.rs').replaceAll('\\','/'))}]pub mod fs;
pub mod utils{pub use crate::fs;}`);
 await writeFile(path.join(dir,'Cargo.toml'),'[package]\nname="conversation-source-contract"\nversion="0.0.0"\nedition="2021"\n[lib]\npath="lib.rs"\n[dependencies]\nserde={version="1",features=["derive"]}\nserde_json="1"\nparking_lot="0.12"\nuuid={version="1",features=["v4"]}\nchrono="0.4"\ntracing="0.1"\ntempfile="3"\nsha2="0.10"\nschemars="0.8"\nwindows={version="0.61",features=["Win32_Storage_FileSystem"]}\n');
 const run=spawnSync('cargo',['test','--manifest-path',path.join(dir,'Cargo.toml'),'--quiet'],{encoding:'utf8'});
 assert.equal(run.status,0,run.stdout+run.stderr);console.log(run.stdout.trim());
} finally { await rm(dir,{recursive:true,force:true}); }
