/**
 * 自适应渲染倍率验证（本地 dev 工具，scripts/ 不入库）
 *
 * 断言三件事：
 *  1) 手工切档时，renderer 的 canvas 与 composer 的 RT **同步**缩小（只改
 *     renderer 不改 composer 的话，后处理仍按旧分辨率画，画面会糊）；
 *  2) 切档后场景仍能正常渲染、无 page error，且画面不是空白/单色；
 *  3) 防护逻辑成立：headless 帧时 ≥120ms 时**不允许**自动降档
 *     （否则探针一跑就把画质降下去，真机也会在切后台时误降）。
 *
 * 用法：node plugins/3d-apartment/tools/room/verify-render-scale.mjs
 */
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/preview.html';
const PORT = 9342;
const profile = await mkdtemp(join(tmpdir(), 'rs-'));
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
const errors = [];
let cast = 0;
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.method === 'Page.screencastFrame') {
    cast++;
    send('Page.screencastFrameAck', { sessionId: m.params.sessionId }).catch(() => {});
    return;
  }
  if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails.text + ' ' + (m.params.exceptionDetails.exception?.description || '').slice(0, 160));
  if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') errors.push('console: ' + JSON.stringify(m.params.args.map((a) => a.value ?? a.description)).slice(0, 160));
  if (m.id && pending.has(m.id)) { const p = pending.get(m.id); pending.delete(m.id); m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result); }
};
const send = (method, params = {}) => new Promise((resolve, reject) => { pending.set(++id, { resolve, reject }); ws.send(JSON.stringify({ id, method, params })); });
const evaluate = async (expression) => {
  const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails).slice(0, 400));
  return r.result.value;
};
await send('Page.enable'); await send('Runtime.enable');

async function waitReady() {
  for (let i = 0; i < 300; i++) {
    if (await evaluate('!!(window.__ROOM__ && window.__ROOM__.controls && window.__ROOM__.getRenderScale)')) return true;
    await sleep(500);
  }
  return false;
}
if (!(await waitReady())) throw new Error('scene / getRenderScale never ready');
await sleep(2500);
console.log('[render-scale] scene ready');

// 渲染到自建 RT 并统计"是不是空白"（非单色即视为正常出图）
await evaluate(`
  (() => {
    const R = window.__ROOM__, THREE = R.THREE;
    const W = 96, H = 64;
    const rt = new THREE.WebGLRenderTarget(W, H);
    window.__probe = () => {
      const prev = R.renderer.getRenderTarget();
      R.renderer.setRenderTarget(rt);
      R.renderer.clear();
      R.renderer.render(R.scene, R.camera);
      const buf = new Uint8Array(W * H * 4);
      R.renderer.readRenderTargetPixels(rt, 0, 0, W, H, buf);
      R.renderer.setRenderTarget(prev);
      let min = 255, max = 0, sum = 0;
      for (let i = 0; i < buf.length; i += 4) {
        const v = (buf[i] + buf[i+1] + buf[i+2]) / 3;
        if (v < min) min = v; if (v > max) max = v; sum += v;
      }
      return { min, max, mean: +(sum / (buf.length / 4)).toFixed(1) };
    };
    return 'ok';
  })()
`);

const report = { errors, steps: [], guard: null };

for (const idx of [0, 1, 2, 3, 0]) {
  await evaluate(`window.__ROOM__.setRenderScale(${idx})`);
  await sleep(400);
  const st = await evaluate('window.__ROOM__.getRenderScale()');
  const probe = await evaluate('window.__probe()');
  const css = await evaluate('(() => { const c = window.__ROOM__.renderer.domElement; return [c.clientWidth, c.clientHeight]; })()');
  const expectDpr = await evaluate('Math.min(window.devicePixelRatio, 1.5) * ' + [1, 0.9, 0.8, 0.7][idx]);
  report.steps.push({
    idx, ...st, cssSize: css, expectDpr: +expectDpr.toFixed(4),
    dprOk: Math.abs(st.dpr - expectDpr) < 1e-6,
    canvasOk: Math.abs(st.canvas[0] - Math.round(css[0] * expectDpr)) <= 1,
    composerRTOk: Math.abs(st.composerRT[0] - Math.round(css[0] * expectDpr)) <= 1,
    probe,
  });
  console.log('[render-scale] idx=' + idx, JSON.stringify(report.steps[report.steps.length - 1]));
}

// 防护验证：headless 帧时是秒级（≥120ms），驱动真实帧后倍率必须仍是 0
await send('Page.startScreencast', { format: 'jpeg', quality: 20, maxWidth: 260, maxHeight: 180, everyNthFrame: 1 });
const before = await evaluate('window.__ROOM__.getRenderScale().idx');
await sleep(30000);
const after = await evaluate('window.__ROOM__.getRenderScale().idx');
const frames = await evaluate('window.__rafN ?? null');
await send('Page.stopScreencast');
report.guard = {
  idxBeforeFrames: before, idxAfterFrames: after,
  screencastFrames: cast,
  heldFullScale: after === 0,
  note: 'headless 帧时 ≥120ms，按设计不得自动降档',
};
console.log('[render-scale] guard', JSON.stringify(report.guard));

report.errors = errors.slice(0, 8);
console.log('\n=== REPORT ===');
console.log(JSON.stringify(report, null, 2));
ws.close(); chrome.kill(); await sleep(600);
await rm(profile, { recursive: true, force: true }).catch(() => {});
