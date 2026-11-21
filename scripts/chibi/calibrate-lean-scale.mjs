/**
 * 倚靠（`lean`）图集的逐帧尺度校准。
 *
 * 为什么单独一张表：`lean` 素材是「走到屏幕边、侧身靠住」的整段表演，画师/生成器
 * 往往换一个镜头尺度重画——vivian 这一套第 2 帧之后就比 idle 小约 6%，靠上去的
 * 瞬间人会「缩一下」。`frameScales` 里一直没有这张表，因为 `frameStyle` 把「左右
 * 镜像」和「尺寸校准」写在同一个 CSS `scale` 上，早年怕互相吃掉就把 edge/lean 整段
 * 排除了。现在渲染器会合成 `-f f`，可以正常校准了。
 *
 * 判定沿用 `calibrate-chibi-scales.mjs`：每帧取三个互不相同的尺度信号（脸宽、
 * 头顶到脚底、下巴到脚底），三者一致才认为这是「同一姿态的另一个镜头尺寸」；
 * 分歧说明是姿态变化（侧身、举手），保持原值。第 0 帧是入场帧，恒为 1。
 *
 * 为什么不用 `edge`（贴边探头）：那张图里人物本来就有大半在画面外，包围盒与人体
 * 比例都不成立，量出来的「尺寸」是构图而不是镜头。
 *
 * 逐帧而非整段统一：这张素材的偏差是台阶状的（入场帧正好是 idle，之后整段小 6%），
 * 统一系数会把入场帧放大，反而在进入倚靠时弹一下。
 *
 * 用法：
 *   node scripts/chibi/calibrate-lean-scale.mjs nana             # 写表
 *   node scripts/chibi/calibrate-lean-scale.mjs nana --dry       # 只出报告
 *   node scripts/chibi/calibrate-lean-scale.mjs chatgpt!         # 末尾 ! = 角色包
 */
import sharp from 'sharp';
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { bounds, faceBounds } from './frame-sheets/registration.mjs';

const CELL = 512;
const ORIGIN_PERCENT = 90;
const ORIGIN_PX = (ORIGIN_PERCENT / 100) * CELL;
const PERCENT_PX = CELL / 100;
/** 三个尺度信号允许的最大离散度；超过就算姿态变化，不是镜头变化。 */
const SPREAD_LIMIT = 0.08;
/** 偏差小于这个比例视为已对齐（包围盒噪声就是 2% 级）。 */
const DEAD_ZONE = 0.03;

const root = resolve(import.meta.dirname, '../..');
const args = process.argv.slice(2);
const dryRun = args.includes('--dry');
const targets = args.filter(argument => !argument.startsWith('--'));
if (targets.length === 0) throw new Error('指定角色：nana / claude! / chatgpt!（包以 ! 结尾）');

const readJson = async path => JSON.parse(await readFile(resolve(root, path), 'utf8'));
const boundaries = await readJson('src/chibi/animations.json');
const baseLean = boundaries.animations.find(spec => spec.name === 'lean');
if (!baseLean) throw new Error('animations.json 缺少 lean 声明');

async function landmarks(file, cols, index) {
  const pixels = await sharp(resolve(root, file))
    .extract({ left: (index % cols) * CELL, top: Math.floor(index / cols) * CELL, width: CELL, height: CELL })
    .ensureAlpha().raw().toBuffer();
  const box = bounds(pixels, CELL, CELL);
  let face = null;
  try { face = faceBounds(pixels, CELL, CELL, box); } catch { return null; }
  return { faceW: face.width, chin: face.y1, top: box.y0, foot: box.y1 };
}

const median = values => {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
};
const round = value => +value.toFixed(6);

const builtinScales = await readJson('src/chibi/frameScales.json');
const builtinAnchors = await readJson('src/chibi/frameAnchors.json');
let builtinChanged = false;
const report = [];

for (const target of targets) {
  const packaged = target.endsWith('!');
  const id = target.replace(/!$/, '');
  const packagePath = `character-packs/${id}/character.json`;
  const pack = packaged ? await readJson(packagePath) : null;
  const spec = packaged ? pack.animations?.lean : {...baseLean, ...baseLean.characterOverrides?.[id]};
  if (!spec || baseLean.excludedCharacters?.includes(id) || (!packaged && !baseLean.characters.includes(id))) {
    throw new Error(`${id}: 没有倚靠动作`);
  }
  const assetRoot = packaged ? `character-packs/${id}/assets` : 'public';

  const idleFile = id === 'nana'
    ? `${assetRoot}/chibi/${id}-atlas.webp`
    : `${assetRoot}/chibi/motion/${id}/${id}-idle-sheet.webp`;
  const rest = await landmarks(idleFile, id === 'nana' ? 3 : 1, 0);
  if (!rest) throw new Error(`${id}: 量不到 idle 的脸`);
  for (const direction of spec.directions ?? [null]) {
    const sheet = spec.sheet.replaceAll('{character}', id).replaceAll('{direction}', direction ?? 'left');
    const label = direction ? `${id}/${direction}` : id;
    await calibrateSheet(spec, sheet, assetRoot, rest, label, pack, packagePath);
  }
}

