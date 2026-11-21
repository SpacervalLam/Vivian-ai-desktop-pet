import sharp from 'sharp';
import { readFile, writeFile, mkdir, copyFile, access } from 'node:fs/promises';
import { resolve, dirname, relative, isAbsolute, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { frameDifference } from './audit-temporal-continuity.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const raw = { width: 512, height: 512, channels: 4 };

// Block motion moves a single source drawing toward its neighbouring pose.
// Work on premultiplied RGBA so transparent matte colours cannot attract flow.
async function reduced(frame) {
  const pixels = await sharp(frame, { raw }).resize(128, 128).raw().toBuffer();
  const out = new Float32Array(pixels.length);
  for (let k = 0; k < pixels.length; k += 4) {
    for (let c = 0; c < 3; c++) out[k + c] = pixels[k + c] * pixels[k + 3] / 255;
    out[k + 3] = pixels[k + 3];
  }
  return out;
}

function motion(a, b) {
  const grid = 32, field = new Float32Array(grid * grid * 2);
  for (let gy = 0; gy < grid; gy++) for (let gx = 0; gx < grid; gx++) {
    const x = gx * 4 + 2, y = gy * 4 + 2;
    let best = Infinity, bx = 0, by = 0;
    for (let dy = -7; dy <= 7; dy++) for (let dx = -7; dx <= 7; dx++) {
      let cost = .8 * (dx * dx + dy * dy);
      for (let py = -2; py <= 2; py++) for (let px = -2; px <= 2; px++) {
        const ax = Math.max(0, Math.min(127, x + px)), ay = Math.max(0, Math.min(127, y + py));
        const tx = Math.max(0, Math.min(127, x + px + dx)), ty = Math.max(0, Math.min(127, y + py + dy));
        const ai = (ay * 128 + ax) * 4, bi = (ty * 128 + tx) * 4;
        for (let c = 0; c < 4; c++) cost += Math.abs(a[ai + c] - b[bi + c]);
      }
      if (cost < best) { best = cost; bx = dx; by = dy; }
    }
    const i = (gy * grid + gx) * 2;
    field[i] = bx * 4; field[i + 1] = by * 4;
  }
  // Regularise local matches into broad coherent motion. Raw block matches can
  // otherwise bend thin hair ornaments or split an eye along block boundaries.
  let smooth = field;
  for (let pass = 0; pass < 4; pass++) {
    const next = new Float32Array(smooth.length);
    for (let y = 0; y < grid; y++) for (let x = 0; x < grid; x++) {
      for (let c = 0; c < 2; c++) {
        let total = 0, weight = 0;
        for (let dy = -2; dy <= 2; dy++) for (let dx = -2; dx <= 2; dx++) {
          const xx = Math.max(0, Math.min(grid - 1, x + dx)), yy = Math.max(0, Math.min(grid - 1, y + dy));
          const w = (3 - Math.abs(dx)) * (3 - Math.abs(dy));
          total += smooth[(yy * grid + xx) * 2 + c] * w; weight += w;
        }
        next[(y * grid + x) * 2 + c] = total / weight;
      }
    }
    smooth = next;
  }
  return smooth;
}

function sample(data, width, height, channels, x, y, channel, premultiply = false) {
  x = Math.max(0, Math.min(width - 1, x)); y = Math.max(0, Math.min(height - 1, y));
  const x0 = Math.floor(x), y0 = Math.floor(y), fx = x - x0, fy = y - y0;
  let value = 0;
  for (let j = 0; j < 2; j++) for (let i = 0; i < 2; i++) {
    const at = (Math.min(height - 1, y0 + j) * width + Math.min(width - 1, x0 + i)) * channels;
    const v = data[at + channel] * (premultiply && channel < 3 ? data[at + 3] / 255 : 1);
    value += v * (i ? fx : 1 - fx) * (j ? fy : 1 - fy);
  }
  return value;
}

export async function inbetween(a, b, t = .5) {
  const [smallA, smallB] = await Promise.all([reduced(a), reduced(b)]);
  const ab = motion(smallA, smallB);
  const out = Buffer.alloc(a.length);
  for (let y = 0; y < 512; y++) for (let x = 0; x < 512; x++) {
    const gx = (x - 8) / 16, gy = (y - 8) / 16;
    const ax = x - t * sample(ab, 32, 32, 2, gx, gy, 0);
    const ay = y - t * sample(ab, 32, 32, 2, gx, gy, 1);
    const i = (y * 512 + x) * 4;
    // Keep a single drawing's texture; mixing both drawings makes double eyes,
    // hands and skirt lines even when the broad silhouettes are registered.
    const alpha = sample(a, 512, 512, 4, ax, ay, 3);
    if (alpha < 8 || x < 2 || y < 2 || x > 509 || y > 509) continue;
    out[i + 3] = Math.round(alpha);
    for (let c = 0; c < 3; c++) out[i + c] = Math.round(255 / alpha * sample(a, 512, 512, 4, ax, ay, c, true));
  }
  return out;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const flags = process.argv.slice(2), id = flags.find(s => s.startsWith('--character='))?.split('=')[1] ?? 'deepseek';
  const backupFlag = flags.find(s => s.startsWith('--backup-root='))?.slice('--backup-root='.length);
  if (!backupFlag || !/^[a-z0-9-]+$/.test(id)) throw new Error('Supply --backup-root=<outside repository> and a valid character');
  const backup = resolve(backupFlag), within = relative(root, backup);
  if (!within || (!(within === '..' || within.startsWith('..' + sep)) && !isAbsolute(within))) throw new Error('Backup directory must be outside repository');
  const apply = flags.includes('--apply'), names = flags.find(s => s.startsWith('--motions='))?.slice(10).split(',');
  const manifestFile = resolve(root, 'character-packs', id, 'character.json');
  const manifest = JSON.parse(await readFile(manifestFile, 'utf8'));
  const vocabulary = JSON.parse(await readFile(resolve(root, 'src/chibi/animations.json'), 'utf8'));
  const standing = new Set(['blink', 'happy', 'angry', 'cast', 'thinking', 'smug', 'remind', 'tired', 'music', 'look']);
  await mkdir(backup, { recursive: true });
  const report = [];
  for (const spec of vocabulary.animations) {
    if (!standing.has(spec.name) || (names && !names.includes(spec.name))) continue;
    const url = spec.sheet.replaceAll('{character}', id), key = 'assets' + url;
    if (!manifest.files[key] || spec.frames < 4) continue;
    const file = resolve(root, 'character-packs', id, key), original = await readFile(file);
    const frames = [];
    for (let i = 0; i < spec.frames; i++) frames.push(await sharp(original).extract({ left: i % spec.cols * 512, top: Math.floor(i / spec.cols) * 512, width: 512, height: 512 }).ensureAlpha().raw().toBuffer());
    const changes = [];
    for (const [index, endpoint, neighbor] of [[1, 0, 2], [spec.frames - 2, spec.frames - 1, spec.frames - 3]]) {
      // Only repair closing boundaries which actually return to the same rest.
      if (endpoint !== 0 && frameDifference(frames[0], frames.at(-1)) > .5) continue;
      const old = frames[index], edge = frameDifference(frames[endpoint], old), adjacent = frameDifference(old, frames[neighbor]);
      if (edge < adjacent * 1.8) continue;
      const candidate = await inbetween(frames[endpoint], frames[neighbor]);
      const before = Math.max(edge, adjacent), after = Math.max(frameDifference(frames[endpoint], candidate), frameDifference(candidate, frames[neighbor]));
      if (after >= before * .9) continue;
      frames[index] = candidate;
      changes.push({ frame: index, before, after, reduction: 1 - after / before });
    }
    if (!changes.length) continue;
    // Copy pixels instead of alpha compositing: compositing rounds the colour
    // of partially transparent edges even in cells we did not change.
    const width = spec.cols * 512, height = spec.rows * 512;
    const pixels = Buffer.alloc(width * height * 4);
    for (let i = 0; i < frames.length; i++) for (let y = 0; y < 512; y++) {
      frames[i].copy(pixels, ((Math.floor(i / spec.cols) * 512 + y) * width + i % spec.cols * 512) * 4, y * 2048, (y + 1) * 2048);
    }
    const encoded = await sharp(pixels, { raw: { width, height, channels: 4 } }).webp({ lossless: true }).toBuffer();
    const output = resolve(backup, `${id}-${spec.name}-after.webp`);
    await writeFile(output, encoded);
    if (apply) {
      const saved = resolve(backup, 'originals', key); await mkdir(dirname(saved), { recursive: true });
      try { await access(saved); } catch { await copyFile(file, saved); }
      await writeFile(file, encoded);
      manifest.files[key] = createHash('sha256').update(encoded).digest('hex');
    }
    report.push({ motion: spec.name, changes, preview: output, applied: apply });
  }
  if (apply && report.length) {
    const saved = resolve(backup, 'character-before.json');
    try { await access(saved); } catch { await copyFile(manifestFile, saved); }
    await writeFile(manifestFile, JSON.stringify(manifest, null, 2) + '\n');
  }
  await writeFile(resolve(backup, apply ? 'applied.json' : 'preview.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report, null, 2));
}
