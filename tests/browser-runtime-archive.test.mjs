// The published runtime pays for its size with an LZMA payload: prove the archive still
// yields a Node that matches the manifest hash and loads both entrypoints.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm, writeFile, access } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const packs = JSON.parse(await readFile('src/optional-resources.json', 'utf8'));
const manifest = packs.find(pack => pack.id === 'browser-runtime');
const archive = `release/${manifest.download.filename}`;
if (!(await access(archive).then(() => true, () => false))) {
  console.log('[browser-runtime] published archive absent; run npm run package:browser-runtime first');
} else {
  const declared = JSON.parse(await readFile('src-tauri/browser-runtime/pack.json', 'utf8'));
  const unpacked = await mkdtemp(join(tmpdir(), 'vivian-runtime-'));
  const python = (code, ...args) => spawnSync('python', ['-c', code, ...args], { encoding: 'utf8' });
  try {
    assert.equal(python('import sys\nfrom zipfile import ZipFile\nZipFile(sys.argv[1]).extractall(sys.argv[2])\n', archive, unpacked).status, 0);
    const listed = python('import sys,json\nfrom zipfile import ZipFile\nprint(json.dumps(ZipFile(sys.argv[1]).namelist()))\n', archive);
    assert.equal(listed.status, 0, listed.stderr);
    const entries = JSON.parse(listed.stdout);
    assert(entries.includes('node.exe.lz'), 'the binary ships compressed');
    assert(!entries.includes('node.exe'), 'the archive must not carry the inflated binary');
    assert.equal(declared.payloads?.['node.exe'], 'node.exe.lz', 'manifest records the payload mapping');

    const restored = python('import sys,lzma\nfrom pathlib import Path\nsrc=Path(sys.argv[1])\nsrc.with_suffix("").write_bytes(lzma.decompress(src.read_bytes(),format=lzma.FORMAT_ALONE))\nsrc.unlink()\n', join(unpacked, 'node.exe.lz'));
    assert.equal(restored.status, 0, restored.stderr);
    const binary = await readFile(join(unpacked, 'node.exe'));
    assert.equal(createHash('sha256').update(binary).digest('hex'), declared.files['node.exe'], 'restored binary keeps the declared hash');

    const probe = join(unpacked, 'probe.mjs');
    await writeFile(probe, "import { chromium } from 'playwright-core';\nimport { expect } from '@playwright/test';\nif (typeof chromium.connectOverCDP !== 'function') throw new Error('chromium entrypoint unavailable');\nexpect(1).toBe(1);\nconsole.log('runtime-ok');\n");
    const run = spawnSync(join(unpacked, 'node.exe'), [probe], { cwd: unpacked, encoding: 'utf8', timeout: 60000 });
    assert.equal(run.status, 0, run.stderr || run.stdout);
    assert.match(run.stdout, /runtime-ok/, 'trimmed runtime loads both entrypoints');
    console.log(`[browser-runtime] ${entries.length} entries restore to a runnable Node (${(binary.length / 1048576).toFixed(1)} MiB)`);
  } finally {
    await rm(unpacked, { recursive: true, force: true });
  }
}
