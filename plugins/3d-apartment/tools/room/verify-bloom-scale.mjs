/**
 * 泛光降分辨率验证（本地 dev 工具，scripts/ 不入库）
 *
 * 被验证的改动：把 UnrealBloomPass 的 setSize 包一层，让它的内部 RT 按
 * bloomScale（默认 0.5）再降一档，从而把泛光的填充量砍到 1/4。
 *
 * 断言四件事：
 *  1) 默认档位下，泛光 mip0 的 RT 恒等于 **composer RT 的 1/4**
 *     （bloomScale 0.5 × UnrealBloomPass 自己内部的 /2 = /4）；
 *  2) 切渲染倍率时泛光 RT 跟着一起变——包装没漏掉 applyRenderScale 这条路径；
 *  3) bloomScale 可 A/B 且完全可逆：置 1 时 RT 回到 composer RT 的 /2
 *     （即改动前的原始行为），置 0.25 时到 /8；
 *  4) 改完场景仍正常出图（非空白）、无 page error。
 *
 * 注意：泛光 RT 尺寸是**两次取整**的结果（外层 bloomScale、内层 /2），
 * 所以断言容差取 2px，不是 0。
 *
 * 用法：node plugins/3d-apartment/tools/room/verify-bloom-scale.mjs
 */
import { spawn } from 'node:child_process';
import { mkdtemp, rm, mkdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/preview.html';
const PORT = 9346;
const SHOT_DIR = 'G:/vivian-rs/plugins/3d-apartment/tools/room/shots';
const profile = await mkdtemp(join(tmpdir(), 'bloom-'));
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

for (let i = 0; i < 300; i++) {
  if (await evaluate('!!(window.__ROOM__ && window.__ROOM__.getBloomScale)')) break;
  await sleep(500);
}
if (!(await evaluate('!!(window.__ROOM__ && window.__ROOM__.getBloomScale)'))) throw new Error('getBloomScale never ready');
await sleep(2500);
console.log('[bloom] scene ready');

// 非空白探针（同 verify-render-scale）：直接 renderer.render 到自建 RT 读像素
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

const report = { defaultScale: null, steps: [], ab: [], errors: [] };
const snap = () => evaluate('({ ...window.__ROOM__.getRenderScale(), ...window.__ROOM__.getBloomScale() })');
const TOL = 2;

// —— 1) 各渲染倍率档位下，泛光 RT 必须跟着 composer RT 同步变化 ——
for (const idx of [0, 1, 2, 3]) {
  await evaluate(`window.__ROOM__.setRenderScale(${idx})`);
  await sleep(400);
  const s = await snap();
  const wantW = s.composerRT[0] * s.scale / 2;
  const wantH = s.composerRT[1] * s.scale / 2;
  const row = {
    scaleIdx: idx, dpr: s.dpr, composerRT: s.composerRT, bloomScale: s.scale, bloomRT: s.bloomRT,
    want: [+wantW.toFixed(1), +wantH.toFixed(1)],
    ok: Math.abs(s.bloomRT[0] - wantW) <= TOL && Math.abs(s.bloomRT[1] - wantH) <= TOL,
    probe: await evaluate('window.__probe()'),
  };
  report.steps.push(row);
  console.log('[bloom] step', JSON.stringify(row));
}

// —— 2) A/B 可逆性：1 = 关掉这项优化（原始行为 /2），0.25 = 更激进 ——
await evaluate('window.__ROOM__.setRenderScale(0)');
await sleep(300);
for (const v of [1, 0.25, 0.5]) {
  await evaluate(`window.__ROOM__.setBloomScale(${v})`);
  await sleep(500);
  const s = await snap();
  const wantW = s.composerRT[0] * s.scale / 2;
  const row = {
    bloomScale: v, composerRT: s.composerRT, bloomRT: s.bloomRT,
    ratioToComposer: +(s.bloomRT[0] / s.composerRT[0]).toFixed(4),
    want: +wantW.toFixed(1),
    ok: Math.abs(s.bloomRT[0] - wantW) <= TOL,
  };
  report.ab.push(row);
  console.log('[bloom] ab', JSON.stringify(row));
}
report.defaultScale = await evaluate('window.__ROOM__.getBloomScale().scale');

// —— 3) 出图对照：同一机位、渲染倍率都取 1.0×，只改泛光分辨率 ——
await send('Page.startScreencast', { format: 'jpeg', quality: 25, maxWidth: 320, maxHeight: 220, everyNthFrame: 1 });
await sleep(1500);
await mkdir(SHOT_DIR, { recursive: true });
const CAM = [0, 5.0, 4, -6, 5.6, 9, 54.8];
const hold = async (ms) => {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    await sleep(2000);
    await evaluate(`(() => { const R = window.__ROOM__; R.controls.enabled = false;
      R.camera.position.set(${CAM[0]},${CAM[1]},${CAM[2]});
      R.camera.lookAt(${CAM[3]},${CAM[4]},${CAM[5]});
      R.camera.fov = ${CAM[6]}; R.camera.updateProjectionMatrix(); return 'ok'; })()`);
  }
};
const shots = [];
for (const [name, v] of [['bloom-050', 0.5], ['bloom-100', 1]]) {
  await evaluate(`window.__ROOM__.setBloomScale(${v})`);
  await hold(9000);
  const shot = await send('Page.captureScreenshot', { format: 'jpeg', quality: 88 });
  const buf = Buffer.from(shot.data, 'base64');
  const p = join(SHOT_DIR, `${name}.jpg`);
  await writeFile(p, buf);
  const s = await snap();
  shots.push({ name: p, bloomScale: v, bytes: buf.length, bloomRT: s.bloomRT, composerRT: s.composerRT });
  console.log('[bloom] shot', name, 'bytes=', buf.length, 'bloomRT=', s.bloomRT.join('x'));
}
await evaluate('window.__ROOM__.setBloomScale(0.5)');
await send('Page.stopScreencast');
report.shots = shots;
report.screencastFrames = cast;
report.errors = errors.slice(0, 8);

console.log('\n=== REPORT ===');
console.log(JSON.stringify(report, null, 2));
const allOk = report.steps.every((s) => s.ok) && report.ab.every((s) => s.ok);
console.log('ALL_OK=' + allOk);
ws.close(); chrome.kill(); await sleep(600);
await rm(profile, { recursive: true, force: true }).catch(() => {});
