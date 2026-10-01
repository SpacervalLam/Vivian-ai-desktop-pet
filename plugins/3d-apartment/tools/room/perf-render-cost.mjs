/**
 * 渲染成本分解探针（本地 dev 工具，scripts/ 不入库）
 *
 * headless 走 SwiftShader，帧时完全不可用。所以只测 GPU 无关的量：
 *   - renderer.render() 的 CPU 侧耗时（渲染列表构建 / 状态切换 / uniform 上传）
 *   - draw call / 三角形 / program 数
 * 并做变量对照：阴影 pass、天空球绘制顺序、阴影贴图尺寸。
 *
 * 用法：node plugins/3d-apartment/tools/room/perf-render-cost.mjs
 */
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/preview.html';
const PORT = 9337;
const profile = await mkdtemp(join(tmpdir(), 'perf-rc-'));
const chrome = spawn(CHROME, [
  '--headless=new', '--use-gl=angle', '--use-angle=swiftshader',
  '--enable-unsafe-swiftshader', '--no-sandbox', '--no-first-run',
  '--window-size=1280,800', '--remote-allow-origins=*',
  `--remote-debugging-port=${PORT}`, `--user-data-dir=${profile}`, URL,
], { stdio: 'ignore' });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function findPage() {
  for (let i = 0; i < 200; i++) {
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
await send('Page.enable'); await send('Runtime.enable');

async function waitReady() {
  for (let i = 0; i < 400; i++) {
    if (await evaluate('!!(window.__ROOM__ && window.__ROOM__.controls)')) return true;
    await sleep(500);
  }
  return false;
}
if (!(await waitReady())) throw new Error('scene never ready');
console.log('[cost] scene ready');

// 找关键对象
const inventory = await evaluate(`
  (() => {
    const R = window.__ROOM__;
    const out = { sky: null, dirLights: [], shadowCasters: 0, transparentMeshes: 0, envSet: !!R.scene.environment, fog: !!R.scene.fog };
    R.scene.traverse((o) => {
      if (o.name === 'district-atmosphere') out.sky = { renderOrder: o.renderOrder, depthTest: o.material.depthTest, depthWrite: o.material.depthWrite, frustumCulled: o.frustumCulled, visible: o.visible, tris: o.geometry.index.count/3 };
      if (o.isDirectionalLight) out.dirLights.push({ castShadow: o.castShadow, mapSize: o.shadow.mapSize.x, autoUpdate: o.shadow.autoUpdate, needsUpdate: o.shadow.needsUpdate, radius: o.shadow.camera.right, near: o.shadow.camera.near, far: o.shadow.camera.far, intensity: o.intensity });
      if (o.isMesh) {
        if (o.castShadow) out.shadowCasters++;
        const m = Array.isArray(o.material) ? o.material[0] : o.material;
        if (m && m.transparent) out.transparentMeshes++;
      }
    });
    out.colliders = window.__COLLIDER_N ?? null;
    out.dpr = R.renderer.getPixelRatio();
    return out;
  })()
`);
console.log('[cost] inventory:', JSON.stringify(inventory));

// 计时工具：反复调用 renderer.render，取中位数
await evaluate(`
  window.__cost = (label, opts) => {
    const R = window.__ROOM__;
    const key = R.scene.children.find(o => o.isDirectionalLight && o.castShadow);
    if (opts && opts.shadowForce) key.shadow.needsUpdate = true;
    const times = [];
    let last = null;
    for (let i = 0; i < 7; i++) {
      if (opts && opts.shadowForce) key.shadow.needsUpdate = true;
      R.renderer.info.reset();
      const t0 = performance.now();
      R.renderer.render(R.scene, R.camera);
      const t1 = performance.now();
      times.push(t1 - t0);
      last = { calls: R.renderer.info.render.calls, tris: R.renderer.info.render.triangles,
               programs: R.renderer.info.programs.length, lines: R.renderer.info.render.lines };
    }
    times.sort((a,b)=>a-b);
    return { label, cpuMsP50: +times[3].toFixed(2), cpuMsMin: +times[0].toFixed(2), ...last };
  };
  'ok'
`);

const results = {};
const camObserver = `(() => { const R = window.__ROOM__; R.controls.enabled=false;
  R.camera.position.set(24,23.4,34); R.camera.lookAt(0,8.4,7); R.camera.fov=42.3; R.camera.updateProjectionMatrix(); return 'ok'; })()`;
const camFps = `(() => { const R = window.__ROOM__; R.controls.enabled=false;
  R.camera.position.set(0,5.0,4); R.camera.lookAt(0,5.0,-6); R.camera.fov=54.8; R.camera.updateProjectionMatrix(); return 'ok'; })()`;

async function run(label, cam, opts = {}) {
  await evaluate(cam);
  const r = await evaluate(`window.__cost(${JSON.stringify(label)}, ${JSON.stringify(opts)})`);
  results[label] = r;
  console.log('[cost]', label, JSON.stringify(r));
}

await run('observer', camObserver);
await run('observer.shadowForced', camObserver, { shadowForce: true });
await run('fps_spawn', camFps);
await run('fps_spawn.shadowForced', camFps, { shadowForce: true });

// 天空球：改成最后绘制 + depthTest（模拟"只画没被挡住的像素"）
await evaluate(`
  (() => {
    const R = window.__ROOM__;
    let sky = null; R.scene.traverse(o => { if (o.name === 'district-atmosphere') sky = o; });
    window.__sky = sky;
    sky.renderOrder = 1000; sky.material.depthTest = true; sky.material.needsUpdate = true;
    return 'ok';
  })()
`);
await run('fps_spawn.skyLast', camFps);
await run('observer.skyLast', camObserver);
// 还原
await evaluate(`(() => { window.__sky.renderOrder = -1000; window.__sky.material.depthTest = false; window.__sky.material.needsUpdate = true; return 'ok'; })()`);

// 天空球隐藏：看它贡献多少 draw call / 顶点
await evaluate(`(() => { window.__sky.visible = false; return 'ok'; })()`);
await run('fps_spawn.noSky', camFps);
await run('observer.noSky', camObserver);
await evaluate(`(() => { window.__sky.visible = true; return 'ok'; })()`);

// 阴影贴图尺寸对照
await evaluate(`
  (() => { const R = window.__ROOM__;
    const key = R.scene.children.find(o => o.isDirectionalLight && o.castShadow);
    window.__key = key;
    key.shadow.mapSize.set(1024,1024);
    if (key.shadow.map) { key.shadow.map.dispose(); key.shadow.map = null; }
    key.shadow.needsUpdate = true; return 'ok'; })()
`);
await run('observer.shadow1024', camObserver, { shadowForce: true });
await run('fps_spawn.shadow1024', camFps, { shadowForce: true });

// 阴影正交框收紧对照（±34 → ±18）
await evaluate(`
  (() => { const k = window.__key;
    k.shadow.camera.left = -18; k.shadow.camera.right = 18; k.shadow.camera.top = 18; k.shadow.camera.bottom = -18;
    k.shadow.camera.updateProjectionMatrix(); k.shadow.needsUpdate = true; return 'ok'; })()
`);
await run('observer.shadowR18', camObserver, { shadowForce: true });

console.log('\n=== RESULTS ===');
console.log(JSON.stringify(results, null, 2));

ws.close(); chrome.kill(); await sleep(600);
await rm(profile, { recursive: true, force: true }).catch(() => {});
