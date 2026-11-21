/**
 * 单个顶层组里「每个 mesh 的世界 AABB」逐条列出（只读；scripts/ 不入库）。
 *
 * 用途：check-district-overlaps.mjs 报出「A 组 ∩ B 组 重叠 x×y×z」之后，
 * 组级 AABB 只能说「有事」，说不出「是哪个构件」。这个探针把它拆到 mesh 级，
 * 并可按一个阈值筛出越界的那些（默认列出全部）。
 *
 * 用法：node plugins/3d-apartment/tools/room/probe-group-aabb.mjs <组名> [--over-x 值] [--min-vol 值]
 *   例：node plugins/3d-apartment/tools/room/probe-group-aabb.mjs small-park --over-x -31.4
 * 需要 1420 dev server 在跑。
 */
import { spawn } from 'node:child_process';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const groupName = process.argv[2];
if (!groupName) throw new Error('用法：node probe-group-aabb.mjs <组名> [--over-x 值]');
const argOf = (flag) => {
  const i = process.argv.indexOf(flag);
  return i >= 0 ? Number(process.argv[i + 1]) : undefined;
};
const overX = argOf('--over-x');

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/preview.html';
const PORT = Number(process.env.PROBE_PORT || 9366);
const profile = await mkdtemp(join(tmpdir(), 'aabb-'));
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

// 等 __ROOM__ 就绪
for (let i = 0; i < 240; i++) {
  const ok = await evaluate('!!(window.__ROOM__ && window.__ROOM__.scene)');
  if (ok) break;
  await sleep(500);
}

const rows = await evaluate(`(() => {
  const THREE = window.__ROOM__.THREE, scene = window.__ROOM__.scene;
  const root = scene.getObjectByName(${JSON.stringify(groupName)});
  if (!root) return { error: '找不到组 ' + ${JSON.stringify(groupName)} };
  const out = [];
  root.traverse((o) => {
    if (!o.isMesh) return;
    const b = new THREE.Box3().setFromObject(o);
    out.push({
      name: o.name || '(匿名)',
      parent: o.parent?.name || '(根)',
      x0: +b.min.x.toFixed(2), x1: +b.max.x.toFixed(2),
      y0: +b.min.y.toFixed(2), y1: +b.max.y.toFixed(2),
      z0: +b.min.z.toFixed(2), z1: +b.max.z.toFixed(2),
    });
  });
  return { total: out.length, rows: out };
})()`);

if (rows.error) { console.log(rows.error); await send('Browser.close').catch(() => {}); chrome.kill(); process.exit(1); }

const list = overX === undefined ? rows.rows : rows.rows.filter((r) => r.x1 > overX);
console.log(`组 ${groupName}：mesh ${rows.total} 个，列出 ${list.length} 个${overX === undefined ? '' : `（x1 > ${overX}）`}`);
console.log('  name'.padEnd(34) + 'parent'.padEnd(26) + 'x'.padEnd(20) + 'y'.padEnd(18) + 'z');
for (const r of list.sort((a, b) => b.x1 - a.x1)) {
  console.log(
    `  ${r.name.slice(0, 32).padEnd(32)}  ${r.parent.slice(0, 24).padEnd(24)}` +
    `  [${String(r.x0).padStart(7)},${String(r.x1).padStart(7)}]` +
    `  [${String(r.y0).padStart(6)},${String(r.y1).padStart(6)}]` +
    `  [${String(r.z0).padStart(7)},${String(r.z1).padStart(7)}]`,
  );
}

send('Browser.close').catch(() => {});   // 不等回包
chrome.kill();
