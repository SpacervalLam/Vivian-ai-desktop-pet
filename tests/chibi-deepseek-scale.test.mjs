/**
 * deepseek 默认状态 vs 全部动作帧：尺寸校准验收。
 *
 * 背景（2026-11-08）：用户报「deepseek 默认状态和其它动画尺寸不一样」。
 * 实测结论：人物本体（脸宽、下巴→脚底、脚底 y）在所有站立动作里都与 idle 对齐，
 * 肉眼看到的「变大」来自 v4 源图把鲸鱼帽的触角画高 11~31px、两侧头发画得更开
 * （面积 +5~22%）。这是轮廓美术差异，不是整段镜头尺度。
 *
 * 关键反例：若按包围盒高度把 angry/look 缩放 0.93 去对齐帽顶，脸和身体会一起缩小
 * 7%，反而比 idle 小。本测试用一个「错误校准」A/B 对照组证明判据真的带电量，
 * 避免以后有人为了消除视觉差异误写 frameScales.json。
 *
 * 与 scripts/check-chibi-scale.mjs 的差别：那个脚本只查 14 张站立表（STANDING），
 * 这里读取 DeepSeek 角色包的全部动作配置，补上 walk/turn/
 * float/sleep/wake/umbrella/edge 等被跳过的部分。
 */
import assert from 'node:assert/strict';
import sharp from 'sharp';
import { readFile } from 'node:fs/promises';
import { characterAssetPath } from './helpers/character-assets.mjs';
import { resolve } from 'node:path';
import { bounds, faceBounds } from '../scripts/chibi/frame-sheets/registration.mjs';

const root = resolve(import.meta.dirname, '..');
const readJson = async p => JSON.parse(await readFile(resolve(root, p), 'utf8'));
const pack = await readJson('character-packs/deepseek/character.json');
const entryStates = await readJson('src/chibi/entryStates.json');
const { scales, anchors } = pack;

const CHARACTER = 'deepseek';
const IDLE = `/chibi/motion/${CHARACTER}/${CHARACTER}-idle-sheet.webp`;
const SPREAD_LIMIT = 0.08;   // 三个尺度信号离散度上限
const COVERAGE = 0.6;        // 至少 60% 帧一致才算「整段」尺度
const DEAD_ZONE = 0.03;      // 需要的 factor 与已应用 scale 的残差容限
const CORE_TOLERANCE = 0.05; // 人物本体（下巴→脚底）允许偏差
const FOOT_TOLERANCE = 3;    // 脚底漂移（格内像素）
const NAIVE_SCALE = 0.93;    // 按 angry/look 帽顶高度推出的「错误校准」因子

const median = xs => {
  const s = [...xs].filter(Number.isFinite).sort((a, b) => a - b);
  if (!s.length) return NaN;
  const m = Math.floor(s.length / 2);
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};

async function measure(url, cols, i) {
  const { data } = await sharp(characterAssetPath('public' + url))
    .extract({ left: (i % cols) * 512, top: Math.floor(i / cols) * 512, width: 512, height: 512 })
    .ensureAlpha().raw().toBuffer({ resolveWithObject: true });
  const box = bounds(data, 512, 512);
  let area = 0;
  for (let k = 3; k < data.length; k += 4) if (data[k] > 200) area++;
  let face = null;
  try { face = faceBounds(data, 512, 512, box); } catch { /* 侧身/遮挡帧测不到脸 */ }
  return {
    top: box.y0, foot: box.y1, height: box.height, width: box.width, area,
    faceW: face?.width ?? NaN, chin: face?.y1 ?? NaN,
  };
}

// 与 motionRegistry.frameStyle 相同的坐标变换
const project = (m, a, s) => {
  const scale = s?.[0] ?? 1, origin = (s?.[1] ?? 90) * 5.12;
  const ay = a?.[1] ?? 0;
  const y = v => (v - origin) * scale + origin + ay * 5.12;
  return { foot: y(m.foot), body: y(m.foot) - y(m.chin), faceW: m.faceW * scale };
};

const rest = await measure(IDLE, 1, 0);
assert.ok(Number.isFinite(rest.chin), 'idle 必须能测到面部地标');
const refBody = rest.foot - rest.chin;

const STANDING = new Set(['blink', 'cast', 'happy', 'angry', 'think', 'thinking', 'smug',
  'busy-in', 'busy-loop', 'remind', 'look', 'tired', 'music']);

