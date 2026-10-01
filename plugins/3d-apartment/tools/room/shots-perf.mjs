/**
 * 出图（本地 dev 工具，scripts/ 不入库）
 *
 * headless 下 rAF 冻结 ⇒ 合成器不出新帧 ⇒ Page.captureScreenshot 只会返回
 * 上一次合成的那一帧（改机位也不生效，四张图会一模一样，字节数都相同）。
 * 所以必须开 screencast 并逐帧 ack，让 app 循环真的跑起来，再截图。
 *
 * 用法：node plugins/3d-apartment/tools/room/shots-perf.mjs
 */
import { spawn } from 'node:child_process';
import { mkdtemp, rm, mkdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/preview.html';
const PORT = 9341;
const SHOT_DIR = 'G:/vivian-rs/plugins/3d-apartment/tools/room/shots';
const profile = await mkdtemp(join(tmpdir(), 'shots-'));
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
const errors = [];
let cast = 0;
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.method === 'Page.screencastFrame') {
    cast++;
    send('Page.screencastFrameAck', { sessionId: m.params.sessionId }).catch(() => {});
    return;
  }
  if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails.text);
  if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') errors.push('console: ' + JSON.stringify(m.params.args.map((a) => a.value ?? a.description)).slice(0, 200));
  if (m.id && pending.has(m.id)) { const p = pending.get(m.id); pending.delete(m.id); m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result); }
};
const send = (method, params = {}) => new Promise((resolve, reject) => { pending.set(++id, { resolve, reject }); ws.send(JSON.stringify({ id, method, params })); });
const evaluate = async (expression) => {
  const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails).slice(0, 400));
  return r.result.value;
};
await send('Page.enable'); await send('Runtime.enable');
for (let i = 0; i < 400; i++) { if (await evaluate('!!(window.__ROOM__ && window.__ROOM__.controls)')) break; await sleep(500); }
await sleep(3000);
console.log('[shots] scene ready');

await send('Page.startScreencast', { format: 'jpeg', quality: 25, maxWidth: 320, maxHeight: 220, everyNthFrame: 1 });
await sleep(1500);

// 让 app 循环把机位固定住：观察者模式下 controls.update() 每帧会把相机拉回
// OrbitControls 自己的球坐标，所以直接改 camera.position 会被覆盖 —— 改成
// 同时改 controls.target 并禁用 controls。
const shots = [
  ['perf-observer', [24, 23.4, 34, 0, 8.4, 7, 42.3]],
  ['perf-fps-window', [0, 5.0, 4, -6, 5.6, 9, 54.8]],
  ['perf-fps-room', [0, 5.0, 4, 6, 4.6, -2, 54.8]],
];
await mkdir(SHOT_DIR, { recursive: true });
const out = [];
for (const [name, c] of shots) {
  await evaluate(`(() => { const R = window.__ROOM__;
    R.controls.enabled = false;
    R.camera.position.set(${c[0]},${c[1]},${c[2]});
    R.camera.lookAt(${c[3]},${c[4]},${c[5]});
    R.camera.fov = ${c[6]}; R.camera.updateProjectionMatrix(); return 'ok'; })()`);
  const castBefore = cast;
  // 等真实帧产出（swiftshader ~3s/帧），并每 2s 重设一次机位防止被覆盖
  for (let i = 0; i < 8; i++) {
    await sleep(2000);
    await evaluate(`(() => { const R = window.__ROOM__; R.controls.enabled = false;
      R.camera.position.set(${c[0]},${c[1]},${c[2]});
      R.camera.lookAt(${c[3]},${c[4]},${c[5]}); return 'ok'; })()`);
  }
  const shot = await send('Page.captureScreenshot', { format: 'jpeg', quality: 78 });
  const p = join(SHOT_DIR, `${name}.jpg`);
  await writeFile(p, Buffer.from(shot.data, 'base64'));
  out.push({ name: p, bytes: Buffer.from(shot.data, 'base64').length, framesDuring: cast - castBefore });
  console.log('[shots]', name, 'bytes=', out[out.length - 1].bytes, 'frames=', out[out.length - 1].framesDuring);
}
await send('Page.stopScreencast');
console.log(JSON.stringify({ errors: errors.slice(0, 5), totalScreencastFrames: cast, out }, null, 2));
ws.close(); chrome.kill(); await sleep(600);
await rm(profile, { recursive: true, force: true }).catch(() => {});
