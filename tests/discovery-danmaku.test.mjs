// Compile the production discovery/danmaku module (with its own unit tests) without the desktop
// runtime. Covers protobuf segment parsing (unknown fields, truncation, blank entries) and the
// minute-bucket digest that feeds the evaluation prompt.
import { mkdir, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const dir = path.join(root, 'tmp', 'discovery-danmaku-tests');
const danmaku = path.join(root, 'src-tauri', 'src', 'discovery', 'danmaku.rs');
// 可选的真实样本：本地拉过一段 seg.so 摆在这里，就会额外跑一条线上结构校验
const sample = path.join(root, 'tmp', '_seg.bin');
await mkdir(path.join(dir, 'src'), { recursive: true });
const rustPath = value => value.replaceAll('\\', '/');

await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-discovery-danmaku-tests"
version="0.1.0"
edition="2021"
[dependencies]
`);

await writeFile(path.join(dir, 'src', 'lib.rs'), `#![allow(dead_code)]
#[path="${rustPath(danmaku)}"] pub mod danmaku;
`);

const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1', '--',
  '--test-threads=1'], {
  cwd: root, stdio: 'inherit',
  env: {
    ...process.env,
    CARGO_INCREMENTAL: '0',
    CARGO_PROFILE_DEV_DEBUG: '0',
    ...(existsSync(sample) ? { DANMAKU_SEGMENT_FILE: sample } : {}),
  },
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
