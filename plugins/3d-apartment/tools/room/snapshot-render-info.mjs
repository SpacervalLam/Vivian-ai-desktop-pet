/**
 * 一帧提交量快照（CDP；scripts/ 不入库）。
 *
 * 为什么不用 perf-render-cost.mjs：那个脚本会主动做多种变量对照（阴影 pass 开关、
 * 天空顺序、不同阴影贴图尺寸），每次都要真渲一帧；SwiftShader 下一帧要几秒到几十秒，
 * 本机跑到 6 分钟还没出结果。这里只读**已经跑过的那一帧**的计数器，不额外渲染。
 *
 * 前提：RoomScene 设了 `renderer.info.autoReset = false` 并在帧首 `info.reset()`，
 * 所以 `info.render.calls` 是**整帧跨所有 renderer.render 累加**的值
 * （RenderPass + 泛光各级 mip + OutputPass + Reflector 整场 pass）。
 * 它不含「一帧内跑了几次 pass」的信息，只回答「这一帧总共提交了多少 draw call」。
 *
 * 用法：node plugins/3d-apartment/tools/room/snapshot-render-info.mjs   （需要 1420 dev server 在跑）
 */
import { spawn } from 'node:child_process';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/preview.html';
const PORT = Number(process.env.PROBE_PORT || 9396);
const profile = await mkdtemp(join(tmpdir(), 'info-'));
const chrome = spawn(CHROME, [
  '--headless=new', '--use-gl=angle', '--use-angle=swiftshader',
  '--enable-unsafe-swiftshader', '--no-sandbox', '--no-first-run',
  '--window-size=1280,800', '--remote-allow-origins=*',
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
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails).slice(0, 800));
  return r.result.value;
};
await send('Runtime.enable');

for (let i = 0; i < 240; i++) {
  if (await evaluate('!!(window.__ROOM__ && window.__ROOM__.scene)')) break;
  await sleep(500);
}
// 计数器是「最近一帧」的，等一帧真的画完再读
for (let i = 0; i < 120; i++) {
  const n = await evaluate('window.__ROOM__.renderer.info.render.calls');
  if (n > 0) break;
  await sleep(1000);
}

const r = await evaluate(`(() => {
  const R = window.__ROOM__, scene = R.scene;
  let meshes = 0, named = 0, transparent = 0, tris = 0;
  const byName = {};
  scene.traverse((o) => {
    if (!o.isMesh) return;
    meshes++;
    if (o.name) named++;
    const mat = Array.isArray(o.material) ? o.material[0] : o.material;
    if (mat && mat.transparent) transparent++;
    const g = o.geometry;
    if (g) {
      const c = g.index ? g.index.count : g.attributes.position.count;
      tris += c / 3;
      if (o.name) byName[o.name] = (byName[o.name] || 0) + c / 3;
    }
  });
  const top = Object.entries(byName).sort((a, b) => b[1] - a[1]).slice(0, 12)
    .map(([n, t]) => n + ' ' + Math.round(t) + ' tri');
  const info = R.renderer.info;
  const cam = R.camera;
  return {
    mode: document.documentElement.className,
    camera: cam ? { x: +cam.position.x.toFixed(2), y: +cam.position.y.toFixed(2), z: +cam.position.z.toFixed(2), fov: cam.fov } : null,
    render: { calls: info.render.calls, triangles: info.render.triangles, frame: info.render.frame },
    memory: info.memory,
    programs: info.programs ? info.programs.length : null,
    sceneMeshes: meshes, sceneNamed: named, sceneTransparent: transparent,
    sceneTriangles: Math.round(tris),
    topNamedByTris: top,
  };
})()`);

console.log('最近一帧的提交量（含阴影 pass / 泛光 mip / Reflector 整场 pass）');
console.log(JSON.stringify(r, null, 2));

send('Browser.close').catch(() => {});
chrome.kill();
