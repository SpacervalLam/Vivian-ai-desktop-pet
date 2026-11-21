import assert from 'node:assert/strict';
import { writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

// 原文检索的匹配 / 分页 / 压缩标记是零依赖纯逻辑，整份生产文件挂进 mini-crate 即可跑。
const modulePath = path.resolve('src-tauri/src/brain/history_search.rs').replaceAll('\\', '/');
const directory = await mkdtemp(path.join(tmpdir(), 'vivian-history-search-'));
try {
  await writeFile(
    path.join(directory, 'lib.rs'),
    `#![allow(dead_code)]\n#[path=${JSON.stringify(modulePath)}] pub mod history_search;\n`,
  );
  await writeFile(path.join(directory, 'Cargo.toml'), `[package]
name="vivian-history-search-fixture"
version="0.0.0"
edition="2021"
[lib]
path="lib.rs"
`);
  const result = spawnSync(
    'cargo',
    ['test', '--offline', '--quiet', '--manifest-path', path.join(directory, 'Cargo.toml'),
      '--target-dir', path.join(tmpdir(), 'vivian-history-search-target')],
    { encoding: 'utf8', env: { ...process.env, CARGO_BUILD_JOBS: '2' } },
  );
  assert.equal(result.status, 0, result.stdout + result.stderr);
  console.log('Original history search (compaction marker, tool arguments, paging):');
  console.log(result.stdout.trim());
} finally {
  await rm(directory, { recursive: true, force: true });
}
