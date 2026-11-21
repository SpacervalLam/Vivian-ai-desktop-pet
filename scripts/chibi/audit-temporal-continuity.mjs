import sharp from 'sharp';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Compare premultiplied pixels: invisible RGB must not count as motion.
export function frameDifference(a, b) {
  if (a.length !== b.length || a.length % 4) throw new Error('Mismatched RGBA frames');
  let total = 0;
  for (let k = 0; k < a.length; k += 4) {
    const aa = a[k + 3] / 255, ba = b[k + 3] / 255;
    for (let c = 0; c < 3; c++) total += Math.abs(a[k + c] * aa - b[k + c] * ba);
    total += Math.abs(a[k + 3] - b[k + 3]);
  }
  return total / a.length;
}

export async function auditSheet(file, spec) {
  const { data, info } = await sharp(file).ensureAlpha().raw().toBuffer({ resolveWithObject: true });
  if (info.width !== spec.cols * 512 || info.height !== spec.rows * 512) {
    throw new Error(`Unexpected grid: ${file}`);
  }
  const frames = [];
  for (let i = 0; i < spec.frames; i++) {
    const cell = Buffer.alloc(512 * 512 * 4);
    for (let y = 0; y < 512; y++) {
      const start = ((Math.floor(i / spec.cols) * 512 + y) * info.width + i % spec.cols * 512) * 4;
      data.copy(cell, y * 2048, start, start + 2048);
    }
    frames.push(cell);
  }
  const steps = frames.slice(1).map((frame, i) => frameDifference(frames[i], frame));
  const inner = steps.slice(1, -1).sort((a, b) => a - b);
  const median = inner.length ? inner[Math.floor(inner.length / 2)] : 0;
  const round = n => Math.round(n * 1000) / 1000;
  return {
    steps: steps.map(round),
    opening: round(steps[0] ?? 0),
    closing: round(steps.at(-1) ?? 0),
    innerMedian: round(median),
    openingRatio: median > 0 ? round(steps[0] / median) : null,
    closingRatio: median > 0 ? round(steps.at(-1) / median) : null,
    // A heuristic for review, not proof that deliberate large motion is wrong.
    reviewOpening: median > 0 && steps[0] > median * 2,
  };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = fileURLToPath(new URL('../../', import.meta.url));
  const id = process.argv[2] ?? 'deepseek';
  if (!/^[a-z0-9-]+$/.test(id)) throw new Error('Invalid character ID');
  const manifest = JSON.parse(await readFile(resolve(root, 'character-packs', id, 'character.json'), 'utf8'));
  const vocabulary = JSON.parse(await readFile(resolve(root, 'src/chibi/animations.json'), 'utf8'));
  const rows = [];
  for (const spec of vocabulary.animations) {
    if (spec.frames < 4) continue;
    for (const direction of spec.directions ?? ['left']) {
      const url = spec.sheet.replaceAll('{character}', id).replaceAll('{direction}', direction);
      if (!manifest.files['assets' + url]) continue;
      rows.push({ motion: spec.name, direction, ...await auditSheet(resolve(root, 'character-packs', id, 'assets' + url), spec) });
    }
  }
  console.log(JSON.stringify(rows, null, 2));
}
