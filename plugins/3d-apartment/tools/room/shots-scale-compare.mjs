/**
 * 渲染倍率清晰度对照出图（本地 dev 工具，scripts/ 不入库）
 *
 * 同一机位、同一帧内容，只在 1.0× 与 0.7× 各出一张，供人肉判断降档代价。
 * 必须开 screencast 并逐帧 ack，否则合成器不出新帧、截图只会拿到同一张旧帧。
 *
 * 用法：node plugins/3d-apartment/tools/room/shots-scale-compare.mjs
 */
import { spawn } from 'node:child_process';
import { mkdtemp, rm, mkdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/preview.html';
const PORT = 9343;
const SHOT_DIR = 'G:/vivian-rs/plugins/3d-apartment/tools/room/shots';
const profile = await mkdtemp(join(tmpdir(), 'scale-cmp-'));
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
let cast = 0;
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.method === 'Page.screencastFrame') {
    cast++;
    send('Page.screencastFrameAck', { sessionId: m.params.sessionId }).catch(() => {});
    return;
  }
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
  if (await evaluate('!!(window.__ROOM__ && window.__ROOM__.getRenderScale)')) break;
  await sleep(500);
}
await sleep(2500);
console.log('[scale-cmp] scene ready');

await send('Page.startScreencast', { format: 'jpeg', quality: 25, maxWidth: 320, maxHeight: 220, everyNthFrame: 1 });
await sleep(1500);
await mkdir(SHOT_DIR, { recursive: true });

// 第一人称客厅机位，朝窗外街区（细节最多、最能看出清晰度差）
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

const out = [];
for (const [name, idx] of [['scale-100', 0], ['scale-070', 3]]) {
  await evaluate(`window.__ROOM__.setRenderScale(${idx})`);
  await hold(10000);
  const shot = await send('Page.captureScreenshot', { format: 'jpeg', quality: 88 });
  const buf = Buffer.from(shot.data, 'base64');
  const p = join(SHOT_DIR, `${name}.jpg`);
  await writeFile(p, buf);
  const st = await evaluate('window.__ROOM__.getRenderScale()');
  out.push({ name: p, bytes: buf.length, dpr: st.dpr, canvas: st.canvas, screencastFrames: cast });
  console.log('[scale-cmp]', name, 'bytes=', buf.length, 'dpr=', st.dpr, 'canvas=', st.canvas.join('x'));
}
await evaluate('window.__ROOM__.setRenderScale(0)');
await send('Page.stopScreencast');
console.log(JSON.stringify(out, null, 2));
ws.close(); chrome.kill(); await sleep(600);
await rm(profile, { recursive: true, force: true }).catch(() => {});
