// Compile the actual icon resolver with one consistent Tokio dependency graph.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

const root = process.cwd();
const directory = await mkdtemp(path.join(tmpdir(), 'vivian-icon-contract-'));
try {
  const source = path.join(root, 'src-tauri/src/commands/shortcut_icons.rs').replaceAll('\\', '/');
  const manifest = path.join(directory, 'Cargo.toml');
  await writeFile(path.join(directory, 'Cargo.lock'), await readFile(path.join(root, 'src-tauri/Cargo.lock')));
  await writeFile(manifest, `[package]
name="vivian-icon-contract"
version="0.0.0"
edition="2021"
[lib]
path=${JSON.stringify(source)}
[dependencies]
base64="0.22"
futures="0.3"
reqwest={version="0.12",default-features=false,features=["json","stream","rustls-tls","http2","multipart"]}
scraper="0.27"
tokio={version="1",features=["macros","rt-multi-thread","net","io-util","time","sync","fs","process"]}
tracing="0.1"
png="0.18"
[target.'cfg(windows)'.dependencies]
windows={version="0.61",features=["Win32_Graphics_Gdi","Win32_Storage_FileSystem","Win32_UI_Shell","Win32_UI_WindowsAndMessaging"]}
[profile.dev]
debug="line-tables-only"
incremental=false
codegen-units=16
`);
  const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', manifest,
    '--target-dir', path.join(root, 'src-tauri/target'), '-j', '1', '--', '--test-threads=1'], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stdout + result.stderr || String(result.error));
  console.log(result.stdout.trim());
} finally {
  await rm(directory, { recursive: true, force: true });
}
