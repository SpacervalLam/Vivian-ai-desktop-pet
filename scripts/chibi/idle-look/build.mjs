/**
 * 待机「四处张望」图集装配。
 *
 * 输入：`raw/` 下四张绿幕关键帧（left-near / left-full / right-near / right-full）。
 * 输出：`public/chibi/motion/vivian/vivian-idle-look-sheet.webp`（4 列 2 行，8 帧，512 格位）。
 *
 * 正面帧不重新生成：直接取 `vivian-atlas.webp` 的 idle 格位，保证循环首帧与静态待机
 * 逐像素一致，进入/退出循环时不会闪一下。其余关键帧抠掉绿幕、按角色高度归一化，
 * 再以脚底基线与脚部横向中心对齐——头部转动会移动整体包围盒，只有脚不动，所以横向
 * 必须锚在脚上，否则每帧人物会左右漂。
 *
 * 参数（可选）：node build.mjs [raw 目录] [输出路径]
 */
import sharp from 'sharp';
import { mkdir, writeFile } from 'node:fs/promises';
import { access } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(fileURLToPath(new URL('../../..', import.meta.url)));
const CELL = 512;
const COLS = 4;
const ROWS = 2;
const ATLAS = join(ROOT, 'public/chibi/vivian-atlas.webp');
const RAW_DIR = resolve(ROOT, process.argv[2] ?? 'assets/chibi/idle-look/raw');
const OUT = resolve(ROOT, process.argv[3] ?? 'public/chibi/motion/vivian/vivian-idle-look-sheet.webp');

/** 抠绿幕时用来判前景/背景的绿差阈值：<=LO 视为前景，>=HI 视为背景，中间线性过渡。 */
const KEY_LO = 14;
const KEY_HI = 64;
/** 脚部锚定带：自包围盒底部往上取的高占比，用于求稳定的脚部横向中心。 */
const BAND_RATIO = 0.12;
/** 帧序列的播放顺序（在图集里的排布）；重复项写成同一张关键帧，靠时长做停顿。 */
const ORDER = ['ref', 'left-near', 'left-full', 'left-near', 'ref', 'right-near', 'right-full', 'right-near'];

async function toRaw(input, width, height) {
  const { data } = await sharp(input).ensureAlpha().resize(width, height, { fit: 'fill' }).raw().toBuffer({ resolveWithObject: true });
  return data;
}

/** 包围盒 + 脚部锚点：脚不动，头部转动只改包围盒上沿，故横向锚脚最稳。 */
function measure(data, width, height, alphaMin = 16) {
  let minX = width, minY = height, maxX = -1, maxY = -1;
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      if (data[(y * width + x) * 4 + 3] > alphaMin) {
        if (x < minX) minX = x;
        if (x > maxX) maxX = x;
        if (y < minY) minY = y;
        if (y > maxY) maxY = y;
      }
    }
  }
  if (maxX < 0) throw new Error('空帧：抠幕后没有任何不透明像素');
  const bandTop = Math.max(minY, maxY - Math.round((maxY - minY + 1) * BAND_RATIO));
  let sum = 0, count = 0;
  for (let y = bandTop; y <= maxY; y++) {
    for (let x = minX; x <= maxX; x++) {
      if (data[(y * width + x) * 4 + 3] > alphaMin) { sum += x; count += 1; }
    }
  }
  return { minX, minY, maxX, maxY, width: maxX - minX + 1, height: maxY - minY + 1, feetX: count ? sum / count : (minX + maxX) / 2 };
}

/**
 * 抠绿幕：按绿差取 alpha，并对半透明边缘做去溢色（把过量的绿压回其它通道），
 * 避免缩放后边缘出现绿色描边。
 */
function chromaKey(data) {
  for (let i = 0; i < data.length; i += 4) {
    const r = data[i], g = data[i + 1], b = data[i + 2];
    const difference = g - Math.max(r, b);
    let alpha;
    if (difference <= KEY_LO) alpha = 255;
    else if (difference >= KEY_HI) alpha = 0;
    else alpha = Math.round(255 * (1 - (difference - KEY_LO) / (KEY_HI - KEY_LO)));
    if (difference > KEY_LO) data[i + 1] = Math.min(g, Math.max(r, b));
    data[i + 3] = alpha;
  }
  return data;
}

