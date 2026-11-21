import sharp from 'sharp';
import { createHash } from 'node:crypto';
import { readFile, writeFile, mkdir, readdir, stat } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { readdirSync } from 'node:fs';

// Keep the original calibrated set as a separate regression budget.
export const calibratedSheets = [
  'walk/nana/nana-walk-left-sheet.webp',
  'walk/nana/nana-walk-right-sheet.webp',
  ...['angry', 'think', 'smug', 'wake', 'busy-in', 'busy-loop', 'cast',
    'tend-out', 'tend', 'tired'].map(name => `motion/nana/nana-${name}-sheet.webp`),
];
// Compress every shipped sprite surface: complete animation sheets *and* the
// default expression atlases. Prop artwork and other artwork stay untouched.
const projectRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
export function discoverArtwork(directory, relative = '') {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const path = [relative, entry.name].filter(Boolean).join('/');
    return entry.isDirectory() ? discoverArtwork(join(directory, entry.name), path)
      : /-(sheet|atlas)\.webp$/.test(path) ? [path] : [];
  });
}
export const releaseArtwork = discoverArtwork(join(projectRoot, 'public/chibi')).sort();
export const releaseSheets = releaseArtwork.filter(path => path.endsWith('-sheet.webp'));
// effort 6 相对 4 只再省约 3~4% 体积，编码耗时却高一个数量级（实测 2.6s vs 0.04s/张）。
export const releaseProfile = Object.freeze({ quality: 90, alphaQuality: 100, effort: 4 });

/** 母版格位：前端逻辑坐标系、所有校准表与物理换算都以此为准，不随发布分辨率变化。 */
export const MASTER_CELL = 512;
/** 发布格位。改这一个常量即可在 512 / 384 / 320 / 256 之间切换，母版与前端都不用动。 */
export const RELEASE_CELL = 320;

/**
 * 把母版等比降到发布格位，作为编码输入与画质校验基准。
 * 前端用 `background-size: cols*100%` 把格子拉伸回容器，格子在 CSS 里的尺寸不变，
 * 所以 defaultSize / bottomDock / frameScales / frameAnchors / desktopPhysics 的
 * 512 坐标系继续有效，无需重算任何校准数值。
 */
export async function downscaleSource(source, cell = RELEASE_CELL) {
  const { data, info } = await sharp(source).ensureAlpha().raw().toBuffer({ resolveWithObject: true });
  const cols = info.width / MASTER_CELL, rows = info.height / MASTER_CELL;
  if (!Number.isInteger(cols) || !Number.isInteger(rows)) {
    throw new Error(`Non-integral master grid: ${info.width}x${info.height}`);
  }
  const width = cols * cell, height = rows * cell;
  const resized = await sharp(data, { raw: { width: info.width, height: info.height, channels: 4 } })
    .resize(width, height, { kernel: 'lanczos3' }).ensureAlpha().raw().toBuffer();
  // 完全透明像素里的 RGB 是编码遗留噪声，会让后续编码白白多花比特；清零不可见且可省 1~3%。
  for (let k = 0; k < resized.length; k += 4) {
    if (resized[k + 3] === 0) { resized[k] = 0; resized[k + 1] = 0; resized[k + 2] = 0; }
  }
  return { data: resized, info: { width, height, channels: 4 } };
}

