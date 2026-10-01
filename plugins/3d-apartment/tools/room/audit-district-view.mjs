/**
 * 街区视线审计（只读，不改任何源码；scripts/ 不入库）
 *
 * 目的：回答「从 203 阳台望出去，实际看到了什么」这类设计问题。模型读不了渲染图，
 * 所以用射线扇扫把视野量化成：各类物体的占比、命中距离分布、以及**天际线剖面**
 * （每个方位角上的最高命中仰角）——天际线剖面的方差直接说明「是不是一片平的楼顶
 * 平台」，这是从代码里读不出来的。
 *
 * 射线方向是手工按方位角/仰角算的，**不经过相机**：观察者模式下渲染循环每帧都会
 * 用 OrbitControls 覆写相机，靠相机算方向会被它搅乱。+z 是南（公寓正面朝向）。
 *
 * 用法：node plugins/3d-apartment/tools/room/audit-district-view.mjs
 */
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/preview.html';
const PORT = Number(process.env.AUDIT_PORT || 9354);
const profile = await mkdtemp(join(tmpdir(), 'district-'));
const chrome = spawn(CHROME, [
  '--headless=new', '--use-gl=angle', '--use-angle=swiftshader',
  '--enable-unsafe-swiftshader', '--no-sandbox', '--no-first-run',
  '--window-size=1000,700', '--remote-allow-origins=*',
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
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails).slice(0, 500));
  return r.result.value;
};
await send('Runtime.enable');
for (let i = 0; i < 300; i++) {
  if (await evaluate('!!(window.__ROOM__ && window.__ROOM__.scene)')) break;
  await sleep(500);
}
await sleep(3000);
console.log('[district-audit] scene ready');

/**
 * 在页面里装一个扇扫函数。方位角 az 以 +z（南）为 0，顺时针为正；仰角 el 向上为正。
 *
 * 场景里有个别对象 raycast 会抛（three 内部），所以这里先只收集**能安全参与**
 * 的 Mesh（有 position 属性、不是 Points/Line/Sprite），再逐条射线 try/catch，
 * 并把抛错的对象单独列出来——免得一个坏对象让整次审计拿不到数。
 */
const sweep = await evaluate(`
(() => {
  const R = window.__ROOM__, THREE = R.THREE;
  const rc = new THREE.Raycaster();
  rc.far = 400;

  // 收集候选 mesh（一次）
  const meshes = [];
  R.scene.traverse((o) => {
    if (!o.isMesh) return;
    if (!o.geometry || !o.geometry.attributes || !o.geometry.attributes.position) return;
    meshes.push(o);
  });

  // 逐个 mesh 单射线试一遍，找出会抛的（诊断用）
  const bad = [];
  const badObjects = new Set();
  const probe = new THREE.Raycaster();
  probe.far = 400;
  probe.set(new THREE.Vector3(0, 5, 7.2), new THREE.Vector3(0, 0, 1));
  for (const m of meshes) {
    try { probe.intersectObject(m, false); }
    catch (e) {
      badObjects.add(m);   // 按对象身份排除，不要按名字——场景里有大量 (unnamed)
      if (bad.length < 12) bad.push({ name: m.name || '(unnamed)', type: m.type, geo: m.geometry.type, err: String(e && e.message).slice(0, 80) });
    }
  }
  const safe = meshes.filter((m) => !badObjects.has(m));

  const classify = (hit) => {
    let o = hit.object, names = [];
    for (let d = 0; d < 4 && o; d++, o = o.parent) if (o.name) names.push(o.name);
    const n = names.join('|');
    const mat = Array.isArray(hit.object.material) ? hit.object.material[0] : hit.object.material;
    const color = mat && mat.color ? mat.color.getHexString() : null;
    if (/district-atmosphere/.test(n)) return { cls: 'sky', names, color };
    if (/world-ground/.test(n)) return { cls: 'ground', names, color };
    if (/city-block/.test(n)) return { cls: 'procedural-block', names, color };
    if (/planned-street-network|sidewalk|road|crosswalk|lane-mark/.test(n)) return { cls: 'street', names, color };
    if (/apartment|unit-203|shell|podium/.test(n)) return { cls: 'apartment', names, color };
    if (/store|izakaya|bookstore|park|subway|furniture|pole/.test(n)) return { cls: 'setpiece', names, color };
    return { cls: 'other', names, color };
  };

  window.__auditInfo = { meshCount: meshes.length, safeCount: safe.length, unraycastable: bad.slice(0, 12) };

  window.__sweep = (ox, oy, oz, azStep, elStep) => {
    const out = [];
    const dir = new THREE.Vector3();
    let errCount = 0;
    for (let az = -75; az <= 75; az += azStep) {
      let skylineEl = null;
      for (let el = -30; el <= 40; el += elStep) {
        const a = az * Math.PI / 180, e = el * Math.PI / 180;
        // +z 为南：az=0 → (0,0,1)；az 增大朝 +x（东）偏
        dir.set(Math.sin(a) * Math.cos(e), Math.sin(e), Math.cos(a) * Math.cos(e)).normalize();
        rc.set(new THREE.Vector3(ox, oy, oz), dir);
        let h = null;
        try { h = rc.intersectObjects(safe, false).find((x) => x.object.visible); }
        catch (err) { errCount++; }
        if (!h) { out.push({ az, el, cls: 'nothing', d: null, names: [], color: null }); continue; }
        const c = classify(h);
        out.push({ az, el, cls: c.cls, d: +h.distance.toFixed(2), names: c.names, color: c.color });
        if (c.cls !== 'sky' && skylineEl === null) skylineEl = el;
      }
      out.push({ az, skylineEl });
    }
    out.push({ __errCount: errCount });
    return out;
  };
  return 'ok';
})()
`);
if (sweep !== 'ok') throw new Error('sweep install failed');
console.log('[district-audit]', JSON.stringify(await evaluate('window.__auditInfo')));

