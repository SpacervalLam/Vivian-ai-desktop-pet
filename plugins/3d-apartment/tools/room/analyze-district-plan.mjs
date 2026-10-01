/**
 * 街区平面设计分析（只读，纯数学，不开浏览器）
 *
 * 目的：把 urbanStreets.ts 里的 CITY_ROADS / CITY_LOTS 原样解析出来，
 * 推导出「肉眼能看出什么」的硬数字：
 *   1. 每个 block 的临街面覆盖率（frontage coverage）
 *   2. 天际线仰角剖面（从几个视点扫方位角）
 *   3. 高度直方图 / 是否平台化
 *   4. 石材配色的周期性与相邻重复率
 *
 * 数据来源：直接正则解析 plugins/3d-apartment/src/anime/urbanStreets.ts，
 * 不复制常量，避免和源码漂移。
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../../../../plugins/3d-apartment/src/anime/urbanStreets.ts');
const src = readFileSync(SRC, 'utf8');

// ---- 1. 从源码里抠出常量 ---------------------------------------------------
function grab(re, source = src) {
  const m = source.match(re);
  if (!m) throw new Error('pattern not found: ' + re);
  return m[1];
}
const ewRaw = grab(/eastWest:\s*(\[[^\]]*\][\s\S]*?\.map\(z=>\(\{z,width:[^)]*\)\))/);
const nsRaw = grab(/northSouth:\s*(\[[^\]]*\])/);
const boundsRaw = grab(/bounds:\s*(\{[^}]*\})/);

const CITY_ROADS = {
  eastWest: eval(ewRaw),                              // eslint-disable-line no-eval
  northSouth: eval(nsRaw),                            // eslint-disable-line no-eval
  bounds: eval('(' + boundsRaw + ')'),                // eslint-disable-line no-eval
};

// 地块表改成「求值源码块」而不是抠字面量：现在的生成器里有 floorsAt()/hash 函数，
// 正则抠不出来。块内是纯算术、不依赖 THREE，剥掉类型标注就能直接跑——单一事实来源。
const START = '/* --- 地块表开始';
const END = '/* --- 地块表结束 --- */';
const i0 = src.indexOf(START), i1 = src.indexOf(END);
if (i0 < 0 || i1 < 0) throw new Error('地块表标记缺失，检查 urbanStreets.ts 的 START/END 注释');
const block = src.slice(i0, i1)
  .replace(/\/\*[\s\S]*?\*\//g, '')
  .replace(/\/\/[^\n]*/g, '')
  .replace(/\bexport\s+/g, '')
  .replace(/:\s*(?:CityLot\[\]|CityLot|number|string|boolean)/g, '')
  .replace(/([A-Za-z_$][\w$]*)\?(?=\s*[,)])/g, '$1');   // 可选参数 floors?
const CITY_LOTS = new Function(`${block}; return CITY_LOTS;`)();  // eslint-disable-line no-new-func

const xCells = JSON.parse(grab(/const xCells=(\[\[[\s\S]*?\]\]);/));

const FLOOR_H = 2.8;
const line = (s) => console.log(s);
const pad = (s, n) => String(s).padEnd(n);

line('═'.repeat(78));
line('街区平面分析 — 数据解析自 urbanStreets.ts');
line('═'.repeat(78));
line(`CITY_ROADS.eastWest  z = ${CITY_ROADS.eastWest.map(r => r.z).join(', ')}`);
line(`                     宽 = ${CITY_ROADS.eastWest.map(r => r.width).join(', ')}`);
line(`CITY_ROADS.northSouth x = ${CITY_ROADS.northSouth.map(r => `${r.x}(w${r.width})`).join(', ')}`);
line(`bounds x∈[${CITY_ROADS.bounds.x0}, ${CITY_ROADS.bounds.x1}]  z∈[${CITY_ROADS.bounds.z0}, ${CITY_ROADS.bounds.z1}]`);
line(`CITY_LOTS 共 ${CITY_LOTS.length} 块：parcel ${CITY_LOTS.filter(l => l.id.startsWith('parcel')).length} + side ${CITY_LOTS.filter(l => l.id.startsWith('side')).length}`);

