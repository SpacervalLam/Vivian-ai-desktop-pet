import sharp from 'sharp';
import { createHash } from 'node:crypto';
import { readFile, writeFile, mkdir, readdir, stat } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { readdirSync } from 'node:fs';

// Keep the original calibrated set as a separate regression budget.
export const calibratedSheets = [
  ...['left', 'right'].map(direction => `walk/nana/nana-walk-${direction}-sheet.webp`),
  ...['angry', 'think', 'smug', 'wake', 'busy-in', 'busy-loop', 'cast',
    'tend-out', 'tend', 'tired'].map(name => `motion/nana/nana-${name}-sheet.webp`),
];
// Compress complete animation sheets only; default atlases and prop artwork stay untouched.
const projectRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
function discoverSheets(directory, relative = '') {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const path = [relative, entry.name].filter(Boolean).join('/');
    return entry.isDirectory() ? discoverSheets(join(directory, entry.name), path)
      : path.endsWith('-sheet.webp') ? [path] : [];
  });
}
export const releaseSheets = discoverSheets(join(projectRoot, 'public/chibi')).sort();
export const releaseProfile = Object.freeze({ quality: 90, alphaQuality: 100, effort: 6 });

export async function validateReleaseImage(source, encoded) {
  const before = await sharp(source).ensureAlpha().raw().toBuffer({ resolveWithObject: true });
  const after = await sharp(encoded).ensureAlpha().raw().toBuffer({ resolveWithObject: true });
  if (before.info.width !== after.info.width || before.info.height !== after.info.height) {
    throw new Error('Sprite dimensions changed');
  }
  let squares = 0, opaque = 0, material = 0;
  const colourShift = [0, 0, 0], materialShift = [0, 0, 0];
  for (let i = 0; i < before.data.length; i += 4) {
    const a = before.data, b = after.data;
    if (a[i + 3] !== b[i + 3]) throw new Error('Sprite alpha changed');
    if (a[i + 3] <= 230) continue;
    opaque++;
    // Sample the original light lavender/white pixels, not a shifting output mask.
    const light = a[i] > 140 && a[i + 1] > 125 && a[i + 2] >= a[i] && a[i + 2] - a[i + 1] < 65;
    if (light) material++;
    for (let c = 0; c < 3; c++) {
      const difference = b[i + c] - a[i + c];
      squares += difference ** 2;
      colourShift[c] += difference;
      if (light) materialShift[c] += difference;
    }
  }
  const rmse = opaque ? Math.sqrt(squares / (opaque * 3)) : 0;
  const meanColourShift = colourShift.map(value => opaque ? value / opaque : 0);
  const lightMaterialColourShift = materialShift.map(value => material ? value / material : 0);
  if (rmse > 5 || [...meanColourShift, ...lightMaterialColourShift].some(value => Math.abs(value) > 1)) {
    throw new Error('Sprite colour error exceeds release budget');
  }
  return { width: after.info.width, height: after.info.height, alphaErrors: 0,
    rmse, meanColourShift, lightMaterialColourShift };
}

async function directoryBytes(directory) {
  let total = 0;
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    total += entry.isDirectory() ? await directoryBytes(path) : (await stat(path)).size;
  }
  return total;
}

export async function compressChibi(root = process.cwd()) {
  const sourceDir = resolve(root, 'public/chibi'), outDir = resolve(root, 'dist/chibi');
  const cacheDir = resolve(root, 'tmp/chibi-release-cache');
  await mkdir(cacheDir, { recursive: true });
  const files = [];
  for (const path of releaseSheets) {
    const source = await readFile(join(sourceDir, path));
    // Require the whitelist copy to have completed; never create a partial build.
    await stat(join(outDir, path));
    const key = createHash('sha256').update(source)
      .update(JSON.stringify({ revision: 2, profile: releaseProfile, versions: sharp.versions })).digest('hex');
    const cacheFile = join(cacheDir, `${key}.webp`);
    let encoded, metrics, cacheHit = false;
    try {
      encoded = await readFile(cacheFile);
      metrics = await validateReleaseImage(source, encoded);
      cacheHit = true;
    } catch (error) {
      // A missing or damaged local cache can always be rebuilt from the master.
      if (error.code && error.code !== 'ENOENT') throw error;
      // Existing lossy sheets may need higher quality to avoid cumulative colour error.
      for (const quality of [90, 95, 98]) {
        const candidate = await sharp(source).webp({ ...releaseProfile, quality }).toBuffer();
        try {
          metrics = await validateReleaseImage(source, candidate);
          encoded = candidate;
          break;
        } catch (validationError) {
          if (!validationError.message.includes('colour error')) throw validationError;
        }
      }
      if (!encoded) { encoded = source; metrics = await validateReleaseImage(source, source); }
      await writeFile(cacheFile, encoded);
    }
    const smaller = encoded.length < source.length * .95;
    await writeFile(join(outDir, path), smaller ? encoded : source);
    files.push({ path, before: source.length, after: smaller ? encoded.length : source.length,
      cacheHit, compressed: smaller, ...(smaller ? metrics : await validateReleaseImage(source, source)) });
  }
  const savedBytes = files.reduce((sum, file) => sum + file.before - file.after, 0);
  const finalChibiBytes = await directoryBytes(outDir);
  const finalFrontendBytes = await directoryBytes(resolve(root, 'dist'));
  // Reconstruct the master baseline so standalone reruns remain comparable.
  const report = { profile: { ...releaseProfile, qualityCandidates: [90, 95, 98], minimumSaving: .05 }, originalChibiBytes: finalChibiBytes + savedBytes,
    originalFrontendBytes: finalFrontendBytes + savedBytes, savedBytes, finalChibiBytes, finalFrontendBytes, files };
  await writeFile(resolve(root, 'tmp/chibi-release-report.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(`[compress-chibi] ${files.length} sheets; saved ${(savedBytes / 1048576).toFixed(2)} MiB; ` +
    `chibi ${(report.finalChibiBytes / 1048576).toFixed(2)} MiB; cache ${files.filter(file => file.cacheHit).length}/${files.length}`);
  return report;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await compressChibi();
}
