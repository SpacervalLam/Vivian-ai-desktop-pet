// Run the production SQLite, relation-state and recall-policy modules without
// launching Tauri, loading application data or calling any model provider.
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const dir = await mkdtemp(path.join(tmpdir(), 'vivian-memory-state-'));
const source = name => path.resolve(`src-tauri/src/${name}.rs`).replaceAll('\\', '/');
const manager = await readFile('src-tauri/src/memory/manager.rs', 'utf8');
function rustItem(marker) {
  const start = manager.indexOf(marker);
  assert.ok(start >= 0, marker);
  const open = manager.indexOf('{', start);
  let depth = 1, end = open + 1;
  for (; end < manager.length && depth; end++) {
    if (manager[end] === '{') depth++;
    if (manager[end] === '}') depth--;
  }
  assert.equal(depth, 0, marker);
  return manager.slice(start, end);
}
try {
  await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-memory-state-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
parking_lot="0.12"
rusqlite={version="0.32",features=["bundled","backup"]}
sha2="0.10"
chrono="0.4"
uuid={version="1",features=["v4"]}
tracing="0.1"
once_cell="1"
jieba-rs="0.7"
tempfile="3"
[target.'cfg(windows)'.dependencies]
windows={version="0.61",features=["Win32_Storage_FileSystem"]}
[lib]
path="lib.rs"
`);
  await writeFile(path.join(dir, 'lib.rs'), `#![allow(dead_code)]
pub mod error {
 #[derive(Debug)] pub enum VivianError {Memory(String),Serialization(String)}
 impl std::fmt::Display for VivianError {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"{self:?}")}}
 pub type VivianResult<T> = Result<T,VivianError>;
}
pub mod utils {
 #[path="${source('utils/sqlite')}"]pub mod sqlite;
 #[path="${source('utils/fs')}"]pub mod fs;
}
pub mod memory {pub use crate::types;}
${['types','kinds','provenance','tokenize','recall_policy','ongoing_tasks','search_index','entry_store'].map(name => `#[path="${source(`memory/${name}`)}"]pub mod ${name};`).join('\n')}
pub use entry_store::relation_state;
#[cfg(test)]mod manager_probe {
 use std::{sync::Arc,collections::HashMap};
 use parking_lot::{Mutex,RwLock};
 use crate::{types::{MemoryItem,MemoryStoreData},error::{VivianError,VivianResult}};
 struct Vectors;
 impl Vectors {fn delete_by_memory_id(&self,_:&str)->bool{false}fn save_to(&self)->VivianResult<()>{Ok(())}}
 struct Inner {
  data:MemoryStoreData,id_index:HashMap<String,usize>,changes:crate::entry_store::EntryChanges,
  dirty:bool,revision:u64,entry_store:Arc<crate::entry_store::MemoryEntryStore>,vector_store:Vectors,
 }
 impl Inner {
  ${['fn mark_changed(', 'fn entry_mut(', 'fn mark_removed(', 'fn rebuild_index(', 'fn remove_by_id(', 'fn flush_dirty('].map(rustItem).join('\n')}
  fn save_to_disk(&mut self)->VivianResult<()>{self.changes.flush(&self.entry_store,&self.data.entries,&self.id_index)?;self.dirty=false;Ok(())}
 }
 struct Manager {inner:RwLock<Inner>,char_id:String,search_cache:Mutex<HashMap<String,String>>}
 impl Manager {
  ${['pub fn supersede_memory(', 'pub fn archive_memory(', 'pub fn recall_relation_bundles('].map(rustItem).join('\n')}
 }
 #[path="${path.resolve('tests/fixtures/memory-state-manager.rs').replaceAll('\\','/')}"]mod tests;
}
`);
  const run = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '--quiet'], {
    encoding: 'utf8', timeout: 240000,
    env: { ...process.env, CARGO_TARGET_DIR: path.resolve('tmp/memory-state-target') },
  });
  assert.ifError(run.error);
  assert.equal(run.status, 0, run.stdout + run.stderr);
  console.log(run.stdout.trim());
} finally {
  assert.equal(path.dirname(path.resolve(dir)), path.resolve(tmpdir()));
  await rm(dir, { recursive: true, force: true });
}
