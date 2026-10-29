import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const root = process.cwd();
const fragments = await Promise.all(['vivian', 'nana'].map(async name =>
  JSON.parse(await readFile(`src-tauri/prompts/characters/${name}/voice_fragments.json`, 'utf8'))));
for (const entries of fragments) {
  const ids = new Set();
  for (const e of entries) {
    assert.ok(e.id.startsWith('voice-') && !ids.has(e.id)); ids.add(e.id);
    assert.equal(e.user, '', 'a single utterance must not acquire a fake user turn');
    assert.ok(e.cues.length && e.situation && e.attention);
    assert.deepEqual(Object.keys(e.response).sort(), ['intent', 'text']);
    assert.deepEqual(Object.keys(e.response.text).sort(), ['en', 'ja', 'zh']);
    for (const lang of ['zh', 'ja', 'en']) {
      assert.ok(e.response.text[lang].trim() && [...e.response.text[lang]].length <= 180);
    }
    assert.ok(!('source' in e), 'research provenance stays outside app assets');
  }
  assert.ok(entries.some(e => e.id.startsWith('voice-ja-')));
  assert.ok(entries.some(e => e.id.startsWith('voice-en-')));
}
for (const character of ['vivian','nana']) {
  for (const suffix of ['', '.en', '.ja']) {
    const text = await readFile(`src-tauri/prompts/characters/${character}/examples${suffix}.md`, 'utf8');
    assert.ok(!text.includes('Response:') && !text.includes('**Example '), 'static files contain rules, not self-written dialogues');
  }
}

// Compile the production retriever and real follow-up predicate in a small
// fixture crate, as other prompt contract tests do. Only transport/types and an
// explicitly offline embedding provider are replaced; no ranking is reimplemented.
const rustItem = (source, marker) => {
  const start = source.indexOf(marker);
  assert.ok(start >= 0, marker);
  const end = source.indexOf('\n}', start);
  assert.ok(end > start, marker);
  return source.slice(start, end + 2);
};
const abs = file => path.resolve(root, file).replaceAll('\\', '/');
const source = (await readFile('src-tauri/src/persona/example_retriever.rs', 'utf8')).replaceAll('\r\n', '\n');
// Keep the retriever's existing unit tests too. Its full CompanionPrompt
// integration test belongs to the app crate; test that corpus's retrieval here
// without substituting a fake prompt builder.
const integrationStart = source.indexOf('    #[test]\n    fn sourced_fragments_reach_companion_prompt_without_fabricated_turns');
const integrationEnd = source.indexOf('    #[test]', integrationStart + 12);
assert.ok(integrationStart >= 0 && integrationEnd > integrationStart);
const retriever = (source.slice(0, integrationStart) + source.slice(integrationEnd)).replaceAll(
  /include_str!\("\.\.\/\.\.\/(prompts\/[^\"]+)"\)/g,
  (_, file) => `include_str!(${JSON.stringify(abs(`src-tauri/${file}`))})`);