/** 汇总一个视点 */
const analyse = async (label, origin) => {
  const raw = await evaluate(`window.__sweep(${origin[0]}, ${origin[1]}, ${origin[2]}, 5, 2.5)`);
  const rays = raw.filter((r) => r.cls);
  const byCls = {};
  for (const r of rays) byCls[r.cls] = (byCls[r.cls] || 0) + 1;
  const total = rays.length;
  const pct = Object.fromEntries(Object.entries(byCls).map(([k, v]) => [k, +(100 * v / total).toFixed(1)]));

  const dists = rays.filter((r) => r.cls !== 'sky' && r.d != null).map((r) => r.d).sort((a, b) => a - b);
  const q = (p) => dists.length ? +dists[Math.min(dists.length - 1, Math.floor(p * dists.length))].toFixed(1) : null;

  const skylines = raw.filter((r) => r.skylineEl !== undefined && r.skylineEl !== null).map((r) => r.skylineEl);
  const mean = skylines.reduce((a, b) => a + b, 0) / (skylines.length || 1);
  const sd = Math.sqrt(skylines.reduce((a, b) => a + (b - mean) ** 2, 0) / (skylines.length || 1));

  // 程序化楼体的石材配色分布（看是否出现规律循环）
  const colors = {};
  for (const r of rays) if (r.cls === 'procedural-block' && r.color) colors[r.color] = (colors[r.color] || 0) + 1;

  return {
    label, origin,
    rayCount: total,
    classPct: pct,
    distance: { p10: q(0.1), p50: q(0.5), p90: q(0.9), max: dists.length ? +dists[dists.length - 1].toFixed(1) : null },
    skylineElevationDeg: {
      samples: skylines.length,
      min: skylines.length ? Math.min(...skylines) : null,
      max: skylines.length ? Math.max(...skylines) : null,
      mean: +mean.toFixed(2),
      stdDev: +sd.toFixed(2),
    },
    proceduralStoneColors: colors,
  };
};

const views = [];
// 1) 203 阳台栏杆外、站在阳台上的眼高（2F 楼板 3.4 + 1.6）
views.push(await analyse('203 阳台（朝南望街区）', [0, 5.0, 7.2]));
// 2) 客厅窗内（窗台后一点，仍是 2F 眼高）
views.push(await analyse('203 客厅窗内（朝南）', [-0.65, 5.2, 2.0]));
// 3) 默认观察者机位（微缩全景）——从 layout.camera 读
const camPos = await evaluate('(() => { const c = window.__ROOM__.camera.position; return [+c.x.toFixed(2), +c.y.toFixed(2), +c.z.toFixed(2)]; })()');
views.push(await analyse('默认观察者机位', camPos));

console.log(JSON.stringify({ views }, null, 2));
ws.close(); chrome.kill(); await sleep(600);
await rm(profile, { recursive: true, force: true }).catch(() => {});
