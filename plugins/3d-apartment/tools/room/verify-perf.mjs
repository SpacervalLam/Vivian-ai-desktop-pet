/**
 * 本轮性能改动的验证探针（本地 dev 工具，scripts/ 不入库）
 *
 * 三件事：
 *  1) 天空球改绘制顺序（renderOrder -1000/depthTest:false → 1000/depthTest:true）
 *     是否真的"零视觉变化"——把同一机位分别用新旧两套设置渲到自建 RT，
 *     逐像素比差值。理论上应完全一致：天空不写深度、alpha 恒 1，谁挡在前面
 *     由几何决定，跟绘制顺序无关。
 *  2) 阴影节流：hook renderer.render，按虚拟时间以 20ms 步进驱动真实帧，
 *     统计每帧 draw call 与 needsUpdate，算出"实际重画阴影的帧占比"。
 *  3) 出图给用户肉眼确认（我读不了图）。
 *
 * 用法：node plugins/3d-apartment/tools/room/verify-perf.mjs
 */
import { spawn } from 'node:child_process';
import { mkdtemp, rm, mkdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/preview.html';
const PORT = 9338;
const SHOT_DIR = 'G:/vivian-rs/plugins/3d-apartment/tools/room/shots';
const profile = await mkdtemp(join(tmpdir(), 'verify-perf-'));
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
const pageErrors = [];
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.method === 'Runtime.exceptionThrown') pageErrors.push(m.params.exceptionDetails.text + ' ' + (m.params.exceptionDetails.exception?.description || '').slice(0, 200));
  if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') pageErrors.push('console: ' + JSON.stringify(m.params.args.map((a) => a.value ?? a.description)).slice(0, 200));
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
await sleep(3000);
console.log('[verify] scene ready');

const report = { pageErrors: [], sky: null, pixelDiff: {}, shadow: null, shots: [] };

/* ---------- 1. 天空：新旧设置逐像素对比 ---------- */

report.sky = await evaluate(`
  (() => {
    const R = window.__ROOM__;
    let sky = null; R.scene.traverse(o => { if (o.name === 'district-atmosphere') sky = o; });
    window.__sky = sky;
    return { found: !!sky, renderOrder: sky.renderOrder, depthTest: sky.material.depthTest,
             depthWrite: sky.material.depthWrite, visible: sky.visible, frustumCulled: sky.frustumCulled };
  })()
`);

await evaluate(`
  (() => {
    const R = window.__ROOM__, THREE = R.THREE;
    window.__W = 176; window.__H = 120;
    window.__rt = new THREE.WebGLRenderTarget(window.__W, window.__H);
    window.__bufA = new Uint8Array(window.__W * window.__H * 4);
    window.__bufB = new Uint8Array(window.__W * window.__H * 4);
    window.__renderToRT = (which) => {
      const sky = window.__sky;
      if (which === 'new') { sky.renderOrder = 1000; sky.material.depthTest = true; }
      else { sky.renderOrder = -1000; sky.material.depthTest = false; }
      const prevTarget = R.renderer.getRenderTarget();
      R.renderer.setRenderTarget(window.__rt);
      R.renderer.clear();
      R.renderer.render(R.scene, R.camera);
      R.renderer.readRenderTargetPixels(window.__rt, 0, 0, window.__W, window.__H, which === 'new' ? window.__bufA : window.__bufB);
      R.renderer.setRenderTarget(prevTarget);
      return 'ok';
    };
    window.__diff = () => {
      const a = window.__bufA, b = window.__bufB;
      let maxAbs = 0, sum = 0, n = a.length;
      for (let i = 0; i < n; i++) { const d = Math.abs(a[i] - b[i]); if (d > maxAbs) maxAbs = d; sum += d; }
      return { maxAbsDiff: maxAbs, meanAbsDiff: +(sum / n).toFixed(4), bytes: n };
    };
    return 'ok';
  })()
`);

