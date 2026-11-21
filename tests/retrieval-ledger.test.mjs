// 隔离编译 production retrieval_ledger（含自带单测），不启桌面运行时。
// 覆盖：真实回执 → 来源台账（去重 / 单查询扁平结构 / 检索失败）、
// 实体命中判定（近似名公司不算命中）、表达阶段守卫（占位符替换、编造 URL 摘除、
// 历史链接不误杀）。
import { mkdir, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const dir = path.join(root, 'tmp', 'retrieval-ledger-tests');
const pipeline = path.join(root, 'src-tauri', 'src', 'pipeline');
await mkdir(path.join(dir, 'src'), { recursive: true });
const rustPath = value => value.replaceAll('\\', '/');

await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-retrieval-ledger-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde_json="1"
`);

// 模块本身只依赖 serde_json；单测挂在模块内，直接全部跑起来。
await writeFile(path.join(dir, 'src', 'lib.rs'), `#![allow(dead_code)]
#[path="${rustPath(path.join(pipeline, 'retrieval_ledger.rs'))}"] pub mod retrieval_ledger;
`);

const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1', '--',
  '--test-threads=1'], {
  cwd: root, stdio: 'inherit',
  env: { ...process.env, CARGO_INCREMENTAL: '0', CARGO_PROFILE_DEV_DEBUG: '0' },
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
