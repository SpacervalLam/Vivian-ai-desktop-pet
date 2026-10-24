import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const root = process.cwd();
const fixtureFile = 'tests/fixtures/few-shot-retrieval.json';
const fixture = JSON.parse(await readFile(fixtureFile, 'utf8'));
const libraries = await Promise.all(['vivian', 'nana'].map(async name =>
  JSON.parse(await readFile(`src-tauri/prompts/characters/${name}/example_library.json`, 'utf8'))));
const newEntries = libraries.map(entries => entries.filter(e => e.id.startsWith('style-')));
assert.ok(fixture.cases.length >= 120, 'retain broad authored coverage');
assert.equal(new Set(fixture.cases.map(c => c.id)).size, fixture.cases.length);
assert.deepEqual(newEntries[0].map(e => e.id), newEntries[1].map(e => e.id));
for (const [index, entries] of libraries.entries()) {
  assert.equal(new Set(entries.map(e => e.id)).size, entries.length);
  const authored = newEntries[index];
  assert.equal(authored.length, fixture.cases.length);
  assert.equal(new Set(authored.map(e => e.response.text)).size, authored.length, 'do not pad the corpus with duplicate lines');
  for (const e of authored) {
    assert.match(e.situation, /^虚构示例：/);
    assert.ok(e.attention.trim() && e.user.trim());
    assert.ok(e.cues.length >= 2 && e.cues.every(c => [...c].length >= 2));
    assert.ok(e.cues.some(c => e.user.toLowerCase().includes(c.toLowerCase())), `${e.id}: canonical input needs a usable cue`);
    assert.ok(e.response.text.trim() && [...e.response.text].length <= 120, `${e.id}: bounded complete speech`);
    assert.ok(['reply', 'short_reply'].includes(e.response.intent));
    assert.ok(fixture.cases.some(c => c.id === e.id));
  }
  assert.ok(authored.filter(e => e.id.startsWith('style-en-')).length >= 12);
  assert.ok(authored.filter(e => e.id.startsWith('style-ja-')).length >= 12);
}
for (let i = 0; i < newEntries[0].length; i++) {
  const [v, n] = newEntries.map(entries => entries[i]);
  for (const key of ['id', 'scope', 'user', 'cues', 'context_cues']) assert.deepEqual(v[key], n[key]);
  assert.notEqual(v.response.text, n.response.text, `${v.id}: write each character's perspective separately`);
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
const integrationStart = source.indexOf('    #[test]\n    fn modern_net_examples_reach_companion_prompt_without_remote_embeddings');
const integrationEnd = source.indexOf('    #[test]', integrationStart + 12);
assert.ok(integrationStart >= 0 && integrationEnd > integrationStart);
const retriever = (source.slice(0, integrationStart) + source.slice(integrationEnd)).replaceAll(
  /include_str!\("\.\.\/\.\.\/(prompts\/[^\"]+)"\)/g,
  (_, file) => `include_str!(${JSON.stringify(abs(`src-tauri/${file}`))})`);
assert.ok(!retriever.includes('include_str!("../../'), 'all source-relative includes must resolve');
const followup = rustItem((await readFile('src-tauri/src/pipeline/message_context.rs', 'utf8')).replaceAll('\r\n', '\n'), 'pub fn is_short_followup');
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
[[bin]]
name="few-shot-library-contract"
path="main.rs"
`);
  await writeFile(path.join(dir, 'main.rs'), `
#![allow(dead_code)]
mod error { pub type VivianResult<T> = Result<T, std::io::Error>; }
mod types {pub mod response {#[derive(serde::Deserialize)] pub struct ChatMessage {pub role:String,pub content:String}}}
mod pipeline {pub mod message_context {${followup}}}
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
 #[derive(serde::Deserialize)] struct Case {id:String,input:String,history:Vec<types::response::ChatMessage>,paraphrases:Vec<String>,gated:bool}
 #[derive(serde::Deserialize)] struct Fixture {cases:Vec<Case>}
 fn cases()->Vec<Case> {serde_json::from_str::<Fixture>(include_str!(${JSON.stringify(abs(fixtureFile))})).unwrap().cases}
 #[test] fn original_modern_corpus_remains_retrievable() {
  for (name,source) in [
   ("vivian",include_str!(${JSON.stringify(abs('src-tauri/prompts/characters/vivian/example_library.json'))})),
   ("nana",include_str!(${JSON.stringify(abs('src-tauri/prompts/characters/nana/example_library.json'))}))
  ] {
   let entries:serde_json::Value=serde_json::from_str(source).unwrap();
   let r=production::ExampleRetriever::new(name,Arc::new(Offline));
   for e in entries.as_array().unwrap().iter().filter(|e|e["id"].as_str().unwrap().starts_with("net-modern-")) {
    for input in std::iter::once(e["user"].as_str().unwrap()).chain(e["cues"].as_array().unwrap().iter().map(|c|c.as_str().unwrap())) {
     let out=r.retrieve(input,&[],None,&[]).unwrap_or_default();
     assert!(out.contains(&format!("Example {}\\n",e["id"].as_str().unwrap())),"{name}: {input}");
    }
   }
  }
 }
 #[test] fn authored_scenarios_and_alternate_cues_reach_actual_retriever() {
  let mut failures=Vec::new(); let mut count=0;
  for name in ["vivian","nana"] {
   let r=production::ExampleRetriever::new(name,Arc::new(Offline));
   for c in cases() {for input in std::iter::once(&c.input).chain(c.paraphrases.iter()) {
    let out=r.retrieve(input,&c.history,None,&[]).unwrap_or_default();count+=1;
    if !out.contains(&format!("Example {}\\n",c.id)) {failures.push(format!("{name}/{}: {input} -> {out}",c.id));}
    assert!(out.chars().count()<=1800);
    assert!(out.lines().filter(|l|l.starts_with("Example response:")).count()<=3);
   }}
  }
  assert!(failures.is_empty(),"{} retrieval failures:\\n{}",failures.len(),failures.join("\\n"));
  println!("Verified {count} positive retrieval probes across both characters");
 }
 #[test] fn context_dependent_examples_require_the_right_recent_scene() {
  for name in ["vivian","nana"] {let r=production::ExampleRetriever::new(name,Arc::new(Offline));
   for c in cases().into_iter().filter(|c|c.gated) {
    for history in [vec![],vec![types::response::ChatMessage{role:"assistant".into(),content:"我们正在谈论一片海。".into()}]] {
     let out=r.retrieve(&c.input,&history,None,&[]).unwrap_or_default();
     assert!(!out.contains(&format!("Example {}\\n",c.id)),"{} leaked into an unrelated scene",c.id);
    }
   }
  }
 }
 #[test] fn learned_scopes_still_replace_factory_samples() {
  let library:serde_json::Value=serde_json::from_str(include_str!(${JSON.stringify(abs('src-tauri/prompts/characters/vivian/example_library.json'))})).unwrap();
  for name in ["vivian","nana"] {let r=production::ExampleRetriever::new(name,Arc::new(Offline));
   for c in cases() {
    let e=library.as_array().unwrap().iter().find(|e|e["id"].as_str()==Some(&c.id)).unwrap();
    let scope=e["scope"].as_str().unwrap().to_owned();
    let out=r.retrieve(&c.input,&c.history,None,&[scope]).unwrap_or_default();
    assert!(!out.contains(&format!("Example {}\\n",c.id)));
   }
  }
 }
}
`);
  const run = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '--quiet', '--', '--nocapture'],
    {encoding:'utf8', timeout:180_000, maxBuffer:4 * 1024 * 1024,
      env:{...process.env, VIVIAN_TEST_DATA_DIR:path.join(dir, 'data')}});
  assert.equal(run.status, 0, run.stdout + run.stderr + (run.error?.message ?? ''));
  console.log(run.stdout.trim());
  console.log(`${newEntries[0].length} authored pairs: schema, distinct voices, multilingual coverage, context gates and production retrieval verified.`);
} finally {
  assert.equal(path.dirname(path.resolve(dir)), path.resolve(tmpdir()), 'cleanup must stay inside the temporary directory');
  assert.ok(path.basename(dir).startsWith('vivian-few-shot-'));
  await rm(dir, {recursive:true, force:true});
}
