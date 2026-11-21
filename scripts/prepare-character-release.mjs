import { cp, mkdir, readdir, readFile, rmdir, unlink, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { compressChibi, discoverArtwork, RELEASE_CELL } from './compress-chibi.mjs';

const root = process.cwd();

async function emptyDirectory(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      await emptyDirectory(path);
      await rmdir(path);
    } else {
      await unlink(path);
    }
  }
}

async function prepareCharacter(id) {
  if (!/^[a-z0-9-]+$/.test(id)) throw Error('Invalid character ID');
  const source = resolve(root, 'character-packs', id);
  const stage = resolve(root, 'tmp/character-release', id);
  const artworkDirectory = join(source, 'assets/chibi');

  await mkdir(stage, { recursive: true });
  await emptyDirectory(stage);
  for (const folder of ['assets', 'prompts']) {
    await cp(join(source, folder), join(stage, folder), { recursive: true });
  }

  const manifest = JSON.parse(await readFile(join(source, 'character.json'), 'utf8'));
  manifest.cell_size = RELEASE_CELL;
  await writeFile(join(stage, 'character.json'), JSON.stringify(manifest, null, 2) + '\n');
  await compressChibi(root, {
    sourceDirectory: artworkDirectory,
    outputDirectory: join(stage, 'assets/chibi'),
    artwork: discoverArtwork(artworkDirectory).sort(),
    reportName: `character-${id}-compression.json`,
    requireReleaseGrid: true,
  });
}

for (const id of process.argv.slice(2)) await prepareCharacter(id);