// ---- 2. block 网格 --------------------------------------------------------
// 竖直路把 x 切成三段（源码里的 xCells），横向路之间是 row。
line('');
line('─'.repeat(78));
line('A. block 网格 与 临街面覆盖率');
line('─'.repeat(78));
line(`xCells（竖直路之间）: ${xCells.map(c => `[${c[0]},${c[1]}]`).join('  ')}`);
for (const c of xCells) line(`   宽 ${pad(c[1] - c[0], 5)} m   ${c[0]} → ${c[1]}`);
line('');

const rows = [];
for (let i = 0; i < CITY_ROADS.eastWest.length - 1; i++) {
  const a = CITY_ROADS.eastWest[i], b = CITY_ROADS.eastWest[i + 1];
  rows.push({ i, z0: a.z + a.width / 2, z1: b.z - b.width / 2, depth: b.z - b.width / 2 - (a.z + a.width / 2) });
}
line(`横向 block 行（road 之间）共 ${rows.length} 行：`);
for (const r of rows) line(`   行${r.i}  z∈[${pad(r.z0, 7)}, ${pad(r.z1, 7)}]  进深 ${r.depth.toFixed(1)} m`);

line('');
line('每行每格的建筑占位（只算 parcel/side，不含公寓/便利店等手工件）：');
const COVER = [];
for (const r of rows) {
  for (let ci = 0; ci < xCells.length; ci++) {
    const [x0, x1] = xCells[ci];
    // 落在这一格、这一行内的 lot
    const inCell = CITY_LOTS.filter(l => l.x - l.w / 2 >= x0 - 0.01 && l.x + l.w / 2 <= x1 + 0.01 && l.z >= r.z0 - 0.01 && l.z <= r.z1 + 0.01);
    const frontage = inCell.reduce((s, l) => s + l.w, 0);
    const span = x1 - x0;
    // 最大的连续空档
    const sorted = inCell.slice().sort((a, b) => a.x - b.x);
    let gap = x0, gapEnd = x0, best = 0, bestRange = null, cursor = x0;
    for (const l of sorted) { if (l.x - l.w / 2 - cursor > best) { best = l.x - l.w / 2 - cursor; bestRange = [cursor, l.x - l.w / 2]; } cursor = l.x + l.w / 2; }
    if (x1 - cursor > best) { best = x1 - cursor; bestRange = [cursor, x1]; }
    COVER.push({ row: r.i, cell: ci, x0, x1, span, n: inCell.length, frontage, cov: frontage / span, maxGap: best, gapRange: bestRange, ids: inCell.map(l => l.id) });
    line(`  行${r.i} 格${ci} x∈[${pad(x0, 4)},${pad(x1, 4)}] 宽${pad(span.toFixed(0), 4)}m  ${inCell.length} 栋 / 面宽 ${pad(frontage.toFixed(0), 3)}m  → 覆盖率 ${pad((frontage / span * 100).toFixed(0) + '%', 5)}  最大连续空档 ${best.toFixed(1)}m ${bestRange ? `[${bestRange[0].toFixed(1)}, ${bestRange[1].toFixed(1)}]` : ''}`);
  }
}

