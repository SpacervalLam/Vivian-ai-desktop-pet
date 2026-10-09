// Compile the production notebook types, renderer and storage without the desktop runtime.
import { mkdir, readFile, writeFile, mkdtemp } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const dir = path.join(root, 'tmp', 'notebook-template-tests');
const notebook = path.join(root, 'src-tauri', 'src', 'notebook');
await mkdir(path.join(dir, 'src'), { recursive: true });
const dataDir = await mkdtemp(path.join(tmpdir(), 'vivian-notebook-tests-'));
const rustPath = value => value.replaceAll('\\', '/');
await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-notebook-template-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
chrono="0.4"
once_cell="1"
regex="1"
tracing="0.1"
`);
let types = await readFile(path.join(notebook, 'mod.rs'), 'utf8');
// Replace only module locations; use the complete production type definitions.
for (const name of ['collected', 'css_guide', 'doc_style', 'renderer', 'storage']) {
  types = types.replace(`pub mod ${name};`, `#[path="${rustPath(path.join(notebook, name + '.rs'))}"] pub mod ${name};`);
}
types = types.replace('pub mod persona_brief;', '');
await writeFile(path.join(dir, 'src', 'notebook.rs'), types);
await writeFile(path.join(dir, 'src', 'lib.rs'), `#![allow(dead_code)]
pub mod utils { pub mod path {
    pub fn get_character_data_dir(id: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(std::env::var("VIVIAN_TEST_DATA_DIR").unwrap()).join(id)
    }
    pub fn ensure_dir(dir: &std::path::Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())
    }
}}
// Only the knowledge ingestion boundary is stubbed; it is outside template rendering.
pub mod memory {
    pub struct MemoryManager;
    pub struct Document { pub id: String }
    impl MemoryManager {
        pub async fn add_knowledge_document(&self, _: &str, _: &str, _: Vec<String>, _: &str, _: Option<i64>) -> Result<Document, String> {
            Ok(Document { id: "test-memory".into() })
        }
    }
}
pub mod notebook;
#[cfg(test)] #[path="${rustPath(path.join(root, 'tests', 'fixtures', 'notebook-templates.rs'))}"] mod regression;
`);
const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1', '--', '--test-threads=1'], {
  cwd: root, stdio: 'inherit',
  env: { ...process.env, CARGO_INCREMENTAL: '0', VIVIAN_TEST_DATA_DIR: dataDir },
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
