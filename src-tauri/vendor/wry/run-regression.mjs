// Run the Windows regression without compiling upstream GPU examples.
import { readFileSync, writeFileSync, mkdirSync, copyFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

if (process.platform !== 'win32') {
  console.log('Windows WebView2 regression requires Windows.');
  process.exit(0);
}
const vendor = dirname(fileURLToPath(import.meta.url));
const root = resolve(vendor, '../../..');
const testDir = resolve(root, 'tmp/wry-teardown-test');
mkdirSync(testDir, { recursive: true });
const rustPath = path => path.replaceAll('\\', '/');
let manifest = readFileSync(resolve(vendor, 'Cargo.toml'), 'utf8')
  .replace(/^\[\[example\]\][\s\S]*?(?=^\[|(?![\s\S]))/gm, '')
  .replace(/^\[(?:dev-dependencies\.[^\]]+|target\.[^\]]+\.dev-dependencies\.[^\]]+)\][\s\S]*?(?=^\[|(?![\s\S]))/gm, '')
  .replace('    "tao/x11",', '')
  .replace('path = "src/lib.rs"', `path = "${rustPath(resolve(vendor, 'src/lib.rs'))}"`)
  .replace('build = "build.rs"', `build = "${rustPath(resolve(vendor, 'build.rs'))}"`);
manifest += '\n[dev-dependencies.base64]\nversion = "0.22"\n'
  + '[dev-dependencies.dom_query]\nversion = "0.27.0"\ndefault-features = false\n'
  + '[dev-dependencies.sha2]\nversion = "0.10"\n';
writeFileSync(resolve(testDir, 'Cargo.toml'), manifest);
copyFileSync(resolve(root, 'src-tauri/Cargo.lock'), resolve(testDir, 'Cargo.lock'));
const result = spawnSync('cargo', ['test', '--manifest-path', resolve(testDir, 'Cargo.toml'),
  '--lib', 'parent_subclass_tests', '--offline', '--target-dir', resolve(testDir, 'target'),
  '-j', '2'], { cwd: root, stdio: 'inherit' });
if (result.error) throw result.error;
process.exit(result.status ?? 1);
