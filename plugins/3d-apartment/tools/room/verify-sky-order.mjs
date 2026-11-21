/**
 * 天空球绘制顺序改动 —— 逐像素对照诊断（本地 dev 工具）
 *
 * 上一轮测得新旧设置像素差很大，但没做对照，无法区分"真的变了"和"渲染本身
 * 不可重复"。这里补齐三个对照：
 *   A) old vs old   —— 同一设置连渲两次，量化 harness 自身的噪声
 *   B) 隐藏天空 old vs new —— 天空不参与时，顺序改动应当毫无影响
 *   C) 显示天空 old vs new —— 真正关心的量
 * 并输出差异像素的包围盒 / 最大差像素的两侧取值，用来定位是哪块区域变了。
 *
 * 用法：node plugins/3d-apartment/tools/room/verify-sky-order.mjs
 */
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/preview.html';
const PORT = 9339;
const profile = await mkdtemp(join(tmpdir(), 'sky-order-'));
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
for (let i = 0; i < 400; i++) { if (await evaluate('!!(window.__ROOM__ && window.__ROOM__.controls)')) break; await sleep(500); }
await sleep(3000);
console.log('[sky-order] scene ready');

await evaluate(`
  (() => {
    const R = window.__ROOM__, THREE = R.THREE;
    let sky = null; R.scene.traverse(o => { if (o.name === 'district-atmosphere') sky = o; });
    const W = 176, H = 120;
    const rt = new THREE.WebGLRenderTarget(W, H);
    const slots = {};
    window.__H = H; window.__W = W;
    window.__shoot = (slot, opts) => {
      sky.visible = opts.sky !== false;
      sky.renderOrder = opts.order;
      sky.material.depthTest = opts.depthTest;
      const prev = R.renderer.getRenderTarget();
      R.renderer.setRenderTarget(rt);
      R.renderer.clear();
      R.renderer.render(R.scene, R.camera);
      const buf = new Uint8Array(W * H * 4);
      R.renderer.readRenderTargetPixels(rt, 0, 0, W, H, buf);
      R.renderer.setRenderTarget(prev);
      slots[slot] = buf;
      return 'ok';
    };
    window.__cmp = (a, b) => {
      const A = slots[a], B = slots[b];
      let maxAbs = 0, sum = 0, n = A.length, idx = -1;
      let x0 = 1e9, y0 = 1e9, x1 = -1, y1 = -1, cnt = 0;
      for (let i = 0; i < n; i++) {
        const d = Math.abs(A[i] - B[i]);
        if (d > maxAbs) { maxAbs = d; idx = i; }
        sum += d;
        if (d > 4) { cnt++; const px = (i >> 2) % W, py = (i >> 2) / W | 0; if (px < x0) x0 = px; if (px > x1) x1 = px; if (py < y0) y0 = py; if (py > y1) y1 = py; }
      }
      const pi = idx - (idx % 4);
      return { maxAbsDiff: maxAbs, meanAbsDiff: +(sum / n).toFixed(4),
        diffPixels: cnt, diffPct: +(100 * cnt / (n / 4)).toFixed(1),
        bbox: cnt ? [x0, y0, x1, y1] : null,
        atMax: { A: [A[pi], A[pi+1], A[pi+2], A[pi+3]], B: [B[pi], B[pi+1], B[pi+2], B[pi+3]] } };
    };
    window.__sky = sky;
    return 'ok';
  })()
`);

const OLD = { order: -1000, depthTest: false };
const NEW = { order: 1000, depthTest: true };

const cams = {
  observer_outdoor: [24, 23.4, 34, 0, 8.4, 7, 42.3],
  fps_room_wall: [0, 5.0, 4, 6, 4.6, -2, 54.8],
};
const out = {};
for (const [label, c] of Object.entries(cams)) {
  await evaluate(`(() => { const R = window.__ROOM__; R.controls.enabled = false;
    R.camera.position.set(${c[0]},${c[1]},${c[2]}); R.camera.lookAt(${c[3]},${c[4]},${c[5]});
    R.camera.fov = ${c[6]}; R.camera.updateProjectionMatrix(); return 'ok'; })()`);
  // 预热 3 次：第一次渲染会现算平面反射的 RT，之后才稳定。丢掉预热帧，
  // 否则量到的是"首帧 vs 次帧"，不是"旧设置 vs 新设置"。
  for (let i = 0; i < 3; i++) await evaluate(`window.__shoot('warm${i}', ${JSON.stringify({ ...OLD, sky: true })})`);
  // 交替各拍 3 张：old/new/old/new/old/new
  for (let i = 0; i < 3; i++) {
    await evaluate(`window.__shoot('old${i}', ${JSON.stringify({ ...OLD, sky: true })})`);
    await evaluate(`window.__shoot('new${i}', ${JSON.stringify({ ...NEW, sky: true })})`);
  }
  out[label] = {
    '对照·old_vs_old(同一设置)': await evaluate(`window.__cmp('old0','old1')`),
    '对照·new_vs_new(同一设置)': await evaluate(`window.__cmp('new0','new1')`),
    '关心·old_vs_new': await evaluate(`window.__cmp('old0','new0')`),
    '关心·old_vs_new(第二组)': await evaluate(`window.__cmp('old1','new1')`),
  };
  console.log('[sky-order]', label, JSON.stringify(out[label], null, 1));
}

// 恢复默认
await evaluate(`(() => { const s = window.__sky; s.visible = true; s.renderOrder = 1000; s.material.depthTest = true; return 'ok'; })()`);
console.log('\n=== SUMMARY ===');
console.log(JSON.stringify(out, null, 2));
ws.close(); chrome.kill(); await sleep(600);
await rm(profile, { recursive: true, force: true }).catch(() => {});