// 行 3 中央的临街面还有五个手工件（exterior.ts 里硬编码世界坐标，不在 CITY_LOTS 里）。
// 数字取自 check-district-overlaps.mjs 在真实场景里量到的组 AABB。这里手工登记一份，
// 否则覆盖率会把它们算成空档，看起来比实际差得多。
//
// ⚠ parking-lot 是**地面**用地（露天停车场），不是体量——但它同样铺满块深
// （z 14.0~25.6，块 12.6~26.7），任何落在它上面的地块都会插进停车场。
// 早先漏登记它，导致「行 3 西侧留 14.8m 洞」的判断出错，补了两栋楼上去。
const HAND_ROW3 = [
  { id: 'subway-entrance', x0: -29.82, x1: -18.89 },
  { id: 'parking-lot', x0: -19.0, x1: -5.6 },
  { id: 'convenience-store', x0: -4.13, x1: 6.32 },
  { id: 'izakaya', x0: 6.85, x1: 12.48 },
  { id: 'street-bookstore', x0: 12.2, x1: 19.9 },
];
{
  const [x0, x1] = xCells[1];
  const lots = CITY_LOTS.filter((l) => l.z === 19.6);
  const pieces = [
    ...lots.map((l) => ({ id: l.id, x0: l.x - l.w / 2, x1: l.x + l.w / 2 })),
    ...HAND_ROW3,
  ].sort((a, b) => a.x0 - b.x0);
  let cursor = x0, gap = 0, gapAt = null, covered = 0;
  for (const p of pieces) {
    if (p.x0 > cursor) { if (p.x0 - cursor > gap) { gap = p.x0 - cursor; gapAt = [cursor, p.x0]; } }
    covered += Math.max(0, Math.min(p.x1, x1) - Math.max(p.x0, x0));
    cursor = Math.max(cursor, p.x1);
  }
  if (x1 - cursor > gap) { gap = x1 - cursor; gapAt = [cursor, x1]; }
  line('');
  line(`  行 3 中央含手工件的真实临街面：${covered.toFixed(1)} m / ${(x1 - x0).toFixed(0)} m = ${(covered / (x1 - x0) * 100).toFixed(0)}%（最大空档 ${gap.toFixed(1)} m ${gapAt ? `[${gapAt[0].toFixed(1)}, ${gapAt[1].toFixed(1)}]` : ''}）`);
  line(`  → 上面只统计 CITY_LOTS，所以行 3 那一格会偏低；这行才是站在街上的实际观感`);
}

// ---- 3. 天际线仰角剖面 -----------------------------------------------------
line('');
line('─'.repeat(78));
line('B. 天际线仰角剖面（解析法：射线 vs 建筑 AABB，含女儿墙 + 最大冠部）');
line('─'.repeat(78));

// 建筑 AABB：主体 + 冠部（冠部随机 2.2~4.2m，这里给 min/max 两个版本）
function aabbs(crownH) {
  return CITY_LOTS.map(l => {
    const h = l.floors * FLOOR_H;
    const cx = l.x + l.w * 0.14, cz = l.z - l.d * 0.12;
    return {
      id: l.id,
      body: [l.x - l.w / 2 - 0.11, l.x + l.w / 2 + 0.11, 0, h, l.z - l.d / 2 - 0.11, l.z + l.d / 2 + 0.11],
      crown: [cx - l.w * 0.46 / 2, cx + l.w * 0.46 / 2, h, h + crownH, cz - l.d * 0.48 / 2, cz + l.d * 0.48 / 2],
    };
  });
}
function elevationFor(O, azDeg, boxes) {
  const a = azDeg * Math.PI / 180, dx = Math.sin(a), dz = Math.cos(a);
  let best = -Infinity, who = null;
  for (const { id, body, crown } of boxes) {
    for (const b of [body, crown]) {
      const [x0, x1, y0, y1, z0, z1] = b;
      let t0 = -Infinity, t1 = Infinity;
      if (Math.abs(dx) < 1e-9) { if (O[0] < x0 || O[0] > x1) continue; }
      else { let ta = (x0 - O[0]) / dx, tb = (x1 - O[0]) / dx; if (ta > tb) [ta, tb] = [tb, ta]; t0 = Math.max(t0, ta); t1 = Math.min(t1, tb); }
      if (Math.abs(dz) < 1e-9) { if (O[2] < z0 || O[2] > z1) continue; }
      else { let ta = (z0 - O[2]) / dz, tb = (z1 - O[2]) / dz; if (ta > tb) [ta, tb] = [tb, ta]; t0 = Math.max(t0, ta); t1 = Math.min(t1, tb); }
      if (t1 < t0 || t1 < 0) continue;
      const t = Math.max(t0, 0);
      if (t < 0.001) continue;
      const el = Math.atan2(y1 - O[1], t) * 180 / Math.PI;
      if (el > best) { best = el; who = id; }
    }
  }
  return { el: best, who };
}

