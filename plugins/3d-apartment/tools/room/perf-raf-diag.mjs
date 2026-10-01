/**
 * rAF / 帧时序诊断（本地 dev 工具）
 * 检查 headless 下 rAF 是否真的在跑；不跑就靠 screencast 强制出帧。
 */
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/preview.html';
const PORT = 9334;
const profile = await mkdtemp(join(tmpdir(), 'raf-diag-'));
const chrome = spawn(CHROME, [
  '--headless=new', '--use-gl=angle', '--use-angle=swiftshader',
  '--enable-unsafe-swiftshader', '--no-sandbox', '--no-first-run',
  '--window-size=1440,1000', '--remote-allow-origins=*',
  `--remote-debugging-port=${PORT}`, `--user-data-dir=${profile}`, URL,
], { stdio: 'ignore' });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function findPage() {
  for (let i = 0; i < 120; i++) {
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
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails).slice(0, 400));
  return r.result.value;
};
await send('Page.enable'); await send('Runtime.enable');

for (let i = 0; i < 240; i++) { if (await evaluate('!!(window.__ROOM__ && window.__ROOM__.controls)')) break; await sleep(500); }
console.log('[raf-diag] scene ready');

// 裸 rAF 计数（不依赖 app）
await evaluate(`window.__rafN = 0; (function l(){ window.__rafN++; requestAnimationFrame(l); })(); 'ok'`);
await sleep(2000);
const withoutCast = await evaluate('window.__rafN');

// 打开 screencast 强制合成器出帧
await send('Page.startScreencast', { format: 'jpeg', quality: 30, maxWidth: 400, maxHeight: 300 });
await evaluate('window.__rafN = 0');
await sleep(2000);
const withCast = await evaluate('window.__rafN');
const vis = await evaluate('document.visibilityState');
console.log(JSON.stringify({ visibility: vis, rafPer2s_noScreencast: withoutCast, rafPer2s_withScreencast: withCast }, null, 2));

ws.close(); chrome.kill(); await sleep(600);
await rm(profile, { recursive: true, force: true }).catch(() => {});