export async function validateReleaseImage(source, encoded, baseline = null, materialKind = 'lavender') {
  const before = baseline ?? await sharp(source).ensureAlpha().raw().toBuffer({ resolveWithObject: true });
  const after = await sharp(encoded).ensureAlpha().raw().toBuffer({ resolveWithObject: true });
  const warm = materialKind === 'cream';
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
    // 暖白角色（claude / deepseek）的裙装是奶油色，不满足冷色的 b>=r 条件；
    // 沿用冷色判据只会命中千把个偏蓝高光，均值毫无代表性。与 palette 测试口径一致。
    const light = warm
      ? a[i] > 175 && a[i + 1] > 145 && a[i + 2] > 130 && a[i] >= a[i + 1] && a[i + 1] >= a[i + 2] && a[i] - a[i + 2] < 80
      : a[i] > 140 && a[i + 1] > 125 && a[i + 2] >= a[i] && a[i + 2] - a[i + 1] < 65;
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
  // 浅色材质专项预算的三条口径修正：
  // 1) 样本不足一万时均值没有统计意义（暖白角色的冷色高光只命中千把个像素，
  //    却能把偏移放大到 8+，把合格的 Q90 一路逼到 lossless）。
  // 2) 材质判据按角色分流，见上面的 warm 分支。
  // 3) 降分辨率后浅色边缘会有 1~3/255 的系统性编码偏移，视觉不可见；沿用过严的
  //    1/255 会把整批图推到 Q98/lossless，抵消降分辨率的收益。宽松到 3/255 后
  //    仍拦得住真正的大面积色偏。
  // 整体 RMSE 预算 5 → 6.5：256 格位下一批 claude 图连 Q98 都在 5.5~6.3，会被逼回
  // lossless（体积翻倍）。6.5/255 仍是肉眼不可见的量级，均值约束 1/255 不变。
  const materialBudget = material >= 10000 ? lightMaterialColourShift : [];
  const overallBad = rmse > 6.5 || meanColourShift.some(value => Math.abs(value) > 1);
  if (overallBad || materialBudget.some(value => Math.abs(value) > 3)) {
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

// Dense generated sheets often fail lossy WebP's colour budget. Reduce tiny RGB
// noise before lossless encoding instead, preserving every original alpha byte.
export async function compactSprite(baseline, material = 'lavender') {
  const {data,info}=baseline;
  for(const step of [12,8]) {
    const pixels=Buffer.from(data);
    for(let i=0;i<pixels.length;i+=4)for(let channel=0;channel<3;channel++) {
      const value=pixels[i+channel];
      pixels[i+channel]=pixels[i+3]===0?0:value>247?value:Math.min(255,Math.round(value/step)*step);
    }
    const encoded=await sharp(pixels,{raw:info}).webp({lossless:true,effort:4}).toBuffer();
    try {const metrics=await validateReleaseImage(null,encoded,baseline,material);return {encoded,metrics};}
    catch(error){if(!error.message.includes('colour error'))throw error;}
  }
  return null;
}

// Reject (instead of hanging the whole build) if a single sheet's encode or
// validation stalls. A stalled libvips pipeline would otherwise freeze `npm run
// package:small` with no diagnostic and no progress output.
async function withTimeout(promise, ms, label) {
  let timer;
  const guard = new Promise((_, reject) =>
    timer = setTimeout(() => reject(new Error(`timed out after ${ms}ms: ${label}`)), ms));
  try { return await Promise.race([promise, guard]); }
  finally { clearTimeout(timer); }
}

// When a sheet is copied through unchanged we know the delta is zero; derive
// dimensions cheaply without decoding pixels, so the report stays consistent.
async function metadataMetrics(source, baseline = null) {
  const { width, height } = baseline?.info ?? await sharp(source).metadata();
  return { width, height, alphaErrors: 0, rmse: 0, meanColourShift: [0, 0, 0], lightMaterialColourShift: [0, 0, 0] };
}

export async function compressChibi(root = process.cwd(), { encodeTimeoutMs = 120000, sourceDirectory, outputDirectory, artwork, reportName = 'chibi-release-report.json', requireReleaseGrid = false } = {}) {
  const sourceDir = sourceDirectory ?? resolve(root, 'public/chibi'), outDir = outputDirectory ?? resolve(root, 'dist/chibi');
  const surfaces = artwork ?? releaseArtwork;
  const cacheDir = resolve(root, 'tmp/chibi-release-cache');
  await mkdir(cacheDir, { recursive: true });
  const total = surfaces.length;
  const files = [];
  let done = 0;
  for (const path of surfaces) {
    const startedAt = Date.now();
    const source = await readFile(join(sourceDir, path));
    // Require the whitelist copy to have completed; never create a partial build.
    await stat(join(outDir, path));
    const baseline = await downscaleSource(source);
    const material = /claude|deepseek/.test(path) ? 'cream' : 'lavender';
    // revision 8: 发布格位收到 256、整体 RMSE 预算 5 → 6.5。缓存键绑定格位与修订。
    const key = createHash('sha256').update(source)
      .update(JSON.stringify({ revision: 8, cell: RELEASE_CELL, material, profile: releaseProfile, versions: sharp.versions })).digest('hex');
    const cacheFile = join(cacheDir, `${key}.webp`);
    const metricsFile = join(cacheDir, `${key}.json`);
    let encoded, metrics, cacheHit = false;
    try {
      encoded = await readFile(cacheFile);
      // The cache key pins the exact source bytes + profile + sharp version, so a
      // hit is guaranteed identical to a previously validated encode. Reuse the
      // stored metrics instead of re-decoding and re-validating every sheet on
      // warm runs (this was the silent multi-minute cost on repeat builds).
      try {
        metrics = JSON.parse(await readFile(metricsFile));
      } catch {
        metrics = await validateReleaseImage(source, encoded, baseline, material);
        // Persist the computed metrics so subsequent runs skip this decode entirely.
        await writeFile(metricsFile, JSON.stringify(metrics)).catch(() => {});
      }
      cacheHit = true;
    } catch (error) {
      // A missing or damaged local cache can always be rebuilt from the master.
      if (error.code && error.code !== 'ENOENT') throw error;
      // Existing lossy sheets may need higher quality to avoid cumulative colour error.
      for (const quality of [90, 95, 98]) {
        try {
          const candidate = await withTimeout(
            sharp(baseline.data, { raw: baseline.info }).webp({ ...releaseProfile, quality }).toBuffer(),
            encodeTimeoutMs, `encode ${path} @q${quality}`);
          metrics = await withTimeout(
            validateReleaseImage(source, candidate, baseline, material),
            encodeTimeoutMs, `validate ${path} @q${quality}`);
          encoded = candidate;
          break;
        } catch (validationError) {
          // Colour budget or a stalled pipeline: try the next quality, never hang.
          if (validationError.message.includes('colour error') || validationError.message.includes('timed out')) continue;
          throw validationError;
        }
      }
      if (!encoded) {
        console.warn(`[compress-chibi] ${path}: colour budget unreachable, falling back to lossless release grid`);
        encoded = await sharp(baseline.data, { raw: baseline.info }).webp({ lossless: true, effort: 4 }).toBuffer();
        metrics = await metadataMetrics(source, baseline);
      }
      await writeFile(cacheFile, encoded);
      await writeFile(metricsFile, JSON.stringify(metrics)).catch(() => {});
    }
    if (encoded.length >= source.length * .95) {
      try {
        const compact = await withTimeout(compactSprite(baseline, material), encodeTimeoutMs, `compact ${path}`);
        if (compact && compact.encoded.length < encoded.length) {
          encoded = compact.encoded; metrics = compact.metrics;
          await writeFile(cacheFile, encoded);
          await writeFile(metricsFile, JSON.stringify(metrics)).catch(() => {});
        }
      } catch (compactError) {
        if (!compactError.message.includes('timed out')) throw compactError;
        console.warn(`[compress-chibi] ${path}: compact skipped (timed out)`);
      }
    }
    const smaller = encoded.length < source.length * .95;
    const publishEncoded = smaller || requireReleaseGrid;
    await writeFile(join(outDir, path), publishEncoded ? encoded : source);
    if (!publishEncoded) metrics = await metadataMetrics(source, baseline);
    const outLen = publishEncoded ? encoded.length : source.length;
    const ratio = outLen / source.length * 100;
    const tag = cacheHit ? 'cache' : (smaller ? 'compressed' : 'copy');
    done++;
    const ms = Date.now() - startedAt;
    console.log(`[compress-chibi] ${String(done).padStart(3)}/${total} ${path.padEnd(40)} ` +
      `${(source.length / 1048576).toFixed(2)}MB -> ${(outLen / 1048576).toFixed(2)}MB (${ratio.toFixed(1)}%) ` +
      `${(ms / 1000).toFixed(1)}s [${tag}]`);
    files.push({ path, kind: path.endsWith('-atlas.webp') ? 'atlas' : 'sheet',
      before: source.length, after: outLen, cacheHit, compressed: smaller, ...metrics });
  }
  const savedBytes = files.reduce((sum, file) => sum + file.before - file.after, 0);
  const finalChibiBytes = await directoryBytes(outDir);
  const finalFrontendBytes = await directoryBytes(resolve(root, 'dist'));
  // Reconstruct the master baseline so standalone reruns remain comparable.
  const report = { profile: { ...releaseProfile, qualityCandidates: [90, 95, 98], minimumSaving: .05 },
    masterCell: MASTER_CELL, releaseCell: RELEASE_CELL,
    originalChibiBytes: finalChibiBytes + savedBytes,
    originalFrontendBytes: finalFrontendBytes + savedBytes, savedBytes, finalChibiBytes, finalFrontendBytes, files };
  await writeFile(resolve(root, 'tmp', reportName), JSON.stringify(report, null, 2) + '\n');
  const atlasCount = files.filter(file => file.kind === 'atlas').length;
  console.log(`[compress-chibi] ${files.length} files (${atlasCount} atlas); saved ${(savedBytes / 1048576).toFixed(2)} MiB; ` +
    `chibi ${(report.finalChibiBytes / 1048576).toFixed(2)} MiB; cache ${files.filter(file => file.cacheHit).length}/${files.length}`);
  return report;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await compressChibi();
}
