import { readFile, writeFile, mkdir, copyFile, unlink, stat } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { resolve, dirname, join } from 'node:path';

const packs = JSON.parse(await readFile('src/optional-resources.json', 'utf8'));
const fromStaging = process.argv.includes('--from-staging');
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
await mkdir('release', { recursive: true });
const artifacts = [], definitions = [];
for (const pack of packs) {
  const staging = resolve('tmp/optional-resources', pack.id);
  await mkdir(staging, { recursive: true });
  const files = {};
  for (const path of pack.files) {
    const source = join('dist', path), destination = join(staging, path);
    files[path] = sha256(await readFile(fromStaging ? destination : source));
    await mkdir(dirname(destination), { recursive: true });
    if (!fromStaging) await copyFile(source, destination);
  }
  if (fromStaging) {
    const previous = JSON.parse(await readFile(join(staging, 'pack.json'), 'utf8'));
    if (previous.id !== pack.id || previous.version !== pack.version || JSON.stringify(previous.files) !== JSON.stringify(files)) throw new Error(`Staging checksum mismatch: ${pack.id}`);
  }
  await writeFile(join(staging, 'pack.json'), JSON.stringify({ id: pack.id, version: pack.version, files }, null, 2));
  const filename = `Vivian-${pack.id}-${pack.version}.zip`;
  // Explicit entries keep stale staging files and artwork masters out of archives.
  const result = spawnSync('python', ['-c',
    'import sys,json\nfrom pathlib import Path\nfrom zipfile import ZipFile,ZipInfo,ZIP_DEFLATED\nroot=Path(sys.argv[1])\nwith ZipFile(sys.argv[2],"w",ZIP_DEFLATED,compresslevel=9) as z:\n for name in ["pack.json",*json.loads(sys.argv[3])]:\n  info=ZipInfo(name,date_time=(2026,1,1,0,0,0))\n  info.external_attr=0o644<<16\n  z.writestr(info,(root/name).read_bytes(),compress_type=ZIP_DEFLATED,compresslevel=9)\n',
    staging, resolve('release', filename), JSON.stringify(pack.files)], { stdio: 'inherit' });
  if (result.status !== 0) throw new Error(`Could not package ${pack.id}`);
  const hash = sha256(await readFile(join('release', filename)));
  artifacts.push({ id: pack.id, label: pack.label, version: pack.version, filename,
    bytes: (await stat(join('release', filename))).size, sha256: hash });
  definitions.push(`!define ${pack.id.toUpperCase()}_PACKAGE "${filename}"\n!define ${pack.id.toUpperCase()}_SHA256 "${hash}"`);
}
await writeFile('src-tauri/windows/resource-packages.nsh', definitions.join('\n') + '\n');
await writeFile('release/resource-packs.json', JSON.stringify(artifacts, null, 2) + '\n');
// Only remove after every archive is successfully built; the installer embeds core only.
if (!fromStaging) for (const pack of packs) for (const path of pack.files) await unlink(join('dist', path));
for (const pack of artifacts) console.log(`[optional-resources] ${pack.label}: ${(pack.bytes / 1048576).toFixed(2)} MiB -> ${pack.filename}`);