async function atlasIdleCell() {
  const { data } = await sharp(ATLAS).ensureAlpha().extract({ left: 0, top: 0, width: CELL, height: CELL })
    .raw().toBuffer({ resolveWithObject: true });
  return data;
}

/** 把一张关键帧归一化到与参考帧同高、同脚底基线、同脚部横向中心，落进 512 格位。 */
async function normalizeCell(file, reference) {
  const source = await sharp(file).ensureAlpha().raw().toBuffer({ resolveWithObject: true });
  const keyed = chromaKey(source.data);
  const box = measure(keyed, source.info.width, source.info.height);
  const scale = reference.height / box.height;
  const width = Math.max(1, Math.round(box.width * scale));
  const cropped = await sharp(keyed, { raw: { width: source.info.width, height: source.info.height, channels: 4 } })
    .extract({ left: box.minX, top: box.minY, width: box.width, height: box.height })
    .resize(width, reference.height, { fit: 'fill' })
    .raw().toBuffer({ resolveWithObject: true });
  const placed = measure(cropped.data, width, reference.height);
  const left = Math.round(reference.feetX - placed.feetX);
  const top = reference.footBaselineY - reference.height + 1;
  const cell = Buffer.alloc(CELL * CELL * 4);
  for (let y = 0; y < reference.height; y++) {
    const dy = top + y;
    if (dy < 0 || dy >= CELL) continue;
    for (let x = 0; x < width; x++) {
      const dx = left + x;
      if (dx < 0 || dx >= CELL) continue;
      const from = (y * width + x) * 4;
      const to = (dy * CELL + dx) * 4;
      cell[to] = cropped.data[from]; cell[to + 1] = cropped.data[from + 1];
      cell[to + 2] = cropped.data[from + 2]; cell[to + 3] = cropped.data[from + 3];
    }
  }
  return cell;
}

/** 关键帧文件名不固定后缀：生成工具可能落成 png / jpg / webp。 */
async function findRaw(name) {
  for (const extension of ['png', 'jpg', 'jpeg', 'webp']) {
    const candidate = join(RAW_DIR, `${name}.${extension}`);
    try { await access(candidate); return candidate; } catch { /* try next extension */ }
  }
  throw new Error(`找不到 ${name} 关键帧`);
}

async function main() {
  const referenceData = await atlasIdleCell();
  const reference = { ...measure(referenceData, CELL, CELL), footBaselineY: 0 };
  reference.footBaselineY = reference.maxY;

  const missing = [];
  for (const name of ['left-near', 'left-full', 'right-near', 'right-full']) {
    try { await access(await findRaw(name)); } catch { missing.push(`${name}.png|jpg|webp`); }
  }
  if (missing.length) throw new Error(`缺少关键帧：${missing.join(', ')}（目录 ${RAW_DIR}）`);

  const cells = new Map();
  for (const name of ['left-near', 'left-full', 'right-near', 'right-full']) {
    cells.set(name, await normalizeCell(await findRaw(name), reference));
  }

  const overlay = [];
  ORDER.forEach((name, index) => {
    overlay.push({
      input: name === 'ref' ? referenceData : cells.get(name),
      raw: { width: CELL, height: CELL, channels: 4 },
      left: (index % COLS) * CELL,
      top: Math.floor(index / COLS) * CELL,
    });
  });

  await mkdir(resolve(OUT, '..'), { recursive: true });
  const sheet = await sharp({ create: { width: COLS * CELL, height: ROWS * CELL, channels: 4, background: { r: 0, g: 0, b: 0, alpha: 0 } } })
    .composite(overlay)
    .webp({ lossless: true, effort: 6 })
    .toBuffer();
  await writeFile(OUT, sheet);

  const metrics = await sharp(sheet).metadata();
  console.log(`[idle-look] 参考基数 高 ${reference.height} 脚底 y=${reference.footBaselineY} 脚心 x=${reference.feetX.toFixed(1)}`);
  console.log(`[idle-look] 输出 ${metrics.width}x${metrics.height} alpha=${metrics.hasAlpha} -> ${OUT}`);
}

main().catch(error => { console.error(`[idle-look] ${error.message}`); process.exitCode = 1; });
