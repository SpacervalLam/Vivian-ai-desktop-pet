/**
 * 生成 src/chibi/defaultSize.json，将角色待机图缩放到平均高度并对齐脚底。
 * 变换施加在精灵的外层容器，对待机和动作帧生效。
 * 脚底投影公式为 O + T + S * (foot - O)，其中 O 是缩放原点，T 是纵向位移。
 *
 * node scripts/calibrate-chibi-default-size.mjs [--dry]
 */
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { bounds } from './chibi/frame-sheets/registration.mjs';
import {
  CELL_SIZE, ORIGIN_PERCENT, PERCENT_PX, defaultIdleSheet, readSpriteFrame,
} from './chibi/scale-measurements.mjs';

const root = resolve(import.meta.dirname, '..');
const dryRun = process.argv.includes('--dry');
const readJson = async path => JSON.parse(await readFile(resolve(root, path), 'utf8'));
const characters = (await readJson('src/chibi/animations.json')).characters;

const ORIGIN = ORIGIN_PERCENT / 100 * CELL_SIZE;
const TARGET_FOOT = 480;

const measured = {};
for (const id of characters) {
  const { url, columns } = defaultIdleSheet(id);
  const pixels = await readSpriteFrame(resolve(root, 'public' + url), columns, 0);
  const box = bounds(pixels, CELL_SIZE, CELL_SIZE);
  measured[id] = {
    top: box.y0,
    foot: box.y1,
    // 用 top→foot 的坐标跨度，而不是 bounds().height（后者是 +1 的像素计数）。
    height: box.y1 - box.y0,
    width: box.x1 - box.x0,
  };
}

const targetHeight = Object.values(measured).reduce((sum, m) => sum + m.height, 0) / characters.length;
const target = {
  top: +(TARGET_FOOT - targetHeight).toFixed(2),
  foot: TARGET_FOOT,
  height: +targetHeight.toFixed(2),
};

const transform = {};
for (const [id, m] of Object.entries(measured)) {
  const scale = target.height / m.height;
  const offset = TARGET_FOOT - ORIGIN - scale * (m.foot - ORIGIN);
  transform[id] = {
    scale: +scale.toFixed(6),
    translateY: +(offset / PERCENT_PX).toFixed(6),
  };
  const project = y => ORIGIN + offset + scale * (y - ORIGIN);
  measured[id].projected = {
    top: +project(m.top).toFixed(2),
    foot: +project(m.foot).toFixed(2),
  };
}

console.log(`\n目标：top=${target.top}  foot=${target.foot}  height=${target.height}（512 格内）`);
console.log('角色'.padEnd(11) + '原始 top..foot'.padEnd(20) + '原始高'.padEnd(10) +
  'scale'.padEnd(11) + 'translateY'.padEnd(13) + '归一后 top..foot');
for (const id of characters) {
  const m = measured[id], t = transform[id];
  console.log(id.padEnd(11) +
    `${m.top}..${m.foot}`.padEnd(20) +
    String(m.height).padEnd(10) +
    String(t.scale).padEnd(11) +
    `${t.translateY}%`.padEnd(13) +
    `${m.projected.top}..${m.projected.foot}`);
}

const config = {
  note: '四个角色默认 idle 素材尺寸不一致，渲染时由 .chibi-pet-default-size 包裹层统一缩放 + 对齐脚底。由 scripts/calibrate-chibi-default-size.mjs 依据 public/ 素材实测生成，勿手改数值；改素材后需重跑本脚本。',
  originPercent: ORIGIN_PERCENT,
  target,
  measured,
  characters: transform,
};

if (dryRun) {
  console.log('\nDry run — no files written.');
} else {
  await writeFile(resolve(root, 'src/chibi/defaultSize.json'), JSON.stringify(config, null, 2) + '\n');
  console.log('\nWrote src/chibi/defaultSize.json');
}