const cams = {
  observer_outdoor: [24, 23.4, 34, 0, 8.4, 7, 42.3],
  fps_window_up: [0, 5.0, 4, -6, 6.6, 8, 54.8],
  fps_room_wall: [0, 5.0, 4, 6, 4.6, -2, 54.8],
  fps_floor: [0, 5.0, 4, 0, 0.2, -2, 54.8],
};
for (const [label, [px, py, pz, tx, ty, tz, fov]] of Object.entries(cams)) {
  await evaluate(`(() => { const R = window.__ROOM__; R.controls.enabled = false;
    R.camera.position.set(${px},${py},${pz}); R.camera.lookAt(${tx},${ty},${tz});
    R.camera.fov = ${fov}; R.camera.updateProjectionMatrix(); return 'ok'; })()`);
  // 两次渲染：new 与 old。先 old 再 new，避免状态残留。
  await evaluate('window.__renderToRT("old")');
  await evaluate('window.__renderToRT("new")');
  report.pixelDiff[label] = await evaluate('window.__diff()');
  console.log('[verify] sky pixelDiff', label, JSON.stringify(report.pixelDiff[label]));
}

/* ---------- 2. 阴影节流：虚拟时间步进，统计真实帧 ---------- */

await evaluate(`
  (() => {
    const R = window.__ROOM__;
    const key = R.scene.children.find(o => o.isDirectionalLight && o.castShadow);
    window.__key = key;
    window.__frames = [];
    const orig = R.renderer.render.bind(R.renderer);
    R.renderer.render = (scene, camera) => {
      const before = key.shadow.needsUpdate;
      orig(scene, camera);
      const calls = R.renderer.info.render.calls;
      // 只记"场景 pass"（composer 的泛光全屏 pass 只有 1~2 个 draw call）
      if (calls > 50) window.__frames.push({ calls, shadowNeedsUpdate: before, t: performance.now() });
    };
    return 'ok';
  })()
`);

// 观察者机位 + 角色在走动 → 应当持续有"想重画"的请求
await evaluate(`(() => { const R = window.__ROOM__; R.controls.enabled = false;
  R.camera.position.set(24,23.4,34); R.camera.lookAt(0,8.4,7); R.camera.fov=42.3; R.camera.updateProjectionMatrix(); return 'ok'; })()`);

try {
  for (let i = 0; i < 60; i++) {
    await send('Emulation.setVirtualTimePolicy', {
      policy: 'pauseIfNetworkFetchesPending', budget: 20,
    });
  }
} catch (e) {
  report.shadow = { error: String(e).slice(0, 200) };
}
await sleep(500);

const frames = await evaluate('window.__frames.slice(0, 200)');
if (Array.isArray(frames) && frames.length) {
  const withShadow = frames.filter((f) => f.calls > 900).length;
  const wanted = frames.filter((f) => f.shadowNeedsUpdate).length;
  report.shadow = {
    sceneFrames: frames.length,
    framesWithShadowPass: withShadow,
    shadowPassRatio: +(withShadow / frames.length).toFixed(3),
    framesRequestingShadow: wanted,
    medianCalls: frames.map((f) => f.calls).sort((a, b) => a - b)[Math.floor(frames.length / 2)],
    callRange: [Math.min(...frames.map((f) => f.calls)), Math.max(...frames.map((f) => f.calls))],
  };
} else {
  report.shadow = { error: 'no frames captured', raw: frames };
}
console.log('[verify] shadow', JSON.stringify(report.shadow));

/* ---------- 3. 出图 ---------- */

await mkdir(SHOT_DIR, { recursive: true });
await evaluate(`(() => { const R = window.__ROOM__; window.__sky.renderOrder = 1000; window.__sky.material.depthTest = true;
  R.camera.position.set(24,23.4,34); R.camera.lookAt(0,8.4,7); R.camera.fov=42.3; R.camera.updateProjectionMatrix(); return 'ok'; })()`);
await sleep(1200);
for (const [name, cam] of Object.entries(cams)) {
  await evaluate(`(() => { const R = window.__ROOM__; R.controls.enabled=false;
    R.camera.position.set(${cam[0]},${cam[1]},${cam[2]}); R.camera.lookAt(${cam[3]},${cam[4]},${cam[5]});
    R.camera.fov=${cam[6]}; R.camera.updateProjectionMatrix(); return 'ok'; })()`);
  await sleep(900);
  const shot = await send('Page.captureScreenshot', { format: 'jpeg', quality: 72 });
  const p = join(SHOT_DIR, `${name}.jpg`);
  await writeFile(p, Buffer.from(shot.data, 'base64'));
  report.shots.push(p);
  console.log('[verify] shot', p);
}

report.pageErrors = pageErrors.slice(0, 10);
console.log('\n=== REPORT ===');
console.log(JSON.stringify(report, null, 2));

ws.close(); chrome.kill(); await sleep(600);
await rm(profile, { recursive: true, force: true }).catch(() => {});
