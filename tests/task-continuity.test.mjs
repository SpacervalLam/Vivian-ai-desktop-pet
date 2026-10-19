import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

// Compile the actual state machine without linking the desktop application.
const directory = await mkdtemp(path.join(tmpdir(), 'vivian-task-continuity-'));
try {
  const source = path.resolve('src-tauri/src/pipeline/task_continuity.rs').replaceAll('\\', '/');
  await writeFile(path.join(directory, 'lib.rs'), `#[path=${JSON.stringify(source)}] mod task_continuity;`);
  await writeFile(path.join(directory, 'Cargo.toml'), `[package]
name="task-continuity-contract"
version="0.0.0"
edition="2021"
[lib]
path="lib.rs"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
`);
  const result = spawnSync('cargo', ['test', '--offline', '--quiet', '--manifest-path', path.join(directory, 'Cargo.toml'),
    '--target-dir', path.resolve('tmp/task-continuity-contract-target')], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  console.log(result.stdout.trim());
} finally {
  await rm(directory, { recursive: true, force: true });
}
