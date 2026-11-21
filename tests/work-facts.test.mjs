import assert from 'node:assert/strict';
import { writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

// 事实清单是零业务边界模块：只依赖 serde / parking_lot 与两个文件工具，
// 整份生产文件挂进 mini-crate 即可连同自带的 #[cfg(test)] 单测一起跑。
const modulePath = path.resolve('src-tauri/src/brain/work_facts.rs').replaceAll('\\', '/');
const directory = await mkdtemp(path.join(tmpdir(), 'vivian-work-facts-'));
try {
  await writeFile(
    path.join(directory, 'lib.rs'),
    `#![allow(dead_code)]
#[path=${JSON.stringify(modulePath)}] pub mod work_facts;

pub mod utils {
    pub mod fs {
        // 单测只走内存存储（path=None），这里给出编译期桩即可。
        pub fn load_json_or_backup<T: serde::de::DeserializeOwned>(_: &std::path::Path) -> Option<T> { None }
        pub fn atomic_write(_: &std::path::Path, _: &[u8]) -> std::io::Result<()> { Ok(()) }
    }
    pub mod path {
        pub fn get_user_data_dir() -> std::path::PathBuf { std::env::temp_dir() }
    }
}
`,
  );
  await writeFile(path.join(directory, 'Cargo.toml'), `[package]
name="vivian-work-facts-fixture"
version="0.0.0"
edition="2021"
[lib]
path="lib.rs"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
parking_lot="0.12"
once_cell="1"
`);
  const result = spawnSync(
    'cargo',
    ['test', '--offline', '--quiet', '--manifest-path', path.join(directory, 'Cargo.toml'),
      '--target-dir', path.join(tmpdir(), 'vivian-work-facts-target')],
    { encoding: 'utf8', env: { ...process.env, CARGO_BUILD_JOBS: '2' } },
  );
  assert.equal(result.status, 0, result.stdout + result.stderr);
  console.log('Shared work facts (normalization, dedup, cap, revision, render):');
  console.log(result.stdout.trim());
} finally {
  await rm(directory, { recursive: true, force: true });
}
