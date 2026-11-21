// Exercise the production task-continuity module (and its own #[cfg(test)] suite)
// without a full-crate build: the module has zero application-boundary imports.
import { mkdir, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';

const root = process.cwd();
const dir = path.join(root, 'tmp', 'task-continuity-tests');
const production = path.join(root, 'src-tauri', 'src', 'pipeline', 'task_continuity.rs').replaceAll('\\', '/');

await mkdir(path.join(dir, 'src'), { recursive: true });
await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-task-continuity-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
`);
await writeFile(path.join(dir, 'src/lib.rs'), `#![allow(dead_code)]
#[path="${production}"] pub mod task_continuity;
`);

const result = spawnSync(
  'cargo',
  ['test', ...(process.env.VIVIAN_TEST_OFFLINE === '0' ? [] : ['--offline']),
    '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1'],
  { stdio: 'inherit', env: { ...process.env, CARGO_INCREMENTAL: '0', CARGO_PROFILE_DEV_DEBUG: '0' } },
);
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
