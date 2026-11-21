import assert from 'node:assert/strict';
import { writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

// 路由统计是零业务边界模块：记录累加、成本合并与建议门槛都可独立验证。
const modulePath = path.resolve('src-tauri/src/brain/work_route_stats.rs').replaceAll('\\', '/');
const directory = await mkdtemp(path.join(tmpdir(), 'vivian-route-stats-'));
try {
  await writeFile(
    path.join(directory, 'lib.rs'),
    `#![allow(dead_code)]
#[path=${JSON.stringify(modulePath)}] pub mod work_route_stats;

pub mod utils {
    pub mod fs {
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
name="vivian-route-stats-fixture"
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
      '--target-dir', path.join(tmpdir(), 'vivian-route-stats-target')],
    { encoding: 'utf8', env: { ...process.env, CARGO_BUILD_JOBS: '2' } },
  );
  assert.equal(result.status, 0, result.stdout + result.stderr);
  console.log('Work route stats (acceptance, cost join, suggestion gates):');
  console.log(result.stdout.trim());
} finally {
  await rm(directory, { recursive: true, force: true });
}
