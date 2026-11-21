// 隔离编译 production request_intent（含自带单测），不启桌面运行时。
// 覆盖：拒绝 / 假设句不升级为请求、混合子句仍按请求处理、词内否定字（"不错"）不误判。
import { mkdir, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const dir = path.join(root, 'tmp', 'request-intent-tests');
const pipeline = path.join(root, 'src-tauri', 'src', 'pipeline');
await mkdir(path.join(dir, 'src'), { recursive: true });
const rustPath = value => value.replaceAll('\\', '/');

await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-request-intent-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde_json="1"
`);

// 模块本身只依赖 serde_json；单测挂在模块内，直接全部跑起来。
await writeFile(path.join(dir, 'src', 'lib.rs'), `#![allow(dead_code)]
#[path="${rustPath(path.join(pipeline, 'request_intent.rs'))}"] pub mod request_intent;
`);

const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1', '--',
  '--test-threads=1'], {
  cwd: root, stdio: 'inherit',
  env: { ...process.env, CARGO_INCREMENTAL: '0', CARGO_PROFILE_DEV_DEBUG: '0' },
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
