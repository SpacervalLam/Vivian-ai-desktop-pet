import assert from 'node:assert/strict';
import { writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

// 状态机是零业务边界模块，可以直接把生产文件整个挂进 mini-crate，
// 连它自带的 #[cfg(test)] 单测一起跑，不需要桩任何应用边界。
const modulePath = path.resolve('src-tauri/src/brain/work_task_status.rs').replaceAll('\\', '/');
const directory = await mkdtemp(path.join(tmpdir(), 'vivian-work-task-status-'));
try {
  await writeFile(
    path.join(directory, 'lib.rs'),
    `#![allow(dead_code)]\n#[path=${JSON.stringify(modulePath)}] pub mod work_task_status;\n`,
  );
  await writeFile(path.join(directory, 'Cargo.toml'), `[package]
name="vivian-work-task-status-fixture"
version="0.0.0"
edition="2021"
[lib]
path="lib.rs"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
`);
  const result = spawnSync(
    'cargo',
    ['test', '--offline', '--quiet', '--manifest-path', path.join(directory, 'Cargo.toml'),
      '--target-dir', path.join(tmpdir(), 'vivian-wts-target')],
    { encoding: 'utf8', env: { ...process.env, CARGO_BUILD_JOBS: '2' } },
  );
  assert.equal(result.status, 0, result.stdout + result.stderr);
  console.log('Work task state machine (transition table, terminal states, review gate):');
  console.log(result.stdout.trim());
} finally {
  await rm(directory, { recursive: true, force: true });
}