const sheets = [];
const seen = new Set();
for (const spec of Object.values(pack.animations)) {
  for (const direction of spec.directions ?? [null]) {
    const url = spec.sheet.split('{character}').join(CHARACTER).split('{direction}').join(direction ?? 'left');
    if (seen.has(url)) continue;
    seen.add(url);
    const meta = await sharp(characterAssetPath('public' + url)).metadata();
    assert.equal(meta.width, spec.cols * 512, `${url}: 列数与词汇表不符`);
    assert.equal(meta.height, spec.rows * 512, `${url}: 行数与词汇表不符`);

    const frames = [];
    const factors = [];
    const coreDeltas = [], footDeltas = [];
    const naiveBodyDeltas = [], naiveFaceDeltas = [];
    for (let i = 0; i < spec.frames; i++) {
      const m = await measure(url, spec.cols, i);
      frames.push(m);
      const ratios = [rest.faceW / m.faceW, rest.height / (m.foot - m.top), refBody / (m.foot - m.chin)]
        .filter(Number.isFinite);
      const mid = median(ratios);
      if (ratios.length === 3 && (Math.max(...ratios) - Math.min(...ratios)) / mid <= SPREAD_LIMIT) {
        factors.push(mid);
      }
      const applied = project(m, anchors[url]?.[i], scales[url]?.[i]);
      coreDeltas.push(Math.abs(applied.body - refBody) / refBody);
      footDeltas.push(Math.abs(applied.foot - rest.foot));
      // 错误校准对照组：把这一帧按 0.93 缩放（origin 90%，与表内一致）
      const naive = project(m, anchors[url]?.[i], [NAIVE_SCALE, 90]);
      naiveBodyDeltas.push(Math.abs(naive.body - refBody) / refBody);
      naiveFaceDeltas.push(Math.abs(naive.faceW - rest.faceW) / rest.faceW);
    }
    sheets.push({
      name: spec.name, direction, url, frames: spec.frames, data: frames,
      coherent: factors.length,
      needed: factors.length >= Math.ceil(spec.frames * COVERAGE) ? median(factors) : null,
      applied: scales[url]?.[1]?.[0] ?? 1,
      core: median(coreDeltas), foot: Math.max(...footDeltas),
      naiveCore: median(naiveBodyDeltas), naiveFace: median(naiveFaceDeltas),
      standing: STANDING.has(spec.name),
    });
  }
}

// 1) 入场帧必须是 idle 的同一轮廓（WebP 二次编码允许 <1px / <2% 面积差）
for (const s of sheets) {
  if (!entryStates.idle.includes(s.name)) continue;
  const f0 = s.data[0];
  assert.ok(Math.abs(f0.top - rest.top) <= 1 && Math.abs(f0.foot - rest.foot) <= 1 &&
    Math.abs(f0.height - rest.height) <= 1 && Math.abs(f0.width - rest.width) <= 1,
    `${s.name}: 入场帧 f0 与默认 idle 轮廓不一致`);
  assert.ok(Math.abs(f0.area - rest.area) / rest.area < 0.02,
    `${s.name}: 入场帧 f0 不是默认 idle 图`);
}

// 2) 全部 sheet：不存在「未校准的整段镜头尺度」（含 checker 跳过的非站立表）
for (const s of sheets) {
  // Prop occlusion changes the measured face and silhouette. Entry alignment for
  // these motions is checked separately in chibi-entry-states and umbrella registration.
  if (s.needed === null || entryStates.props.includes(s.name)) continue;
  const residual = Math.abs(s.needed - s.applied) / s.needed;
  assert.ok(residual <= DEAD_ZONE,
    `${s.name}${s.direction ? '/' + s.direction : ''}: 需要 scale ${s.needed.toFixed(4)}，` +
    `表内为 ${s.applied}，残差 ${(residual * 100).toFixed(1)}%`);
}

// 3) 人物本体与脚底：站立动作必须与 idle 对齐（这才是「尺寸一致」的定义）
const standing = sheets.filter(s => s.standing);
for (const s of standing) {
  assert.ok(s.core <= CORE_TOLERANCE,
    `${s.name}: 下巴→脚底偏差 ${(s.core * 100).toFixed(1)}%，人物本体被缩放了`);
  assert.ok(s.foot <= FOOT_TOLERANCE,
    `${s.name}: 脚底漂移 ${s.foot.toFixed(1)}px`);
}

// 4) A/B 对照组：按帽顶高度做 0.93 缩放会同时缩小脸和身体 —— 证明判据带电，
//    也说明这种「校准」是错的，别写进 frameScales.json。
const naiveVictims = standing.filter(s => ['angry', 'look', 'cast', 'smug'].includes(s.name));
assert.ok(naiveVictims.length >= 3, '对照组必须覆盖多张表');
for (const s of naiveVictims) {
  assert.ok(s.naiveCore > CORE_TOLERANCE,
    `${s.name}: 0.93 缩放后人物本体偏差 ${(s.naiveCore * 100).toFixed(1)}% 应超过 ` +
    `${(CORE_TOLERANCE * 100).toFixed(0)}% 容限`);
  assert.ok(s.naiveCore - s.core >= 0.03,
    `${s.name}: 0.93 缩放必须让本体明显更偏离 idle（${(s.core * 100).toFixed(1)}% → ` +
    `${(s.naiveCore * 100).toFixed(1)}%），否则判据失去灵敏度`);
}

console.log(`deepseek 尺寸校准：${sheets.length} 张 sheet / ${sheets.reduce((n, s) => n + s.frames, 0)} 帧全部覆盖；` +
  `站立表 ${standing.length} 张，人物本体最大偏差 ${(Math.max(...standing.map(s => s.core)) * 100).toFixed(1)}%，` +
  `脚底最大漂移 ${Math.max(...standing.map(s => s.foot)).toFixed(1)}px；` +
  `错误缩放对照组 ${naiveVictims.length} 张全部如预期失效。`);
console.log('结论：默认状态与动作帧的人物尺度一致；帽顶/头发轮廓差异属美术，不使用 frameScales 校准。');
