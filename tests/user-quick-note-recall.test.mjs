import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
const source = await readFile('src-tauri/src/memory/kinds.rs', 'utf8');
const recall = source.match(/pub fn recallable\([\s\S]*?\n\}/)?.[0];
assert.ok(recall);
const dir = await mkdtemp(path.join(tmpdir(), 'vivian-user-note-recall-'));
try {
  const input = path.join(dir, 'recall.rs'), exe = path.join(dir, 'recall.exe');
  await writeFile(input, `
#![allow(dead_code)]
use std::collections::HashMap;
#[derive(Clone,Copy)] enum RecordKind { Fact, SessionSummary, Dialogue, Reference, Internal }
struct MemoryItem { content:String, tags:Vec<String>, consolidated:bool, metadata:HashMap<&'static str,bool>, kind:RecordKind }
fn kind(item:&MemoryItem)->RecordKind {item.kind}
${recall}
fn main() {
  for kind in [RecordKind::Fact,RecordKind::Reference,RecordKind::SessionSummary,RecordKind::Dialogue] {
    let mut item=MemoryItem{content:"用户的灵感".into(),tags:vec!["notebook".into()],consolidated:false,metadata:HashMap::from([("index_active",true)]),kind};
    assert!(recallable(&item),"normal role memory remains available");
    item.tags.push("quick_note".into());
    assert!(!recallable(&item),"legacy user notes must never re-enter character memory recall");
  }
}
`);
  const compile = spawnSync('rustc', ['--edition=2021', input, '-o', exe], { encoding:'utf8' });
  assert.equal(compile.status, 0, compile.stderr || String(compile.error));
  const run = spawnSync(exe, [], { encoding:'utf8' });
  assert.equal(run.status, 0, run.stderr);
  console.log('User note recall: legacy quick notes excluded from all character recall kinds; ordinary memories preserved');
} finally { await rm(dir, { recursive:true, force:true }); }