assert.ok(!retriever.includes('include_str!("../../'), 'all source-relative includes must resolve');
const followup = rustItem((await readFile('src-tauri/src/pipeline/message_context.rs', 'utf8')).replaceAll('\r\n', '\n'), 'pub fn is_short_followup');
// 语种门用生产 normalize_lang，不在测试里重写一份分发逻辑。
const normalize = rustItem((await readFile('src-tauri/src/pipeline/prompt_modules.rs', 'utf8')).replaceAll('\r\n', '\n'), 'pub fn normalize_lang');
const cosine = rustItem((await readFile('src-tauri/src/memory/vector_search.rs', 'utf8')).replaceAll('\r\n', '\n'), 'pub fn cosine_similarity');
const embeddingTrait = rustItem((await readFile('src-tauri/src/memory/embedding.rs', 'utf8')).replaceAll('\r\n', '\n'), 'pub trait MemoryEmbeddingProvider');
const dir = await mkdtemp(path.join(tmpdir(), 'vivian-few-shot-'));
try {
  await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="few-shot-library-contract"
version="0.0.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
parking_lot="0.12"
tracing="0.1"
sha2="0.10"
uuid={version="1",features=["v4"]}
once_cell="1"
tempfile="3"
[[bin]]
name="few-shot-library-contract"
path="main.rs"
`);
  await writeFile(path.join(dir, 'main.rs'), `
#![allow(dead_code)]
mod error { pub type VivianResult<T> = Result<T, std::io::Error>; }
mod types {pub mod response {#[derive(serde::Deserialize)] pub struct ChatMessage {pub role:String,pub content:String}}}
mod pipeline {pub mod message_context {${followup}}
 pub mod prompt_modules {${normalize}}}
mod utils {
 pub mod path {
  pub fn get_user_data_dir()->std::path::PathBuf {std::env::var_os("VIVIAN_TEST_DATA_DIR").unwrap().into()}
  pub fn get_character_data_dir(id:&str)->std::path::PathBuf {get_user_data_dir().join("characters").join(id)}
 }
 pub mod fs {pub fn write_atomic(p:&std::path::Path,s:&str)->std::io::Result<()> {if let Some(dir)=p.parent(){std::fs::create_dir_all(dir)?;}std::fs::write(p,s)}}
}
mod emotion {#[path=${JSON.stringify(abs('src-tauri/src/emotion/corpus_cache.rs'))}] pub mod corpus_cache;}
#[path=${JSON.stringify(abs('src-tauri/src/persona/embedding_snapshot.rs'))}] mod embedding_snapshot;
#[path=${JSON.stringify(abs('src-tauri/src/persona/example_library.rs'))}] mod example_library;
mod memory {
 pub mod vector_search {${cosine}}
 pub mod embedding {use crate::error::VivianResult;${embeddingTrait}
  pub fn default_embedding()->std::sync::Arc<dyn MemoryEmbeddingProvider> {std::sync::Arc::new(crate::Offline)}
 }
}
mod production {${retriever}}
struct Offline;
impl memory::embedding::MemoryEmbeddingProvider for Offline {
 fn dimension(&self)->usize {2}
 fn embed(&self,_:&str)->error::VivianResult<Vec<f32>> {panic!("offline retrieval must not call embedding")}
}
fn main() {}
#[cfg(test)] mod tests {
 use super::*;
 use std::sync::Arc;
 struct Identity {len:usize}
 impl memory::embedding::MemoryEmbeddingProvider for Identity {
  fn dimension(&self)->usize {self.len}
  fn is_remote(&self)->bool {true}
  fn embed(&self,_:&str)->error::VivianResult<Vec<f32>> {panic!("query vectors are supplied by the contract test")}
  fn embed_batch(&self,texts:&[String])->error::VivianResult<Vec<Vec<f32>>> {
   assert_eq!(texts.len(),self.len);
   Ok((0..self.len).map(|i|{let mut v=vec![0.0;self.len];v[i]=1.0;v}).collect())
  }
 }
 #[test] fn source_fragments_are_loaded_without_fabricating_offline_matches() {
  for name in ["vivian", "nana"] {
   let entries=example_library::factory_examples(name);
   let r=production::ExampleRetriever::new(name,Arc::new(Offline));
   assert!(entries.iter().any(|e|e.is_fragment()));
   for e in entries.iter().filter(|e|e.is_fragment()) {
    let out=r.retrieve(e.text_for("zh").unwrap(),&[],None,&[], "zh").unwrap_or_default();
    assert!(!out.contains("Expression fragment "),"a semantic-only fragment fired without embeddings: {name}/{}",e.id);
   }
  }
 }
 #[test] fn every_sourced_quote_shares_one_index_across_three_languages() {
  let mut probes=0;
  for name in ["vivian","nana"] {
   let entries=example_library::factory_examples(name);
   assert!(entries.iter().all(|e|e.is_approved_utterance() && e.is_fragment()));
   let r=production::ExampleRetriever::new(name,Arc::new(Identity{len:entries.len()}));
   r.preload();
   for (i,e) in entries.iter().enumerate() {
    let mut q=vec![0.0;entries.len()];q[i]=1.0;
    for lang in ["zh","ja","en"] {
    let out=r.retrieve("semantic probe",&[],Some(&q),&[],lang).unwrap();probes+=1;
    assert!(out.contains(&format!("Expression fragment {}",e.id)),"{name}/{}",e.id);
    assert!(!out.contains("Fictional user:") && out.chars().count()<=1800);
    assert!(out.lines().filter(|line|line.starts_with("Expression fragment ")).count() <= 6);
    assert_eq!(r.retrieve("semantic probe",&[],Some(&q),&[e.scope.clone()],lang).as_deref(),Some(out.as_str()));
    let marker=format!("Expression fragment {}\\nWording and rhythm only: ",e.id);
    let wording=out.split(&marker).nth(1).unwrap().lines().next().unwrap();
    let wording:serde_json::Value=serde_json::from_str(wording).unwrap();
    assert_eq!(wording.as_str(),e.text_for(lang),"wrong translation for {name}/{} in {lang}",e.id);
    for line in out.lines().filter_map(|l|l.strip_prefix("Wording and rhythm only: ")) {
     assert!(serde_json::from_str::<serde_json::Value>(line).unwrap().is_string(),"never inject the trilingual object");
    }
    }
   }
  }
  println!("Verified {probes} corpus indices with controlled identity vectors, not a real-model relevance benchmark");
 }

}
`);
  const run = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '--quiet', '--', '--nocapture'],
    {encoding:'utf8', timeout:180_000, maxBuffer:4 * 1024 * 1024,
      env:{...process.env, VIVIAN_TEST_DATA_DIR:path.join(dir, 'data')}});
  assert.equal(run.status, 0, run.stdout + run.stderr + (run.error?.message ?? ''));
  console.log(run.stdout.trim());
  console.log(`${fragments.flat().length} dataset-sourced fragments; zero authored dialogue samples. Approved corpus, persistence, language gates and retrieval verified.`);
} finally {
  assert.equal(path.dirname(path.resolve(dir)), path.resolve(tmpdir()), 'cleanup must stay inside the temporary directory');
  assert.ok(path.basename(dir).startsWith('vivian-few-shot-'));
  await rm(dir, {recursive:true, force:true});
}
