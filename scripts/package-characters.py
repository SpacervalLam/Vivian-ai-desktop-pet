"""Build deterministic data-only character ZIPs, separate from the application."""
import argparse
import hashlib
import json
import subprocess
from contextlib import contextmanager
from pathlib import Path
from tempfile import NamedTemporaryFile
from zipfile import ZipFile, ZipInfo, ZIP_DEFLATED

ROOT = Path(__file__).resolve().parents[1]


@contextmanager
def atomic_output(target: Path):
    """Keep the previous release readable until its replacement is complete."""
    with NamedTemporaryFile(dir=target.parent, suffix='.tmp', delete=False) as handle:
        temporary = Path(handle.name)
    try:
        yield temporary
        temporary.replace(target)
    finally:
        temporary.unlink(missing_ok=True)


def file_digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def build(character: str, destination: Path) -> dict:
    source = ROOT / 'tmp' / 'character-release' / character
    definition = json.loads((source / 'character.json').read_text(encoding='utf-8'))
    files = {}
    for folder in ('assets', 'prompts'):
        for path in sorted((source / folder).rglob('*')):
            if path.is_file():
                if path.is_symlink():
                    raise ValueError(f'Symlinks are not supported: {path}')
                files[path.relative_to(source).as_posix()] = file_digest(path)
    for required in (f'assets/chibi/{character}-atlas.webp', 'prompts/identity.md', 'prompts/personality.md'):
        if required not in files:
            raise ValueError(f'{character}: missing {required}')
    manifest = {**definition, 'files': files}
    manifest_bytes = (json.dumps(manifest, ensure_ascii=False, indent=2) + '\n').encode()
    destination.mkdir(parents=True, exist_ok=True)
    filename = f'Vivian-character-{character}-{manifest["version"]}.zip'
    target = destination / filename
    with atomic_output(target) as temporary:
        with ZipFile(temporary, 'w', ZIP_DEFLATED, compresslevel=9) as archive:
            for name in ('character.json', *files):
                data = manifest_bytes if name == 'character.json' else (source / name).read_bytes()
                if name in files and hashlib.sha256(data).hexdigest() != files[name]:
                    raise ValueError(f'{character}: {name} changed during packaging')
                info = ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
                info.external_attr = 0o644 << 16
                archive.writestr(info, data, compress_type=ZIP_DEFLATED, compresslevel=9)
        with atomic_output(source / 'character.json') as staged_manifest:
            staged_manifest.write_bytes(manifest_bytes)
        checksum = file_digest(temporary)
        size = temporary.stat().st_size
    return {'id': character, 'name': manifest['name'], 'version': manifest['version'],
            'filename': filename, 'bytes': size, 'sha256': checksum}


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('characters', nargs='*')
    parser.add_argument('--output', type=Path, default=ROOT / 'release' / 'characters')
    args = parser.parse_args()
    characters = args.characters or [p.parent.name for p in sorted((ROOT / 'character-packs').glob('*/character.json'))]
    subprocess.run(['node', str(ROOT / 'scripts' / 'prepare-character-release.mjs'), *characters], cwd=ROOT, check=True)
    rows = [build(character, args.output) for character in characters]
    if args.characters and (args.output / 'catalog.json').exists():
        previous = json.loads((args.output / 'catalog.json').read_text(encoding='utf-8'))
        rows += [row for row in previous if row['id'] not in characters and (args.output / row['filename']).exists()]
    rows.sort(key=lambda row: row['id'])
    with atomic_output(args.output / 'catalog.json') as catalog:
        catalog.write_text(json.dumps(rows, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    for row in rows:
        print(f'{row["name"]}: {row["filename"]} ({row["bytes"] / 1048576:.1f} MiB)')
