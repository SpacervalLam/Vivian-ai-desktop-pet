import { spawn } from 'node:child_process';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const data = await mkdtemp(path.join(tmpdir(), 'vivian-rust-tests-'));
const args = ['test', '--locked', '--manifest-path', 'src-tauri/Cargo.toml', '--lib', '-j', '1'];
if (process.argv.includes('--windows-integration')) args.push('utils::sandbox::tests', '--', '--ignored', '--test-threads=1');
else args.push('--', '--test-threads=1');
console.log(`Isolated Rust test data: ${data}`);
const child = spawn('cargo', args, { cwd: root, stdio: 'inherit', env: { ...process.env, VIVIAN_TEST_DATA_DIR: data } });
child.once('error', error => { console.error(error); process.exitCode = 1; });
child.once('exit', code => { process.exitCode = code ?? 1; });