async function calibrateSheet(leanSpec, sheet, assetRoot, rest, id, pack, packagePath) {
  const packaged = pack !== null;
  const refHeight = rest.foot - rest.top;
  const refBody = rest.foot - rest.chin;
  const measured = [];
  for (let index = 0; index < leanSpec.frames; index++) {
    const frame = await landmarks(`${assetRoot}${sheet}`, leanSpec.cols, index);
    if (!frame) { measured.push(null); continue; }
    const ratios = [rest.faceW / frame.faceW, refHeight / (frame.foot - frame.top), refBody / (frame.foot - frame.chin)];
    const centre = median(ratios);
    measured.push({ ...frame, factor: centre, spread: (Math.max(...ratios) - Math.min(...ratios)) / centre });
  }
  const usable = measured.filter(entry => entry && entry.spread <= SPREAD_LIMIT);
  const factor = usable.length ? median(usable.map(entry => entry.factor)) : 1;
  report.push({ id, packaged, usable: usable.length, frames: leanSpec.frames, factor: +factor.toFixed(4), needsWork: Math.abs(factor - 1) > DEAD_ZONE });
  if (dryRun || Math.abs(factor - 1) <= DEAD_ZONE) return;

  const scales = packaged ? pack.scales : builtinScales;
  const anchors = packaged ? pack.anchors : builtinAnchors;
  scales[sheet] ??= Array.from({ length: leanSpec.frames }, () => [1, ORIGIN_PERCENT]);
  anchors[sheet] ??= Array.from({ length: leanSpec.frames }, () => [0, 0]);
  for (let index = 0; index < leanSpec.frames; index++) {
    // 入场帧就是 idle 本身，永远不缩放（否则靠上去的瞬间会弹一下）。
    if (index === 0) {
      scales[sheet][index] = [1, ORIGIN_PERCENT];
      anchors[sheet][index] = [0, 0];
      continue;
    }
    const entry = measured[index];
    const consistent = entry && entry.spread <= SPREAD_LIMIT;
    // 三个信号打架（脸被手挡住、侧脸）不等于「这一帧尺寸不同」：整段是同一个镜头，
    // 所以退回整段的估计值。退回 1 会在同一段里留下 6% 的凹陷，看起来像人物抽了一下。
    const applied = consistent ? entry.factor : factor;
    scales[sheet][index] = [round(applied), ORIGIN_PERCENT];
    if (!entry) {
      anchors[sheet][index] = [0, 0];
      continue;
    }
    const scaledFoot = (entry.foot - ORIGIN_PX) * applied + ORIGIN_PX;
    // 绕 90% 缩放会把脚整体往下推：用竖向锚点把脚钉回原位，横向不动（横向一挪就离开墙）。
    anchors[sheet][index] = [0, round((entry.foot - scaledFoot) / PERCENT_PX)];
  }
  if (packaged) await writeFile(resolve(root, packagePath), JSON.stringify(pack, null, 2) + '\n');
  else builtinChanged = true;
}

console.log('\ncharacter   frames  usable  factor   verdict');
for (const row of report) {
  console.log(`${(row.id + (row.packaged ? ' (pack)' : '')).padEnd(12)}${String(row.frames).padEnd(8)}${String(row.usable).padEnd(8)}${String(row.factor).padEnd(9)}${row.needsWork ? 'CALIBRATE' : 'aligned — keep'}`);
}
if (!dryRun && builtinChanged) {
  await writeFile(resolve(root, 'src/chibi/frameScales.json'), JSON.stringify(builtinScales, null, 2) + '\n');
  await writeFile(resolve(root, 'src/chibi/frameAnchors.json'), JSON.stringify(builtinAnchors, null, 2) + '\n');
  console.log('Wrote src/chibi/frameScales.json and src/chibi/frameAnchors.json');
}
if (dryRun) console.log('Dry run — nothing written.');
