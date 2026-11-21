/**
 * 手工件体量冲突检查（只读，不改源码；scripts/ 不入库）
 *
 * 背景：check-urban-plan.mjs 只校验**程序化**的街区平面（CITY_LOTS / 路网 / 人行道），
 * 对 buildApartmentShell / buildSmallPark / buildConvenienceStore 这些**手工件**没有任何约束。
 * 而手工件的坐标全是硬编码的世界绝对坐标，很容易出现「公园埋进公寓一层实心体量」这类
 * 从代码里肉眼看不出来的冲突。
 *
 * 本脚本在真实场景里：
 *   1. 取每个具名顶层组的 AABB，两两求交，列出体量重叠（排除地面/天空/路网这类本来就铺满的）；
 *   2. 对每个组里的每个 mesh，判断其中心是否落在**公寓一层实心体量**里
 *      （x∈[-31,31], z∈[-5.9,4.7], y∈[0,3.4]，见 exterior.ts:1766「1F 实心体量」），
 *      用来抓「被埋进楼里」的构件；
 *   3. 顺带统计合批后还剩下多少具名 mesh（验证 mergeByMaterial 丢名字这件事）；
 *   4. **逐 mesh** 把建筑基座的 AABB 投影到 XZ，去撞停车场的地面构件并集
 *      ——组级 AABB 查不出「楼站在停车场上」（district 的 AABB 横跨 146m），
 *      只有这一节能查。2026-09-20 行 3 西侧两栋就是这么漏掉的。
 *
 * 用法：node plugins/3d-apartment/tools/room/check-district-overlaps.mjs   （需要 1420 dev server 在跑）
 * 退出码：有建筑压在停车场上 → 1
 */
import { spawn } from 'node:child_process';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/preview.html';
const PORT = Number(process.env.PROBE_PORT || 9356);
const profile = await mkdtemp(join(tmpdir(), 'overlap-'));
const chrome = spawn(CHROME, [
  '--headless=new', '--use-gl=angle', '--use-angle=swiftshader',
  '--enable-unsafe-swiftshader', '--no-sandbox', '--no-first-run',
  '--window-size=900,600', '--remote-allow-origins=*',
  `--remote-debugging-port=${PORT}`, `--user-data-dir=${profile}`, URL,
], { stdio: 'ignore' });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function findPage() {
  for (let i = 0; i < 240; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
      const p = list.find((t) => t.type === 'page' && t.url.includes('room-preview'));
      if (p?.webSocketDebuggerUrl) return p;
    } catch { /* wait */ }
    await sleep(500);
  }
  throw new Error('no target');
}
const page = await findPage();
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
let id = 0; const pending = new Map();
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { const p = pending.get(m.id); pending.delete(m.id); m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result); }
};
const send = (method, params = {}) => new Promise((resolve, reject) => { pending.set(++id, { resolve, reject }); ws.send(JSON.stringify({ id, method, params })); });
const evaluate = async (expression) => {
  const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails).slice(0, 600));
  return r.result.value;
};
await send('Runtime.enable');
for (let i = 0; i < 300; i++) { if (await evaluate('!!(window.__ROOM__ && window.__ROOM__.scene)')) break; await sleep(500); }
await sleep(3000);

