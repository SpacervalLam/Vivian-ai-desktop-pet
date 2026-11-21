import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

// The stage table and the settlement contract it reads are pure Rust with no
// Tauri dependency, so they can be linked into a two-module crate directly.
// Keeping this harness separate means a regression here never needs the full
// desktop crate, which does not build within this machine's memory budget.
const modulePath = name => JSON.stringify(path.resolve(`src-tauri/src/brain/${name}.rs`).replaceAll('\\', '/'));
const directory = await mkdtemp(path.join(tmpdir(), 'vivian-work-stage-'));
try {
  await writeFile(path.join(directory, 'Cargo.toml'), `[package]
name = "work-stage-harness"
version = "0.0.0"
edition = "2021"

[lib]
path = "lib.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
`);
  await writeFile(path.join(directory, 'lib.rs'), `#![allow(dead_code, unused_imports)]
pub mod brain {
  #[path=${modulePath('work_contract')}] pub mod work_contract;
  #[path=${modulePath('work_stage')}] pub mod work_stage;
}
`);
  const result = spawnSync('cargo', ['test', '--offline', '--quiet',
    '--manifest-path', path.join(directory, 'Cargo.toml'),
    '--target-dir', path.resolve('tmp/work-stage-target')], {
    encoding: 'utf8',
    env: { ...process.env, CARGO_PROFILE_DEV_DEBUG: '0', CARGO_BUILD_JOBS: '2' },
  });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  console.log('work stage table and settlement contract: ok');
  console.log(result.stdout.trim());
} finally {
  await rm(directory, { recursive: true, force: true });
}
