import { spawnSync } from 'node:child_process';
import { readFile, writeFile, readdir, stat, mkdir } from 'node:fs/promises';
import { join } from 'node:path';

const result = spawnSync('cargo', ['metadata', '--manifest-path', 'src-tauri/Cargo.toml', '--format-version', '1',
  '--filter-platform', 'x86_64-pc-windows-msvc', '--locked', '--offline'], { encoding: 'utf8', maxBuffer: 20 * 1024 * 1024 });
if (result.status !== 0) throw new Error(result.stderr);
const metadata = JSON.parse(result.stdout), artifacts = await readdir('src-tauri/target/release/deps');
const names = ['boa_engine', 'jieba-rs', 'tiktoken-rs', 'pdf-extract', 'rodio', 'sysinfo', 'image', 'rustls', 'reqwest', 'tokio'];
const audit = [];
for (const name of names) {
  for (const pack of metadata.packages.filter(pack => pack.name === name)) {
    const files = [];
    for (const filename of artifacts.filter(file => file.startsWith(`lib${name.replaceAll('-', '_')}-`) && file.endsWith('.rlib'))) {
      const info = await stat(join('src-tauri/target/release/deps', filename));
      files.push({ filename, bytes: info.size, modified: info.mtimeMs });
    }
    files.sort((a, b) => b.modified - a.modified);
    audit.push({ name, version: pack.version, defaults: pack.features.default ?? [],
      enabled: metadata.resolve.nodes.find(node => node.id === pack.id)?.features ?? [],
      latestLibraryArtifact: files[0] ?? null });
  }
}
await mkdir('tmp', { recursive: true });
await writeFile('tmp/rust-dependency-audit.json', JSON.stringify({
  note: 'rlib sizes contain metadata/bitcode and are NOT contributions to the linked executable. Enabled features come from the resolved Windows dependency graph.',
  dependencies: audit,
}, null, 2) + '\n');
for (const row of audit) console.log(`${row.name} ${row.version}: ${row.enabled.join(', ')}`);