const out = await evaluate(`
(async () => {
  const R = window.__ROOM__, THREE = R.THREE;
  // parking-lot 是**地面**用地（露天停车场），但必须进这份清单：它是低层铺装，
  // 任何落在它上面的地块都会「楼插进停车场」。早先漏登记它，行 3 西侧两栋
  // 正好站在停车场里、其中一栋还啃掉便利店西墙 0.1m，本脚本一声没吭。
  const WANT = ['apartment-shell','small-park','convenience-store','convenience-dynamic',
    'subway-entrance','izakaya','street-bookstore','authored-city-district',
    'authored-city-frontage','authored-roof-and-garden','parking-lot',
    'street-furniture','utility-poles'];
  const groups = {};
  R.scene.traverse(o => { if (WANT.includes(o.name) && !groups[o.name]) groups[o.name] = o; });

  const box = (o) => { o.updateMatrixWorld(true); return new THREE.Box3().setFromObject(o); };
  const info = {};
  for (const [n, o] of Object.entries(groups)) {
    const b = box(o); let meshes = 0, named = 0, tris = 0;
    o.traverse(m => { if (!m.isMesh) return; meshes++; if (m.name) named++;
      const g = m.geometry; const idx = g.index; tris += (idx ? idx.count : (g.attributes.position?.count || 0)) / 3; });
    info[n] = { min: b.min.toArray().map(v => +v.toFixed(2)), max: b.max.toArray().map(v => +v.toFixed(2)),
      meshes, named, tris: Math.round(tris) };
  }

  // 两两 AABB 交叠
  const names = Object.keys(info);
  const overlaps = [];
  for (let i = 0; i < names.length; i++) for (let j = i + 1; j < names.length; j++) {
    const a = info[names[i]], b = info[names[j]];
    const ov = [0,1,2].map(k => Math.min(a.max[k], b.max[k]) - Math.max(a.min[k], b.min[k]));
    if (ov.every(v => v > 0.05)) overlaps.push({ a: names[i], b: names[j],
      ov: ov.map(v => +v.toFixed(2)), vol: +(ov[0]*ov[1]*ov[2]).toFixed(1) });
  }
  overlaps.sort((x, y) => y.vol - x.vol);

  // 公寓一层实心体量（exterior.ts:1766 说明 1F 是实心体量，贴在立面线后）
  const APT = new THREE.Box3(new THREE.Vector3(-31, 0, -5.9), new THREE.Vector3(31, 3.4, 4.7));
  const buried = [];
  for (const [n, o] of Object.entries(groups)) {
    if (n === 'apartment-shell') continue;
    let cnt = 0, deepest = 0;
    o.traverse(m => {
      if (!m.isMesh) return;
      const c = new THREE.Box3().setFromObject(m).getCenter(new THREE.Vector3());
      if (APT.containsPoint(c)) { cnt++; const d = APT.max.z - c.z; if (d > deepest) deepest = d; }
    });
    if (cnt) buried.push({ group: n, meshesInsideApartment1F: cnt, deepest_m: +deepest.toFixed(2) });
  }
  buried.sort((a, b) => b.meshesInsideApartment1F - a.meshesInsideApartment1F);

  // 合批丢名字的验证
  let totalMeshes = 0, namedMeshes = 0;
  R.scene.traverse(o => { if (o.isMesh) { totalMeshes++; if (o.name) namedMeshes++; } });
  const cityBlockNames = [];
  R.scene.traverse(o => { if (/^city-block-/.test(o.name)) cityBlockNames.push(o.name); });

  // ---- 地块表 vs 低层用地（停车场铺装）的 2D 投影 ----
  // 不能拿场景里的建筑 mesh 做这件事：mergeByMaterial 会把整个街区压成十几个
  // 大 mesh，每个的 AABB 都横跨 146m，跟谁都"重叠"。所以这里回到**地块表**
  // （CITY_LOTS 是每栋楼的权威矩形，合批免疫），只把停车场的用地矩形从场景实测。
  // 停车场地面构件 = y 上限 0.2 的那些（铺装 / 白线 / 路缘 / 车止め），
  // 不含车、高杆灯、竖招牌。
  const { CITY_LOTS } = await import('/plugins/3d-apartment/src/anime/urbanStreets.ts');
  const park = groups['parking-lot'];
  let parkRect = null;
  if (park) {
    let x0 = Infinity, x1 = -Infinity, z0 = Infinity, z1 = -Infinity;
    park.traverse(m => {
      if (!m.isMesh) return;
      const b = new THREE.Box3().setFromObject(m);
      if (b.max.y > 0.2) return;
      x0 = Math.min(x0, b.min.x); x1 = Math.max(x1, b.max.x);
      z0 = Math.min(z0, b.min.z); z1 = Math.max(z1, b.max.z);
    });
    if (x1 > x0) parkRect = { x0, x1, z0, z1 };
  }
  const onLot = [];
  if (parkRect) {
    for (const l of CITY_LOTS) {
      const a = { x0: l.x - l.w / 2, x1: l.x + l.w / 2, z0: l.z - l.d / 2, z1: l.z + l.d / 2 };
      const ox = Math.min(a.x1, parkRect.x1) - Math.max(a.x0, parkRect.x0);
      const oz = Math.min(a.z1, parkRect.z1) - Math.max(a.z0, parkRect.z0);
      if (ox > 0.05 && oz > 0.05 && ox * oz > 0.25) {
        onLot.push({ id: l.id, ox: +ox.toFixed(2), oz: +oz.toFixed(2),
          area: +(ox * oz).toFixed(1), floors: l.floors });
      }
    }
    onLot.sort((a, b) => b.area - a.area);
  }

  return { info, overlaps, buried, onLot, lots: CITY_LOTS.length,
    parkRect: parkRect && { x0: +parkRect.x0.toFixed(2), x1: +parkRect.x1.toFixed(2), z0: +parkRect.z0.toFixed(2), z1: +parkRect.z1.toFixed(2) },
    nameStats: { totalMeshes, namedMeshes, cityBlockSurvivors: cityBlockNames.length } };
})()`);

