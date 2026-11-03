// Execute the production learning policy and persistence helper without models,
// browsers or application data. These tests do not claim real-model relevance.
import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';

const root = process.cwd();
const dir = path.join(root, 'tmp/interest-learning-tests');
const rustPath = file => path.join(root, file).replaceAll('\\', '/');
await mkdir(path.join(dir, 'src'), { recursive: true });
const context = await readFile('src-tauri/src/discovery/learning_context.rs', 'utf8');
const start = context.indexOf('static STATE_WRITE:');
const end = context.indexOf('\nfn anchors(', start);
assert.ok(start >= 0 && end > start);
await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-interest-learning-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
sha2="0.10"
parking_lot="0.12"
tempfile="3"
tracing="0.1"
`);
await writeFile(path.join(dir, 'src/lib.rs'), `#![allow(dead_code)]
#[path="${rustPath('src-tauri/src/discovery/learning_interest.rs')}"] mod learning_interest;
use learning_interest::{LearningState,LearningAnchor,LearningProposal};
mod utils {
 pub mod path {
  pub fn get_character_data_dir(id:&str)->std::path::PathBuf {
   static ROOT:std::sync::OnceLock<tempfile::TempDir>=std::sync::OnceLock::new();
   ROOT.get_or_init(||tempfile::tempdir().unwrap()).path().join(id)
  }
 }
 pub mod fs {
  pub fn write_atomic(path:&std::path::Path,text:&str)->std::io::Result<()>{std::fs::write(path,text)}
  pub fn load_json_or_backup<T:serde::de::DeserializeOwned>(path:&std::path::Path)->Option<T>{
   serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
  }
 }
}
mod persona {
 #[path="${rustPath('src-tauri/src/persona/self_profile.rs')}"] pub mod self_profile;
 #[path="${rustPath('src-tauri/src/persona/self_profile_store.rs')}"] pub mod self_profile_store;
}
${context.slice(start, end)}
#[cfg(test)] mod persistence {
 use super::*;
 #[test] fn busy_and_thought_paths_share_reservations_but_characters_do_not() {
  let anchor=||LearningAnchor::new("memory:real-id","recent_topic","一次真实对话提到了游戏机制里的连锁反应");
  assert_eq!(with_state("shared",|s|s.begin_evaluation(vec![anchor()],86400)).unwrap().len(),1);
  assert!(with_state("shared",|s|s.begin_evaluation(vec![anchor()],90000)).unwrap().is_empty());
  assert_eq!(with_state("other",|s|s.begin_evaluation(vec![anchor()],90000)).unwrap().len(),1);
  let proposal=||LearningProposal{anchor_id:"memory:real-id".into(),source_quote:"游戏机制里的连锁反应".into(),
   focus:"连锁反应".into(),query:"游戏物理连锁反应机制".into(),question:"不同游戏如何实现连续的连锁反应？".into(),personal_motive:"我想弄懂自己觉得巧妙的机制".into()};
  assert!(with_state("shared",|s|s.reserve(proposal(),&[anchor()],90000)).unwrap().is_some());
  assert!(with_state("shared",|s|s.reserve(proposal(),&[anchor()],90100)).unwrap().is_none());
 }
 #[test] fn corrupt_or_unwritable_state_cannot_reset_budgets_or_authorize_search() {
  let dir=utils::path::get_character_data_dir("broken").join("discovery");
  std::fs::create_dir_all(&dir).unwrap();let file=dir.join("private_learning.json");
  std::fs::write(&file,"invalid json").unwrap();
  assert!(with_state("broken",|s|s.begin_evaluation(vec![],86400)).is_err());
  assert_eq!(std::fs::read_to_string(&file).unwrap(),"invalid json");
  let blocked=utils::path::get_character_data_dir("blocked");std::fs::write(&blocked,"not a directory").unwrap();
  assert!(with_state("blocked",|s|s.begin_evaluation(vec![LearningAnchor::new("a","base_interest","这是实际的个人兴趣来源")],86400)).is_err());
 }
}
`);
const run = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1', '--', '--test-threads=1'], {
  cwd: root, stdio: 'inherit', env: { ...process.env, CARGO_INCREMENTAL: '0' },
});
if (run.error) throw run.error;
process.exitCode = run.status ?? 1;
