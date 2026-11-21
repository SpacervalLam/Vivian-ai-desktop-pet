// 隔离编译 production platform_search_policy（含自带单测），不启桌面运行时。
// 覆盖：默认平台集合、条数夹取、空/超长 query 拒绝、登录态平台指向浏览器通道、
// 未知平台与重复项处理。网络适配器不在此处验证（需要真实平台与网络）。
import { mkdir, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const dir = path.join(root, 'tmp', 'platform-search-policy-tests');
const builtin = path.join(root, 'src-tauri', 'src', 'tools', 'builtin');
await mkdir(path.join(dir, 'src'), { recursive: true });
const rustPath = value => value.replaceAll('\\', '/');

await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-platform-search-policy-tests"
version="0.1.0"
edition="2021"
[dependencies]
`);

await writeFile(path.join(dir, 'src', 'lib.rs'), `#![allow(dead_code)]
#[path="${rustPath(path.join(builtin, 'platform_search_policy.rs'))}"] pub mod platform_search_policy;
`);

const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1', '--',
  '--test-threads=1'], {
  cwd: root, stdio: 'inherit',
  env: { ...process.env, CARGO_INCREMENTAL: '0', CARGO_PROFILE_DEV_DEBUG: '0' },
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
