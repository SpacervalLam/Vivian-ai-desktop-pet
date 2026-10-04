import { readdir, readFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const flags = process.argv.slice(2);
const frontend = flags.includes('--frontend');
const live = flags.includes('--live');
const filter = flags.find(arg => !arg.startsWith('--')) ?? '';
const directories = ['tests', 'scripts', 'plugins/3d-apartment/tests'];
const files = [];
for (const directory of directories) {
  for (const name of await readdir(path.join(root, directory))) {
    if (!/\.test\.(mjs|ts)$/.test(name)) continue;
    const file = `${directory}/${name}`;
    if (!file.includes(filter)) continue;
    const source = await readFile(path.join(root, file), 'utf8');
    const requiresDesktop = source.includes('@test-environment live-desktop');
    if (requiresDesktop !== live) continue;
    if (frontend && /spawnSync\(['"](?:cargo|rustc)['"]/.test(source)) continue;
    files.push(file);
  }
}
files.sort();
if (!files.length) throw new Error(`No tests match ${JSON.stringify(filter)}`);
const failures = [];
for (const file of files) {
  console.log(`\n[test] ${file}`);
  const status = await new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [file], { cwd: root, stdio: 'inherit' });
    const timer = setTimeout(() => child.kill(), 10 * 60_000);
    child.once('error', error => { clearTimeout(timer); reject(error); });
    child.once('exit', (code, signal) => { clearTimeout(timer); resolve(code ?? signal); });
  });
  if (status !== 0) failures.push({ file, status });
}
console.log(`\n${files.length - failures.length}/${files.length} test scripts passed.`);
for (const failure of failures) console.error(`${failure.file}: ${failure.status}`);
process.exitCode = failures.length ? 1 : 0;
