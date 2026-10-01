/**
 * 程序化街区平面自检（CDP 版；scripts/ 不入库）。
 *
 * 原 check-urban-plan.mjs 走 playwright，而本机 playwright 未安装（见
 * vivian-rs-3d-verify 技能），所以这里用裸 WebSocket + CDP 重写，逻辑照旧：
 *   1. plan.surfaces 两两求交 —— 同一 y、平面投影真重叠（>0.001m）即报冲突；
 *   2. CITY_LOTS 里有没有地块压在人行道带上（离路缘 <0.23m）；
 *   3. setWet(true) 后沥青粗糙度必须低于 dry（湿地更亮）；
 *   4. **CITY_LOTS 不许压在任何「低层用地」上** —— 停车场（LOT）与便利店（ST）
 *      的矩形直接从 exterior.ts 源码抠出来，不手工镜像，避免漂移。
 *      这条是 2026-09-20 补的：行 3 西侧两栋楼整栋站在停车场上、其中一栋还
 *      啃掉便利店西墙 0.1m，而当时所有自检都是绿的——因为地块表只看得见
 *      CITY_LOTS，对手工件占地一无所知。
 *
 * 用法：node plugins/3d-apartment/tools/room/check-urban-plan-cdp.mjs   （需要 1420 dev server 在跑）
 * 退出码：有冲突 / 有占道 / 有地块压手工件用地 / wet>=dry → 1
 */
import { spawn } from 'node:child_process';
import { mkdtemp, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

// ── 从 exterior.ts 抠「低层用地」矩形 ────────────────────────────────────────
// LOT / ST 都是干净的对象字面量，正则可靠。其余手工件（地铁口 / 居酒屋 / 书店 /
// 小公园）的坐标散在多个常量与注释里，抠不动——那几件由
// check-district-overlaps.mjs 在真实场景里用实测矩形兜底。
const EXT_PATH = 'plugins/3d-apartment/src/anime/exterior.ts';
const extSrc = await readFile(EXT_PATH, 'utf8');
function numBlock(name) {
  const m = extSrc.match(new RegExp(`const ${name}\\s*=\\s*\\{([\\s\\S]*?)\\};`));
  if (!m) throw new Error(`未能在 ${EXT_PATH} 里找到 const ${name} = {…}`);
  const out = {};
  for (const [, k, v] of m[1].matchAll(/(\w+)\s*:\s*(-?[\d.]+)/g)) out[k] = Number(v);
  return out;
}
const LOT = numBlock('LOT'), ST = numBlock('ST');
const GROUND = [
  { id: 'parking-lot', x0: LOT.x0, x1: LOT.x1, z0: LOT.z0, z1: LOT.z1 },
  { id: 'convenience-store', x0: ST.x0, x1: ST.x1, z0: ST.zFront, z1: ST.zBack },
];

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/tools/room/export-base.html';
const PORT = Number(process.env.PROBE_PORT || 9376);
const profile = await mkdtemp(join(tmpdir(), 'urban-'));
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
      const p = list.find((t) => t.type === 'page' && t.url.includes('export-base'));
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
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails).slice(0, 800));
  return r.result.value;
};
await send('Runtime.enable');
await sleep(1500);

const r = await evaluate(`(async () => {
  const THREE = await import('/node_modules/three/build/three.module.js');
  const { buildUrbanStreets, CITY_LOTS, CITY_ROADS } = await import('/plugins/3d-apartment/src/anime/urbanStreets.ts');
  const scene = new THREE.Scene();
  const u = buildUrbanStreets(scene);
  const plan = scene.getObjectByName('planned-street-network').userData.plan;
  const conflicts = [];
  for (let i = 0; i < plan.surfaces.length; i++) {
    for (let j = i + 1; j < plan.surfaces.length; j++) {
      const a = plan.surfaces[i], b = plan.surfaces[j];
      if (Math.abs(a.y - b.y) < 0.0001
        && Math.min(a.x1, b.x1) - Math.max(a.x0, b.x0) > 0.001
        && Math.min(a.z1, b.z1) - Math.max(a.z0, b.z0) > 0.001) {
        conflicts.push([a.name, b.name]);
      }
    }
  }
  const blocked = CITY_LOTS.filter(l =>
    CITY_ROADS.eastWest.some(r => Math.abs(l.z - r.z) < l.d / 2 + 0.23 + r.width / 2 + 2.4)
    || CITY_ROADS.northSouth.some(r => Math.abs(l.x - r.x) < l.w / 2 + 0.23 + r.width / 2 + 2.4));
  // 地块不许压在任何低层用地上（停车场 / 便利店）
  const GROUND = ${JSON.stringify(GROUND)};
  const onGround = [];
  for (const l of CITY_LOTS) {
    for (const g of GROUND) {
      const ox = Math.min(l.x + l.w / 2, g.x1) - Math.max(l.x - l.w / 2, g.x0);
      const oz = Math.min(l.z + l.d / 2, g.z1) - Math.max(l.z - l.d / 2, g.z0);
      if (ox > 0.05 && oz > 0.05 && ox * oz > 0.25)
        onGround.push({ lot: l.id, ground: g.id, ox: +ox.toFixed(2), oz: +oz.toFixed(2), area: +(ox * oz).toFixed(1) });
    }
  }
  u.setWet(false); const dry = u.asphalt.roughness;
  u.setWet(true);  const wet = u.asphalt.roughness;
  // 按名字汇总一下 surface 数量，便于看人行道带有没有被改动搞出重复
  const byName = {};
  for (const s of plan.surfaces) byName[s.name] = (byName[s.name] || 0) + 1;
  return {
    buildings: CITY_LOTS.length,
    roads: CITY_ROADS.eastWest.length + CITY_ROADS.northSouth.length,
    surfaces: plan.surfaces.length,
    surfaceOverlaps: conflicts,
    sidewalkObstructions: blocked.map(x => x.id),
    onGround,
    dry, wet, byName,
  };
})()`);

console.log(JSON.stringify(r, null, 2));
send('Browser.close').catch(() => {});   // 不等回包：关掉连接时 promise 永远不 settle
chrome.kill();

const bad = r.surfaceOverlaps.length || r.sidewalkObstructions.length || r.onGround.length || r.wet >= r.dry;
console.log(r.onGround.length ? `\n❌ ${r.onGround.length} 块地块压在手工件用地上` : '\n✅ 地块表未侵入任何低层用地');
console.log(bad ? '\n❌ 平面自检未通过' : '\n✅ 平面自检通过（0 重叠 / 0 占道 / 0 侵用地 / wet<dry）');
if (bad) process.exitCode = 1;
