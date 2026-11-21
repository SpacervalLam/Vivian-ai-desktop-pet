import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

// Compile the actual state machine, parser, corpus and atomic filesystem code.
// Only desktop paths, unrelated error types and unused model transport are
// replaced; no growth or validation rule is reimplemented in this fixture.
const root = process.cwd();
const abs = file => JSON.stringify(path.resolve(root, file).replaceAll('\\', '/'));
const dir = await mkdtemp(path.join(tmpdir(), 'vivian-persona-evolution-'));
try {
  await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="persona-evolution-contract"
version="0.0.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json={version="1",features=["float_roundtrip"]}
parking_lot="0.12"
sha2="0.10"
uuid={version="1",features=["v4"]}
once_cell="1"
tracing="0.1"
tempfile="3"
thiserror="2"
[target.'cfg(windows)'.dependencies]
windows={version="0.61",features=["Win32_Storage_FileSystem"]}
[[bin]]
name="persona-evolution-contract"
path="main.rs"
`);
  await writeFile(path.join(dir, 'main.rs'), `
#![allow(dead_code)]
mod error {
 #[derive(Debug,thiserror::Error)] pub enum VivianError {
  #[error("{0}")] Other(String),
  #[error("{0}")] Memory(String),
 }
 pub type VivianResult<T> = Result<T,VivianError>;
}
mod utils {
 pub mod path {
  pub fn get_character_data_dir(id:&str)->std::path::PathBuf {
   std::path::PathBuf::from(std::env::var_os("VIVIAN_TEST_DATA_DIR").unwrap()).join("characters").join(id)
  }
 }
 #[path=${abs('src-tauri/src/utils/fs.rs')}] pub mod fs;
}
mod memory {pub mod types {pub fn current_timestamp()->f64 {
 std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64()
}}}
mod types {pub mod response {
 pub struct ChatMessage;
 impl ChatMessage {
  pub fn system(_:impl Into<String>)->Self {Self}
  pub fn user(_:impl Into<String>)->Self {Self}
 }
}}
mod providers {
 pub mod base {
  pub struct LLMRequest;
  impl LLMRequest {
   pub fn new(_:&str,_:Vec<crate::types::response::ChatMessage>)->Self {Self}
   pub fn without_framework_instructions(self)->Self {self}
   pub fn with_json_schema(self,_:serde_json::Value)->Self {self}
   pub fn with_character_id(self,_:String)->Self {self}
  }
 }
 pub struct ModelRouter;
 impl ModelRouter {pub async fn generate(&self,_:base::LLMRequest)->crate::error::VivianResult<String> {
  panic!("offline growth contracts must not call a model")
 }}
}
mod persona {
 #[path=${abs('src-tauri/src/persona/example_library.rs')}] pub mod example_library;
 #[path=${abs('src-tauri/src/persona/evolution.rs')}] pub mod evolution;
 #[path=${abs('src-tauri/src/persona/evolution_agent.rs')}] pub mod evolution_agent;
}
fn main() {}
`);
  const run = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1', '--', '--test-threads=1'], {
    cwd: root,
    encoding: 'utf8',
    timeout: 240_000,
    maxBuffer: 8 * 1024 * 1024,
    env: { ...process.env, VIVIAN_TEST_DATA_DIR: path.join(dir, 'data'), CARGO_TARGET_DIR: path.join(root, 'tmp/persona-evolution-target') },
  });
  assert.equal(run.status, 0, run.stdout + run.stderr + (run.error?.message ?? ''));
  console.log(run.stdout.trim());
} finally {
  assert.equal(path.dirname(path.resolve(dir)), path.resolve(tmpdir()));
  assert.ok(path.basename(dir).startsWith('vivian-persona-evolution-'));
  await rm(dir, { recursive: true, force: true });
}
