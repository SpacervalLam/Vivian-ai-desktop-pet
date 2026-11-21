/**
 * 将人物明显偏大的或偏小的整段动作统一缩放到默认待机尺寸。
 * 脸宽、全高和身体高度三个信号须一致，低头、侧身或道具引起的变化保留。
 * 第 0 帧与默认待机图一致，保持原始缩放和位移。
 *
 * node scripts/calibrate-chibi-scales.mjs [character] [--dry]
 */
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import {
  COVERAGE, DEAD_ZONE, ORIGIN_PERCENT, PERCENT_PX, SPREAD_LIMIT,
  defaultIdleSheet, measureSpriteFrame, median, scaleSignal,
} from './chibi/scale-measurements.mjs';

const root = resolve(import.meta.dirname, '..');
const character = process.argv[2] && !process.argv[2].startsWith('--') ? process.argv[2] : 'claude';
const dryRun = process.argv.includes('--dry');

const readJson = async path => JSON.parse(await readFile(resolve(root, path), 'utf8'));
const vocab = await readJson('src/chibi/animations.json');
const anchors = await readJson('src/chibi/frameAnchors.json');
const scales = await readJson('src/chibi/frameScales.json');

const ORIGIN_PX = ORIGIN_PERCENT * PERCENT_PX;
/**
 * `frameStyle` 用 `style.scale` 同时承载「left 方向水平翻转」(-1 1) 与尺寸校准。
 * 给这些动作写 scale 会静默吃掉翻转，所以整段排除，别去碰它们。
 */
const FLIP_SHEETS = new Set(['edge', 'lean']);

const { url: idleUrl, columns: idleColumns } = defaultIdleSheet(character);
const rest = await measureSpriteFrame(resolve(root, 'public' + idleUrl), idleColumns, 0);
if (!rest) throw new Error(`无法测量 ${character} 的默认 idle 人脸`);
const refHeight = rest.foot - rest.top;
const refBody = rest.foot - rest.chin;

const round = value => +value.toFixed(6);

const report = [];
const handledUrls = new Set();
let changed = 0;
for (const spec of vocab.animations) {
  if (spec.characters && !spec.characters.includes(character)) continue;
  for (const direction of spec.directions ?? [null]) {
    const url = spec.sheet.split('{character}').join(character).split('{direction}').join(direction ?? 'left');
    // thinking / think 共用同一张 sheet，按 URL 去重，否则会被校准两遍。
    if (handledUrls.has(url)) continue;
    handledUrls.add(url);
    const perFrame = [];
    const file = resolve(root, 'public' + url);
    for (let i = 0; i < spec.frames; i++) {
      const frame = await measureSpriteFrame(file, spec.cols, i);
      perFrame.push(frame ? { ...frame, ...scaleSignal(frame, rest, refHeight) } : null);
    }
    const usable = perFrame.filter(f => f && f.spread <= SPREAD_LIMIT);
    const measurable = !FLIP_SHEETS.has(spec.name)
      && usable.length >= Math.ceil(spec.frames * COVERAGE);
    const factor = measurable ? median(usable.map(f => f.ratio)) : null;
    const before = scales[url]?.[1]?.[0] ?? 1;
    // 比较已应用的缩放，重复校准同一套素材时只更新残差超出容限的动作。
    const needsWork = factor !== null && Math.abs(factor - before) > DEAD_ZONE;
    const alreadyApplied = factor !== null && before !== 1
      && Math.abs(factor - before) <= DEAD_ZONE;
    report.push({
      name: spec.name + (direction ? ':' + direction : ''), url,
      frames: spec.frames, usable: usable.length,
      factor: factor === null ? null : +factor.toFixed(4),
      before, verdict: needsWork ? 'CALIBRATE'
        : (alreadyApplied ? 'already applied — keep'
          : (factor === null ? (FLIP_SHEETS.has(spec.name) ? 'flip — keep' : 'pose/prop — keep') : 'aligned — keep')),
    });
    if (!needsWork) continue;

    anchors[url] ??= Array.from({ length: spec.frames }, () => [0, 0]);
    scales[url] ??= Array.from({ length: spec.frames }, () => [1, ORIGIN_PERCENT]);
    for (let i = 0; i < spec.frames; i++) {
      // 入场帧就是 idle 图本身，必须保持未缩放。
      if (i === 0) {
        anchors[url][i] = [0, 0];
        scales[url][i] = [1, ORIGIN_PERCENT];
        continue;
      }
      const m = perFrame[i];
      if (!m) continue;  // 无可测人脸的帧（躺卧/露头）保持原值
      const scaledFoot = (m.foot - ORIGIN_PX) * factor + ORIGIN_PX;
      const scaledCx = (m.faceCx - 256) * factor + 256;
      anchors[url][i] = [
        round((rest.faceCx - scaledCx) / PERCENT_PX),
        round((rest.foot - scaledFoot) / PERCENT_PX),
      ];
      scales[url][i] = [round(factor), ORIGIN_PERCENT];
    }
    changed++;
  }
}

console.log(`\n${character}: 基准 idle  faceW=${rest.faceW}  top=${rest.top}  chin=${rest.chin}  foot=${rest.foot}  height=${refHeight}  body=${refBody}`);
console.log('sheet'.padEnd(20) + 'frames'.padEnd(8) + 'usable'.padEnd(8) + 'factor'.padEnd(9) + 'before'.padEnd(9) + 'verdict');
for (const r of report) {
  console.log(r.name.padEnd(20) + String(r.frames).padEnd(8) + String(r.usable).padEnd(8) +
    String(r.factor ?? '-').padEnd(9) + String(r.before).padEnd(9) + r.verdict);
}
console.log(`\n${changed} sheet(s) need calibration.`);

if (!dryRun && changed) {
  await writeFile(resolve(root, 'src/chibi/frameAnchors.json'), JSON.stringify(anchors, null, 2) + '\n');
  await writeFile(resolve(root, 'src/chibi/frameScales.json'), JSON.stringify(scales, null, 2) + '\n');
  console.log('Wrote src/chibi/frameAnchors.json and src/chibi/frameScales.json');
} else if (dryRun) {
  console.log('Dry run — no files written.');
}
