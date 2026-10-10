import assert from 'node:assert/strict';
import { readFile, mkdtemp, writeFile, access } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';

if (process.platform !== 'win32') {
  console.log('Optional resource installer: Windows integration skipped on this platform.');
} else {
  // Avoid requiring release artifacts for source-only test runs.
  let artifacts;
  try { artifacts = JSON.parse(await readFile('release/resource-packs.json', 'utf8')); }
  catch (error) { if (error.code !== 'ENOENT') throw error; }
  if (!artifacts) {
    console.log('Optional resource installer: run npm run build first to generate ZIPs.');
  } else {
    const root = await mkdtemp(resolve('tmp/resource-install test-'));
    const shell = join(process.env.SystemRoot, 'System32/WindowsPowerShell/v1.0/powershell.exe');
    const digest = bytes => createHash('sha256').update(bytes).digest('hex');
    function install(packagePath, id, hash) {
      return spawnSync(shell, ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File',
        resolve('src-tauri/windows/install-resources.ps1'), '-Package', resolve(packagePath), '-InstallRoot', root,
        '-PackId', id, '-ExpectedHash', hash, '-ManifestPath', resolve('src/optional-resources.json')], { encoding: 'utf8', windowsHide: true });
    }
    for (const pack of artifacts) {
      const result = install(`release/${pack.filename}`, pack.id, pack.sha256);
      assert.equal(result.status, 0, result.stderr);
      await access(join(root, 'optional', pack.id, 'pack.json'));
    }
    // Replacement, paths containing spaces, corrupt archives and ZIP traversal.
    const fonts = artifacts.find(pack => pack.id === 'fonts');
    const current = await readFile(join(root, 'optional/fonts/fonts/ma-shan-zheng.woff2'));
    assert.equal(install(`release/${fonts.filename}`, fonts.id, fonts.sha256).status, 0);
    assert.notEqual(install(`release/${fonts.filename}`, fonts.id, '0'.repeat(64)).status, 0);
    const malicious = join(root, 'malicious.zip');
    const zip = spawnSync('python', ['-c',
      'import sys\nfrom zipfile import ZipFile\nwith ZipFile(sys.argv[1],"w") as z:\n z.writestr("../outside.txt","escape")\n z.writestr("pack.json","{}")\n', malicious], { encoding: 'utf8' });
    assert.equal(zip.status, 0, zip.stderr);
    assert.notEqual(install(malicious, fonts.id, digest(await readFile(malicious))).status, 0);
    await assert.rejects(access(join(root, 'outside.txt')), { code: 'ENOENT' });
    assert.deepEqual(await readFile(join(root, 'optional/fonts/fonts/ma-shan-zheng.woff2')), current);
    const invalid = join(root, 'invalid-font.zip');
    const altered = spawnSync('python', ['-c',
      'import sys,json\nfrom zipfile import ZipFile\nwith ZipFile(sys.argv[1],"w") as z:\n z.writestr("pack.json",json.dumps({"id":"fonts","version":"1.0.0","files":{"fonts/ma-shan-zheng.woff2":"0"*64}}))\n z.writestr("fonts/ma-shan-zheng.woff2","bad")\n', invalid], { encoding: 'utf8' });
    assert.equal(altered.status, 0, altered.stderr);
    assert.notEqual(install(invalid, fonts.id, digest(await readFile(invalid))).status, 0);
    assert.deepEqual(await readFile(join(root, 'optional/fonts/fonts/ma-shan-zheng.woff2')), current);
    console.log('Optional resource installer: both packs, replacement, space paths, archive/file checksums, traversal rejection and previous-install preservation passed.');
  }
}