console.log('═══ 具名顶层组体量 ═══');
for (const [n, v] of Object.entries(out.info)) {
  console.log(`  ${n.padEnd(24)} x[${String(v.min[0]).padStart(7)},${String(v.max[0]).padStart(7)}]  y[${String(v.min[1]).padStart(6)},${String(v.max[1]).padStart(6)}]  z[${String(v.min[2]).padStart(7)},${String(v.max[2]).padStart(7)}]  mesh ${String(v.meshes).padStart(4)} (具名 ${String(v.named).padStart(4)})  ${String(v.tris).padStart(7)} tris`);
}
console.log('');
console.log('═══ 组间 AABB 交叠（>0.05m 三轴同时）——粗筛，假阳性多，以末节为准 ═══');
if (!out.overlaps.length) console.log('  （无）');
for (const o of out.overlaps) console.log(`  ${o.a} ∩ ${o.b}   重叠 ${o.ov.join(' × ')} m   体积 ${o.vol} m³`);
console.log('');
console.log('═══ 地块表 vs 停车场用地（CITY_LOTS 矩形 vs 场景实测，2D 投影） ═══');
console.log(`  CITY_LOTS 共 ${out.lots} 块`);
if (out.parkRect) console.log(`  停车场地面构件并集：x[${out.parkRect.x0}, ${out.parkRect.x1}]  z[${out.parkRect.z0}, ${out.parkRect.z1}]`);
if (!out.onLot.length) console.log('  ✅ 没有任何地块落在停车场用地上');
for (const b of out.onLot) console.log(`  ❌ ${b.id}（${b.floors} 层）  压入 ${b.ox} × ${b.oz} m（${b.area} m²）`);
console.log('');
console.log('═══ 构件中心落在「公寓一层实心体量」内（x[-31,31] z[-5.9,4.7] y[0,3.4]） ═══');
if (!out.buried.length) console.log('  （无）');
for (const b of out.buried) console.log(`  ${b.group.padEnd(24)} ${String(b.meshesInsideApartment1F).padStart(4)} 个 mesh 埋在楼里，最深 ${b.deepest_m} m`);
console.log('');
console.log('═══ 合批丢名字 ═══');
console.log(`  场景 mesh 总数 ${out.nameStats.totalMeshes}，其中具名 ${out.nameStats.namedMeshes}`);
console.log(`  仍以 city-block-* 命名的对象：${out.nameStats.cityBlockSurvivors} 个（合批后应为 0）`);

ws.close(); chrome.kill();
if (out.onLot.length) { console.log(`\n❌ 有 ${out.onLot.length} 个建筑构件压在停车场用地上`); process.exitCode = 1; }
else console.log('\n✅ 建筑基座与停车场用地无冲突');
process.exit(process.exitCode || 0);
