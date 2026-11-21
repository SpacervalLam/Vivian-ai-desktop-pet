import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const directory = await mkdtemp(path.join(tmpdir(), 'vivian-spoken-turn-'));
try {
  await writeFile(path.join(directory, 'Cargo.toml'), `[package]
name = "spoken-turn-harness"
version = "0.0.0"
edition = "2021"
[lib]
path = "lib.rs"
[dependencies]
tokio = { version = "1", features = ["rt", "macros", "sync"] }
futures = "0.3"
serde_json = "1"
`);
  const source = JSON.stringify(path.resolve('src-tauri/src/spoken_turn.rs').replaceAll('\\', '/'));
  await writeFile(path.join(directory, 'lib.rs'), `#[path=${source}] pub mod spoken_turn;`);
  const result = spawnSync('cargo', ['test', '--offline', '--quiet', '--manifest-path', path.join(directory, 'Cargo.toml'),
    '--target-dir', path.resolve('tmp/spoken-turn-target')], {
    encoding: 'utf8', env: { ...process.env, CARGO_PROFILE_DEV_DEBUG: '0', CARGO_BUILD_JOBS: '2' },
  });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  console.log('Public speech: voluntary replies, all-silent fallback, single listener, empty room and shared utterance identity passed.');
} finally {
  await rm(directory, { recursive: true, force: true });
}