const VIEWPOINTS = [
  { name: '阳台（朝南看街区）', O: [0, 5.0, 7.2], sector: [0, 180], note: '主景观面' },
  { name: '客厅窗口（朝南）', O: [0, 4.2, 5.6], sector: [0, 180], note: '' },
  { name: '阳台（朝北看后排）', O: [0, 5.0, 7.2], sector: [180, 360], note: '背面' },
  { name: '街面行人眼高', O: [0, 1.65, 11.0], sector: [0, 180], note: '便利店门口' },
];
const boxes = aabbs(4.2); // 最不利（冠部最高）
for (const vp of VIEWPOINTS) {
  const prof = [];
  for (let az = 0; az < 360; az += 5) prof.push({ az, ...elevationFor(vp.O, az, boxes) });
  const south = prof.filter(p => p.az >= 0 && p.az <= 180);
  const sky = south.filter(p => p.el === -Infinity).length;
  const vals = south.filter(p => p.el !== -Infinity).map(p => p.el);
  const mean = vals.reduce((a, b) => a + b, 0) / (vals.length || 1);
  const sd = Math.sqrt(vals.reduce((a, b) => a + (b - mean) ** 2, 0) / (vals.length || 1));
  line('');
  line(`视点：${vp.name}  ${vp.note}  O=(${vp.O.join(', ')})`);
  line(`  南半区 37 个方位角：${sky} 个方向完全无遮挡（纯天空） = ${(sky / 37 * 100).toFixed(0)}%`);
  line(`  有遮挡方向的仰角：min ${Math.min(...vals).toFixed(1)}° / mean ${mean.toFixed(1)}° / max ${Math.max(...vals).toFixed(1)}°  σ=${sd.toFixed(1)}°`);
  const buckets = { '0-10°': 0, '10-20°': 0, '20-30°': 0, '30-40°': 0, '>40°': 0 };
  for (const v of vals) { const k = v < 10 ? '0-10°' : v < 20 ? '10-20°' : v < 30 ? '20-30°' : v < 40 ? '30-40°' : '>40°'; buckets[k]++; }
  line(`  仰角分布：${Object.entries(buckets).map(([k, n]) => `${k} ${n}`).join('  ')}`);
  // 正南（主视轴）剖面
  const axis = [-30, -20, -10, 0, 10, 20, 30].map(a => { const p = prof.find(x => x.az === (a + 360) % 360); return `${a}°:${p.el === -Infinity ? '天空' : p.el.toFixed(0) + '°'}`; });
  line(`  主视轴剖面（az 为负=偏西）： ${axis.join('  ')}`);
  const south0 = prof.find(x => x.az === 0);
  line(`  正南（az=0）：${south0.el === -Infinity ? '完全无遮挡 → 天空' : `${south0.el.toFixed(1)}° 由 ${south0.who} 挡住`}`);
}

// ---- 3b. 阳台南向完整剖面（给图表用） --------------------------------------
{
  const O = [0, 5.0, 7.2];
  const prof = [];
  for (let az = 0; az <= 180; az += 5) { const r = elevationFor(O, az, boxes); prof.push(r.el === -Infinity ? null : +r.el.toFixed(1)); }
  line('');
  line(`[CHART] balconySouthElevationDeg = ${JSON.stringify(prof)}`);
  line('[CHART] az = 0,5,...,180（null = 完全无遮挡 → 天空）');
}

