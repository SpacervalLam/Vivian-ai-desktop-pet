/**
 * 把 edge-bottom 的后三帧统一到第一帧的镜头尺度。
 *
 * 第一帧与站立 idle 同源，是尺寸基准，逐像素保留；后三帧按头部、眼线、下巴和身体分别配准。
 * 头部外轮廓与双眼独立对齐；第二帧的身体段再对齐第一帧脚底，避免整帧缩放改变身高。
 *
 * 用法：
 *   node scripts/chibi/edge-bottom-align/build.mjs --dry   # 只校验，不写 public/
 *   node scripts/chibi/edge-bottom-align/build.mjs         # 校验通过后才写回
 *
 * 首次运行把原图备份到 assets/chibi/edge-bottom-align/ *-edge-bottom-original.webp；之后一律从备份读取，
 * 重复运行不会累积缩放。备份不进安装包。
 */
import sharp from 'sharp';
import { access, copyFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { bounds, cleanMatte } from '../frame-sheets/registration.mjs';

const scriptDirectory = fileURLToPath(new URL('./', import.meta.url));
const base = resolve(scriptDirectory, '../../../');
const root = resolve(base, 'assets/chibi/edge-bottom-align');
const dry = process.argv.includes('--dry');
sharp.cache(false);
const CELL = 512;
// 原始 512px 格中左右虹膜中心的 X 坐标。用同一个脸部特征衡量镜头尺度，
// 避免把尾巴、帽子、抬手和最后一帧被遮住的下巴算进角色大小。
const EYES = {
  claude: [[221, 287], [232, 306], [229, 305], [222, 301]],
  deepseek: [[231, 293], [232, 299], [232, 295], [230, 300]],
  nana: [[239, 308], [232, 297], [230, 301], [231, 300]],
  vivian: [[218, 290], [227, 295], [227, 296], [224, 296]],
};

const EYE_Y = {
  claude: [196, 190, 183, 212],
  deepseek: [247, 230, 191, 229],
  nana: [207, 182, 196, 226],
  vivian: [215, 205, 213, 225],
};
const CHIN = { claude: [244, 242], deepseek: [285, 273], nana: [256, 228], vivian: [262, 244] };

// 分段线性反向采样：锚点严格对齐，同时保持各段内部连续。
function inverseAxis(source, target) {
  return Array.from({ length: CELL }, (_, value) => {
    let j = 0;
    while (j < target.length - 2 && value > target[j + 1]) j++;
    return source[j] + (value - target[j]) * (source[j + 1] - source[j]) / (target[j + 1] - target[j]);
  });
}

function landmark(frame, id, i) {
  const box = bounds(frame, CELL, CELL);
  let left = CELL, right = -1;
  // 此范围只含头部，排除下垂头发、尾巴、手臂和裙摆。
  for (let y = box.y0; y < EYE_Y[id][i] - 30; y++) for (let x = 0; x < CELL; x++) {
    if (frame[(y * CELL + x) * 4 + 3] > 200) { left = Math.min(left, x); right = Math.max(right, x); }
  }
  return { box, x: [left, ...EYES[id][i], right], eyeY: EYE_Y[id][i] };
}

await mkdir(root, { recursive: true });

function frameAt(data, width, i) {
  const out = Buffer.alloc(CELL * CELL * 4);
  for (let y = 0; y < CELL; y++) data.copy(out, y * CELL * 4, (y * width + i * CELL) * 4, (y * width + i * CELL + CELL) * 4);
  return out;
}

function putFrame(combined, width, i, frame) {
  for (let y = 0; y < CELL; y++) frame.copy(combined, (y * width + i * CELL) * 4, y * CELL * 4, (y + 1) * CELL * 4);
}

function alignFrame(frame, source, target, id, i) {
  const faceScale = (target.x[2] - target.x[1]) / (source.x[2] - source.x[1]);
  // 双眼必须位于同一个仿射段内，不能在虹膜中央改变缩放率。
  const sourceX = [source.x[0], source.x[1] - 12, source.x[2] + 12, source.x[3]];
  const targetX = [target.x[0], target.x[1] - 12 * faceScale, target.x[2] + 12 * faceScale, target.x[3]];
  let ys;
  if (i === 1) {
    const sourceY = [source.box.y0, source.eyeY - 12, source.eyeY + 12, CHIN[id][1], source.box.y1];
    const targetY = [target.box.y0, target.eyeY - 12 * faceScale, target.eyeY + 12 * faceScale, CHIN[id][0], target.box.y1];
    ys = inverseAxis(sourceY, targetY);
  } else {
    // 探出与藏低的幅度是动作本身：统一头顶到眼线的尺度和手部接触线，
    // 不强行把遮住下巴的帧拉成露出下巴的帧，也不在眼睛或手指中间分段。
    const headScaleY = (target.eyeY - target.box.y0) / (source.eyeY - source.box.y0);
    // DeepSeek 的鲸鱼帽较高，接触线下移以容纳完整头顶；运行时底边校准会补偿。
    const contact = id === 'deepseek' ? 280 : 256;
    ys = inverseAxis([source.box.y1 - 100, source.box.y1], [contact - 100 * headScaleY, contact]);
  }
  const xs = inverseAxis(sourceX, targetX);
  const out = Buffer.alloc(frame.length);
  for (let y = 0; y < CELL; y++) for (let x = 0; x < CELL; x++) {
    const sx = xs[x], sy = ys[y], x0 = Math.floor(sx), y0 = Math.floor(sy);
    const dx = sx - x0, dy = sy - y0;
    let alpha = 0;
    const rgb = [0, 0, 0];
    for (let oy = 0; oy <= 1; oy++) for (let ox = 0; ox <= 1; ox++) {
      const xx = x0 + ox, yy = y0 + oy;
      if (xx < 0 || xx >= CELL || yy < 0 || yy >= CELL) continue;
      const k = (yy * CELL + xx) * 4;
      const weight = (ox ? dx : 1 - dx) * (oy ? dy : 1 - dy) * frame[k + 3];
      alpha += weight;
      for (let c = 0; c < 3; c++) rgb[c] += weight * frame[k + c];
    }
    const k = (y * CELL + x) * 4;
    out[k + 3] = Math.round(alpha);
    if (alpha) for (let c = 0; c < 3; c++) out[k + c] = Math.round(rgb[c] / alpha);
  }
  cleanMatte(out, CELL, CELL);
  return out;
}

function checkFrame(frame, label, problems) {
  let visible = 0, opaque = 0, margin = 0, topEdge = 0, bottomEdge = 0, leftEdge = 0, rightEdge = 0;
  let mTop = 0, mBottom = 0, mLeft = 0, mRight = 0, mOpaque = 0, mFaint = 0;
  for (let y = 0; y < CELL; y++) for (let x = 0; x < CELL; x++) {
    const a = frame[(y * CELL + x) * 4 + 3];
    if ((x < 12 || x >= 500 || y < 12 || y >= 500) && a) { if (a > 200) mOpaque++; else mFaint++; }
    if (a) visible++;
    if (a > 128) opaque++;
    if ((x < 12 || x >= 500 || y < 12 || y >= 500) && a) {
      margin++;
      if (y < 12) mTop++;
      if (y >= 500) mBottom++;
      if (x < 12) mLeft++;
      if (x >= 500) mRight++;
    }
    if (a > 200) {
      if (y === 0) topEdge++;
      if (y === CELL - 1) bottomEdge++;
      if (x === 0) leftEdge++;
      if (x === CELL - 1) rightEdge++;
    }
  }
  if (margin) problems.push(`${label}: 12px 安全边距内 ${margin} 个非透明像素 上${mTop}/下${mBottom}/左${mLeft}/右${mRight}（实体 a>200: ${mOpaque}，柔边: ${mFaint}）`);
  if (visible >= CELL * CELL * 0.8) problems.push(`${label}: 可见像素超 80% 上限`);
  if (opaque <= 3000) problems.push(`${label}: 不透明像素不足 3000`);
  if (topEdge || bottomEdge || leftEdge || rightEdge) problems.push(`${label}: 内容被画布裁平 上${topEdge}/下${bottomEdge}/左${leftEdge}/右${rightEdge}`);
  return { visible, opaque };
}

const problems = [];
const report = [];
for (const id of Object.keys(EYES)) {
  const selected = process.argv.find(arg => arg.startsWith('--character='))?.split('=')[1];
  if (selected && selected !== id) continue;
  const publicPath = resolve(base, `public/chibi/motion/${id}/${id}-edge-bottom-sheet.webp`);
  const backupPath = resolve(root, `${id}-edge-bottom-original.webp`);
  try { await access(backupPath); } catch { await copyFile(publicPath, backupPath); }
  const { data, info } = await sharp(backupPath).ensureAlpha().raw().toBuffer({ resolveWithObject: true });
  if (info.width !== CELL * 4 || info.height !== CELL) problems.push(`${id}: 网格尺寸异常 ${info.width}x${info.height}`);
  const combined = Buffer.alloc(info.width * info.height * 4);
  const stats = [];
  const target = landmark(frameAt(data, info.width, 0), id, 0);
  for (let i = 0; i < 4; i++) {
    const original = frameAt(data, info.width, i);
    const source = landmark(original, id, i);
    const frame = i === 0 ? original : alignFrame(original, source, target, id, i);
    if (i === 0 && !frame.equals(original)) problems.push(`${id}: 第 1 帧被改动`);
    stats.push(checkFrame(frame, `${id}/帧${i}`, problems));
    putFrame(combined, info.width, i, frame);
  }
  report.push({ id, before: stats, output: publicPath, combined, width: info.width, height: info.height });
}

for (const r of report) {
  console.log(`${r.id.padEnd(10)}  ` + r.before.map((v, i) => `帧${i}: 可见${v.visible}/不透明${v.opaque}`).join('  '));
}
if (problems.length) {
  console.error('\n校验未通过：\n  ' + problems.join('\n  '));
  process.exitCode = 1;
} else {
  const temp = await mkdtemp(resolve(tmpdir(), 'chibi-edge-'));
  try {
    for (const r of report) {
      const keys = resolve(temp, `${r.id}-keys.png`);
      const output = resolve(temp, `${r.id}-sheet.webp`);
      await sharp(r.combined, { raw: { width: r.width, height: r.height, channels: 4 } }).png().toFile(keys);
      execFileSync('python', [resolve(scriptDirectory, 'interpolate.py'), keys, output], { stdio: 'inherit' });
      const pixels = await sharp(output).ensureAlpha().raw().toBuffer();
      for (let i = 0; i < 16; i++) {
        const row = Math.floor(i / 4);
        checkFrame(frameAt(pixels.subarray(row * 512 * 2048 * 4), 2048, i % 4), `${r.id}/插值帧${i}`, problems);
      }
    }
    if (problems.length) throw new Error(problems.join('\n'));
    if (!dry) for (const r of report) await copyFile(resolve(temp, `${r.id}-sheet.webp`), r.output);
  } finally { await rm(temp, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 }); }
  console.log(dry ? '\nDry run — 校验通过，未写 public/' : '\n校验通过，已写回 public/chibi/motion/*/*-edge-bottom-sheet.webp');
}
