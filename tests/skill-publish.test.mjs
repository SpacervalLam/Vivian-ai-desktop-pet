import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

// Compile the real publish + evaluation modules in a small isolated crate.
// The repo-wide lib test harness is not usable as a gate, so the modules' own
// #[cfg(test)] cases are executed here without reimplementing any rule.
// (selection.rs replays the real BM25 index and needs the full crate; it is
//  compile-checked by cargo check --lib --tests and tested in its own module.)
const root = process.cwd();
const abs = file => JSON.stringify(path.resolve(root, file).replaceAll('\\', '/'));
const dir = await mkdtemp(path.join(tmpdir(), 'vivian-skill-publish-'));
try {
  await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="skill-publish-contract"
version="0.0.0"
edition="2021"
[dependencies]
serde_json="1"
sha2="0.10"
tempfile="3"
[[bin]]
name="skill-publish-contract"
path="main.rs"
`);
  await writeFile(path.join(dir, 'main.rs'), `
#![allow(dead_code)]
#[path=${abs('src-tauri/src/skills/publish.rs')}] pub mod publish;
#[path=${abs('src-tauri/src/skills/evaluation.rs')}] pub mod evaluation;
fn main() {}
`);
  const run = spawnSync('cargo', [
    'test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'),
    '-j', '1', '--', '--test-threads=1',
  ], {
    cwd: root,
    encoding: 'utf8',
    timeout: 240_000,
    maxBuffer: 8 * 1024 * 1024,
    env: { ...process.env, CARGO_TARGET_DIR: path.join(root, 'tmp/skill-publish-target') },
  });
  assert.equal(run.status, 0, run.stdout + run.stderr + (run.error?.message ?? ''));
  // Guard against a silently empty / partially-collected test harness.
  assert.match(run.stdout, /publish::tests::/);
  assert.match(run.stdout, /evaluation::tests::/);
  for (const critical of [
    'restore_target_pins_the_real_current_file',
    'pinned_digest_blocks_session_local_tampering',
    'staged_candidate_keeps_layout_and_resources',
  ]) {
    assert.ok(run.stdout.includes(critical), `关键回归用例没有跑：${critical}`);
  }
  const passed = [...run.stdout.matchAll(/test result: ok\. (\d+) passed/g)]
    .reduce((sum, m) => sum + Number(m[1]), 0);
  assert.ok(passed >= 15, `expected publish(8) + evaluation(7) tests, got ${passed}`);
  console.log(run.stdout.trim());
} finally {
  assert.equal(path.dirname(path.resolve(dir)), path.resolve(tmpdir()));
  assert.ok(path.basename(dir).startsWith('vivian-skill-publish-'));
  await rm(dir, { recursive: true, force: true });
}