// ---- 4. 高度直方图 ---------------------------------------------------------
line('');
line('─'.repeat(78));
line('C. 高度直方图 / 天际线是否平台化');
line('─'.repeat(78));
const hist = {};
for (const l of CITY_LOTS) hist[l.floors] = (hist[l.floors] || 0) + 1;
for (const f of Object.keys(hist).sort()) line(`  ${pad(f + ' 层', 8)} (${pad((f * FLOOR_H).toFixed(1), 5)} m)  ×  ${hist[f]} 栋   ${'█'.repeat(hist[f])}`);
line(`  → 众数 ${Object.entries(hist).sort((a, b) => b[1] - a[1])[0][0]} 层，占 ${(Object.entries(hist).sort((a, b) => b[1] - a[1])[0][1] / CITY_LOTS.length * 100).toFixed(0)}%`);
line('');
line('  按 x 列看层数（每列 4 个 z）：');
const xs = [...new Set(CITY_LOTS.map(l => l.x))].sort((a, b) => a - b);
for (const x of xs) {
  const col = CITY_LOTS.filter(l => l.x === x).sort((a, b) => a.z - b.z);
  line(`   x=${pad(x, 5)}  ${col.map(l => `${l.id.startsWith('side') ? 's' : 'p'}${l.floors}`).join(' ')}   ${col.map(l => (l.floors * FLOOR_H).toFixed(1)).join(' ')} m`);
}

// ---- 5. 石材配色周期性 -----------------------------------------------------
line('');
line('─'.repeat(78));
line('D. 石材配色（districtArt.ts）');
line('─'.repeat(78));
const artSrc = readFileSync(resolve(here, '../../../../plugins/3d-apartment/src/anime/districtArt.ts'), 'utf8');
// `\]\s*\.map\(`：数组与 .map 之间允许换行/缩进——源码里那两行是分开写的，
// 早先写死 `\]\.map\(` 会一路lazy到文件后面某个 `].map(` 才匹配，抠出一大段代码。
const STONE = eval(grab(/const stone\s*=\s*(\[[\s\S]*?\])\s*\.map\(/, artSrc));   // eslint-disable-line no-eval
line(`  色阶数 ${STONE.length}：${STONE.join('  ')}`);
// 与 districtArt 同源的选色函数（乘法散列取高位），这里照抄一份用于统计
const pick = (i) => ((Math.imul(i + 1, 2654435761) >>> 8) % STONE.length);
const idx = CITY_LOTS.map((_, i) => pick(i));
const cHist = {};
idx.forEach((v) => { cHist[v] = (cHist[v] || 0) + 1; });
line(`  分布：${STONE.map((c, i) => `${c}×${cHist[i] || 0}`).join('  ')}`);
const rowsZ = [...new Set(CITY_LOTS.map((l) => l.z))].sort((a, b) => a - b);
let samePair = 0;
for (const z of rowsZ) {
  const row = CITY_LOTS.map((l, i) => ({ ...l, i })).filter((l) => l.z === z).sort((a, b) => a.x - b.x);
  line(`   z=${String(z).padStart(6)}  ${row.map((l) => `${String(l.x).padStart(6)}:c${pick(l.i)}`).join(' ')}`);
}
// 同色回卷：同一行里相隔固定列数的两栋同色，就是旧版 index%5 的指纹
for (const z of rowsZ) {
  const row = CITY_LOTS.map((l, i) => ({ ...l, i })).filter((l) => l.z === z).sort((a, b) => a.x - b.x);
  for (let i = 0; i < row.length; i++) for (let j = i + 1; j < row.length; j++) if (pick(row[i].i) === pick(row[j].i)) samePair++;
}
line(`  同一行内同色对数：${samePair}（旧版 index%5 在 8 列上是每行必 3 对）`);

// ---- 6. 结论摘要 -----------------------------------------------------------
line('');
line('═'.repeat(78));
line('摘要');
line('═'.repeat(78));
const worst = COVER.filter(c => c.cell === 1).sort((a, b) => b.maxGap - a.maxGap);
line(`· 中央格（cell 1, x∈[-42,39], 宽 81m）最差行最大空档 ${worst[0].maxGap.toFixed(1)} m`);
line(`· 中央格覆盖率：${COVER.filter(c => c.cell === 1).map(c => `行${c.row} ${(c.cov * 100).toFixed(0)}%`).join('  ')}`);
line(`· 侧格覆盖率：  ${COVER.filter(c => c.cell !== 1).map(c => `行${c.row}格${c.cell} ${(c.cov * 100).toFixed(0)}%`).join('  ')}`);
line('═'.repeat(78));
