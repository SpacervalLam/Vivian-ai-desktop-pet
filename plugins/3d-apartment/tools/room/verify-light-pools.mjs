/**
 * 地面光池 decal 自检（CDP；scripts/ 不入库）。
 *
 * 为什么需要它：光池是**贴地的加法平面**，深度测试照常生效。程序化街区的人行道面
 * 在 y=0.028、盲道条在 y=0.035 —— 光池只要低于铺装面，就会被自己的地面吃掉一块，
 * 而这件事在源码里完全看不出来（plane 的 position.y 是个孤立数字）。
 * 所以这里在真实场景里断言三件事：
 *   1. 每个光池都存在，且 position.y 高于它正下方最高的一块铺装面；
 *   2. 材质确实是 additive + depthWrite=false（否则会盖住地面成一块糊斑）；
 *   3. 整页没有未捕获异常 / console error。
 *
 * 用法：node plugins/3d-apartment/tools/room/verify-light-pools.mjs   （需要 1420 dev server 在跑）
 * 退出码：任何一条不满足 → 1
 */
import { spawn } from 'node:child_process';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const EXPECT = [
  { name: 'store-light-pool',     group: 'convenience-dynamic', z: [11.1, 15.3] },
  { name: 'izakaya-light-pool',   group: 'izakaya',             z: [11.2, 13.8] },
  { name: 'subway-light-spill',   group: 'subway-entrance',     z: [11.4, 14.8] },
  { name: 'subway-light-throat',  group: 'subway-entrance',     z: [12.95, 14.45] },
  { name: 'bookstore-light-pool', group: 'street-bookstore',    z: [12.7, 14.9] },
];

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/preview.html';
const PORT = Number(process.env.PROBE_PORT || 9386);
const profile = await mkdtemp(join(tmpdir(), 'lp-'));
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
const pageErrors = [];
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.method === 'Runtime.exceptionThrown') {
    pageErrors.push('exception: ' + (m.params?.exceptionDetails?.text || '?'));
  }
  if (m.method === 'Runtime.consoleAPICalled' && m.params?.type === 'error') {
    pageErrors.push('console.error: ' + (m.params.args || []).map((a) => a.value ?? a.description ?? '?').join(' ').slice(0, 200));
  }
  if (m.id && pending.has(m.id)) { const p = pending.get(m.id); pending.delete(m.id); m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result); }
};
const send = (method, params = {}) => new Promise((resolve, reject) => { pending.set(++id, { resolve, reject }); ws.send(JSON.stringify({ id, method, params })); });
const evaluate = async (expression) => {
  const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails).slice(0, 800));
  return r.result.value;
};
await send('Runtime.enable');
await send('Log.enable').catch(() => {});

for (let i = 0; i < 240; i++) {
  if (await evaluate('!!(window.__ROOM__ && window.__ROOM__.scene)')) break;
  await sleep(500);
}

const got = await evaluate(`(() => {
  const scene = window.__ROOM__.scene;
  const names = ${JSON.stringify(EXPECT.map((e) => e.name))};
  const out = {};
  for (const n of names) {
    const m = scene.getObjectByName(n);
    if (!m) { out[n] = null; continue; }
    const mat = Array.isArray(m.material) ? m.material[0] : m.material;
    out[n] = {
      y: +m.position.y.toFixed(3),
      z: +m.position.z.toFixed(2),
      parent: m.parent?.name || '(根)',
      additive: mat.blending === window.__ROOM__.THREE.AdditiveBlending,
      depthWrite: mat.depthWrite,
      transparent: mat.transparent,
      renderOrder: m.renderOrder,
      sceneCollideSkip: m.userData.sceneCollideSkip === true,
    };
  }
  return out;
})()`);

// 铺装面最高 y：程序化街区人行道 0.028 / 盲道条 0.035 / 绿地 0.015 / 车行道 0
const PAVING_TOP = 0.035;
let bad = 0;
console.log('地面光池自检（铺装面最高 y=0.035，光池必须高于它）\n');
for (const e of EXPECT) {
  const g = got[e.name];
  if (!g) { console.log(`  ❌ ${e.name} —— 场景里找不到`); bad++; continue; }
  const okY = g.y > PAVING_TOP;
  const okBlend = g.additive && g.depthWrite === false;
  const okParent = g.parent === e.group;
  const okZ = g.z >= e.z[0] && g.z <= e.z[1];
  const flag = okY && okBlend && okParent && okZ ? '✅' : '❌';
  if (flag === '❌') bad++;
  console.log(
    `  ${flag} ${e.name.padEnd(22)} y=${String(g.y).padEnd(6)} z=${String(g.z).padEnd(7)}` +
    ` 父=${g.parent.padEnd(20)} additive=${g.additive} depthWrite=${g.depthWrite}` +
    ` renderOrder=${g.renderOrder} collideSkip=${g.sceneCollideSkip}`,
  );
  if (!okY) console.log(`       ↳ y=${g.y} 不高于铺装面 ${PAVING_TOP} —— 会被地面吃掉一块`);
  if (!okBlend) console.log('       ↳ 混合模式不对：必须是 AdditiveBlending + depthWrite=false');
  if (!okParent) console.log(`       ↳ 父组是 ${g.parent}，期望 ${e.group}`);
  if (!okZ) console.log(`       ↳ z=${g.z} 不在期望区间 [${e.z[0]}, ${e.z[1]}]`);
}

console.log(`\n页面异常/报错：${pageErrors.length === 0 ? '无 ✅' : pageErrors.length + ' 条 ❌'}`);
for (const p of pageErrors.slice(0, 10)) console.log('  ' + p);
if (pageErrors.length) bad++;

send('Browser.close').catch(() => {});
chrome.kill();
console.log(bad ? '\n❌ 光池自检未通过' : '\n✅ 光池自检通过');
if (bad) process.exitCode = 1;
