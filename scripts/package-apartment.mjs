import { spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { resolve } from 'node:path';

const root = resolve('plugins/3d-apartment');
const manifest = JSON.parse(readFileSync(`${root}/plugin.json`, 'utf8'));
const filename = `Vivian-3D-Apartment-${manifest.version}.zip`;
mkdirSync('release', { recursive: true });
const result = spawnSync('python', ['-c', `
from pathlib import Path
from zipfile import ZipFile, ZIP_DEFLATED
import sys
root, destination = map(Path, sys.argv[1:])
with ZipFile(destination, 'w', ZIP_DEFLATED, compresslevel=9) as archive:
    for path in [root/'plugin.json', root/'ui/room.js', *sorted((root/'room').rglob('*.glb'))]:
        if not path.is_file(): raise RuntimeError(f'Missing plugin asset: {path}')
        archive.write(path, path.relative_to(root).as_posix())
`, root, resolve('release', filename)], { stdio: 'inherit' });
if (result.status !== 0) throw new Error('Apartment archive creation failed');
const hash = createHash('sha256').update(readFileSync(`release/${filename}`)).digest('hex');
writeFileSync('src-tauri/windows/apartment-package.nsh', `!define APARTMENT_PACKAGE "${filename}"\n!define APARTMENT_SHA256 "${hash}"\n`);
console.log(`[apartment-package] ${filename}, SHA256 ${hash}`);
