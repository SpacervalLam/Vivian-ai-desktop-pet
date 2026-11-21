/**
 * 按渲染器的坐标变换验收校准后的尺寸与脚底位置。
 * 脸宽、全高和身体高度有一致的缩放信号时，才判断该动作的整段尺度。
 * 高度、脸宽和脚底的最大偏差也会打印，便于复核。
 *
 * node scripts/check-chibi-scale.mjs [character] [--baseline <json>] [--anchors <json>]
 */
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import {
  COVERAGE, DEAD_ZONE, ORIGIN_PERCENT, PERCENT_PX, SPREAD_LIMIT,
  defaultIdleSheet, measureSpriteFrame, median, scaleSignal,
} from './chibi/scale-measurements.mjs';

const root = resolve(import.meta.dirname, '..');
const args = process.argv.slice(2);
const character = args.find(a => !a.startsWith('--') && !['--baseline', '--anchors'].includes(a)) ?? 'claude';
const readJson = async path => JSON.parse(await readFile(resolve(root, path), 'utf8'));
const vocab = await readJson('src/chibi/animations.json');
const anchors = await readJson('src/chibi/frameAnchors.json');
const scales = await readJson('src/chibi/frameScales.json');
const baselineFlag = args.indexOf('--baseline');
const beforeScales = baselineFlag >= 0 ? JSON.parse(await readFile(resolve(root, args[baselineFlag + 1]), 'utf8')) : null;
const anchorsFlag = args.indexOf('--anchors');
const beforeAnchors = anchorsFlag >= 0 ? JSON.parse(await readFile(resolve(root, args[anchorsFlag + 1]), 'utf8')) : null;

const { url: idleUrl, columns: idleColumns } = defaultIdleSheet(character);
const rest = await measureSpriteFrame(resolve(root, 'public' + idleUrl), idleColumns, 0);
if (!rest) throw new Error(`无法测量 ${character} 的默认 idle 人脸`);
const refBody = rest.foot - rest.chin;

const project = (m, a, s) => {
  const scale = s?.[0] ?? 1;
  const origin = (s?.[1] ?? ORIGIN_PERCENT) * PERCENT_PX;
  const offsetY = (a?.[1] ?? 0) * PERCENT_PX;
  const y = v => (v - origin) * scale + origin + offsetY;
  return {
    height: y(m.foot) - y(m.top),
    faceW: m.faceW * scale,
    foot: y(m.foot),
  };
};

// 只统计正面站立帧：侧身/躺卧/道具帧的地标不代表人物尺度，校准脚本也不动它们。
const STANDING = new Set(['blink', 'cast', 'happy', 'angry', 'think', 'thinking', 'smug',
  'busy-in', 'busy-loop', 'remind', 'look', 'tired', 'music', 'clipboard']);
const rows = [];
const seen = new Set();
for (const spec of vocab.animations) {
  if (!STANDING.has(spec.name)) continue;
  if (spec.characters && !spec.characters.includes(character)) continue;
  const url = spec.sheet.split('{character}').join(character).split('{direction}').join('left');
  if (seen.has(url)) continue;
  seen.add(url);
  const diffs = [], before = [], coherent = [];
  const file = resolve(root, 'public' + url);
  for (let i = 0; i < spec.frames; i++) {
    const m = await measureSpriteFrame(file, spec.cols, i);
    if (!m) continue;
    const after = project(m, anchors[url]?.[i], scales[url]?.[i]);
    diffs.push({
      height: (after.height - rest.height) / rest.height,
      faceW: (after.faceW - rest.faceW) / rest.faceW,
      foot: after.foot - rest.foot,
    });
    if (beforeScales) {
      const prior = project(m, beforeAnchors?.[url]?.[i], beforeScales[url]?.[i]);
      before.push({
        height: (prior.height - rest.height) / rest.height,
        faceW: (prior.faceW - rest.faceW) / rest.faceW,
        foot: prior.foot - rest.foot,
      });
    }
    const { ratio, spread } = scaleSignal(m, rest, rest.height);
    if (spread <= SPREAD_LIMIT) coherent.push(ratio);
  }
  if (!diffs.length) continue;
  const worst = key => Math.max(...diffs.map(d => Math.abs(d[key])));
  const worstBefore = key => before.length ? Math.max(...before.map(d => Math.abs(d[key]))) : null;
  const isWholeSheet = coherent.length >= Math.ceil(spec.frames * COVERAGE);
  const needed = isWholeSheet ? median(coherent) : null;
  const applied = scales[url]?.[1]?.[0] ?? 1;
  const residual = needed === null ? null : Math.abs(needed - applied) / needed;
  rows.push({
    name: spec.name, frames: diffs.length, coherent: coherent.length,
    needed, applied, residual,
    verdict: needed === null ? 'pose/art' : (residual > DEAD_ZONE ? 'SCALE MISMATCH' : 'aligned'),
    height: worst('height'), faceW: worst('faceW'), foot: worst('foot'),
    beforeHeight: worstBefore('height'), beforeFaceW: worstBefore('faceW'), beforeFoot: worstBefore('foot'),
  });
}

const pct = v => v === null ? '-' : (v * 100).toFixed(1) + '%';
const num = v => v === null ? '-' : v.toFixed(4);
console.log(`\n${character}: 基准 idle  h=${rest.height} faceW=${rest.faceW} body=${refBody} foot=${rest.foot}`);
console.log(`${character}: 渲染后各站立帧相对默认 idle 的最大偏差（h/w/foot 为原始偏差，仅供参考）`);
console.log('sheet'.padEnd(14) + 'sig'.padEnd(8) + 'needed'.padEnd(9) + 'applied'.padEnd(9) +
  'residual'.padEnd(10) + 'verdict'.padEnd(16) +
  (beforeScales ? 'before h/w/foot'.padEnd(26) : '') + 'after h/w/foot');
for (const r of rows) {
  console.log(r.name.padEnd(14) + `${r.coherent}/${r.frames}`.padEnd(8) +
    num(r.needed).padEnd(9) + num(r.applied).padEnd(9) +
    pct(r.residual).padEnd(10) + r.verdict.padEnd(16) +
    (beforeScales ? `${pct(r.beforeHeight)} / ${pct(r.beforeFaceW)} / ${r.beforeFoot?.toFixed(1)}px`.padEnd(26) : '') +
    `${pct(r.height)} / ${pct(r.faceW)} / ${r.foot.toFixed(1)}px`);
}

// 只有「整段镜头尺度没校准」或「脚底被挪走」才算回归；姿态/装饰变化不算。
const offenders = rows.filter(r => r.verdict === 'SCALE MISMATCH' || r.foot > 3);
const poseOnly = rows.filter(r => r.height > 0.05 && r.verdict !== 'SCALE MISMATCH');
if (poseOnly.length) {
  console.log(`\n说明: ${poseOnly.map(r => r.name).join(', ')} 的包围盒高度偏差 >5%，` +
    `但下巴/身体/脚底稳定，属于帽子、头发或姿态变化，不是人物尺度问题。`);
}
if (offenders.length) {
  console.error('\nFAIL: 仍有未校准的整段尺度或脚底漂移 >3px:', offenders.map(r => r.name).join(', '));
  process.exitCode = 1;
} else {
  console.log('\nPASS: 无未校准的整段镜头尺度偏差，脚底漂移均在 3px 内。');
}
