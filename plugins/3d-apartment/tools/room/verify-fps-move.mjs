/**
 * 第一人称功能回归 + 每帧统计（本地 dev 工具）
 *
 * headless 走 SwiftShader，一帧要好几秒，所以只能拿到个位数帧。够证明
 * 「进得去、走得动、不报错」，也够读到 HUD 的真实 draw call / fps 读数。
 * 帧率本身的绝对值在这里没有意义（软件光栅），真机数字要看 app 里的 HUD。
 *
 * 用 Page.startScreencast + 每帧 ack 强制合成器出帧 —— 不 ack 的话
 * Chrome 出完第一帧就停，rAF 也就冻结了。
 *
 * ⚠ 为什么每个方向要测**两轮**：rAF 帧在这个环境里极其稀疏（50s 才 ~13 帧），
 * 而且**进第一人称后的头几帧最贵**（新相机 = 全量新着色器变体 + 阴影重渲 +
 * Reflector 整场 pass）。所以「第一个按键窗口」可能一帧都没跑完，读数会是
 * 0.03m 这种假阴性——这跟按键有没有生效毫无关系。第一版把 W 放在最前面，
 * 就是这么误报的。现在改成 W/D 各测两轮：只有**两轮都为 0**才算真回归。
 *
 * 用法：node plugins/3d-apartment/tools/room/verify-fps-move.mjs
 */
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/preview.html';
const PORT = 9340;
const profile = await mkdtemp(join(tmpdir(), 'fps-move-'));
const chrome = spawn(CHROME, [
  '--headless=new', '--use-gl=angle', '--use-angle=swiftshader',
  '--enable-unsafe-swiftshader', '--no-sandbox', '--no-first-run',
  '--window-size=900,620', '--remote-allow-origins=*',
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
let castFrames = 0;
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.method === 'Page.screencastFrame') {
    castFrames++;
    send('Page.screencastFrameAck', { sessionId: m.params.sessionId }).catch(() => {});
    return;
  }
  if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails.text + ' ' + (m.params.exceptionDetails.exception?.description || '').slice(0, 200));
  if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') errors.push('console: ' + JSON.stringify(m.params.args.map((a) => a.value ?? a.description)).slice(0, 200));
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
console.log('[fps-move] scene ready');

// 帧计数器 + 每帧统计 hook
await evaluate(`
  (() => {
    const R = window.__ROOM__;
    const key = R.scene.children.find(o => o.isDirectionalLight && o.castShadow);
    window.__key = key;
    window.__frames = [];
    window.__rafN = 0;
    (function l(){ window.__rafN++; requestAnimationFrame(l); })();
    const orig = R.renderer.render.bind(R.renderer);
    R.renderer.render = (scene, camera) => {
      const before = key.shadow.needsUpdate;
      orig(scene, camera);
      const info = R.renderer.info.render;
      if (info.calls > 50) window.__frames.push({ calls: info.calls, tris: info.triangles, shadowWanted: before, t: +performance.now().toFixed(0) });
    };
    return 'ok';
  })()
`);

const key = async (type, k, code, vk) => send('Input.dispatchKeyEvent', {
  type, key: k, code, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk,
});
const pos = () => evaluate('(() => { const p = window.__ROOM__.camera.position; return [+p.x.toFixed(3), +p.y.toFixed(3), +p.z.toFixed(3)]; })()');
const rafN = () => evaluate('window.__rafN');
const dist = (a, b) => +Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]).toFixed(3);

// 强制出帧
await send('Page.startScreencast', { format: 'jpeg', quality: 20, maxWidth: 320, maxHeight: 220, everyNthFrame: 1 });
await sleep(2000);

const report = { errors, castFrames: 0, rafFrames: 0, pointerLock: null, phases: [], frameStats: null };

// 进第一人称
await key('keyDown', 'Enter', 'Enter', 13);
await key('keyUp', 'Enter', 'Enter', 13);
await sleep(6000);
report.pointerLock = await evaluate('document.pointerLockElement ? "LOCKED" : "NOT-LOCKED"');

// 预热：先按住 D 走一段（这一段读数丢弃），让第一人称的新着色器/阴影/反射
// 都编译并跑过一遍，避免第一个正式窗口撞在冷启动上
await key('keyDown', 'd', 'KeyD', 68);
await sleep(12000);
await key('keyUp', 'd', 'KeyD', 68);
console.log('[fps-move] warm-up done, rafN=' + (await rafN()));

/**
 * 一轮 = 按 W 测前进 → 按 D 测右移，每段都记录「这一段实际跑了几帧」，
 * 这样「位移为 0」到底是因为按键没生效、还是因为这一段一帧没跑，可区分。
 *
 * ⚠ 第二个假阴性来源：**同一个方向连测两轮会撞墙**。房间 bounds 是
 * x∈[-6.2,6.2] / z∈[-5.9,6.3]，`PLAYER_R=0.15`。实测 r1 的 D 把玩家从
 * x=2.825 平移到 x=5.575（离东墙只剩 0.62m），r2 再按 D 就只挪了 0.023m——
 * 那是**碰撞挡住**，不是按键失效。所以判定取「任一方向**任一**轮走出距离即通过」，
 * 并配合 framesInXxxWindow 一起看；不要看到某一轮为 0 就下结论。
 */
const round = async (tag, wMs, dMs) => {
  let before = await pos(); let rf0 = await rafN();
  await key('keyDown', 'w', 'KeyW', 87);
  await sleep(wMs);
  const afterW = await pos(); const rf1 = await rafN();
  await key('keyUp', 'w', 'KeyW', 87);

  await key('keyDown', 'd', 'KeyD', 68);
  await sleep(dMs);
  const afterD = await pos(); const rf2 = await rafN();
  await key('keyUp', 'd', 'KeyD', 68);

  const row = {
    tag, before, afterW, afterD,
    movedForward: dist(before, afterW),
    movedRight: dist(afterW, afterD),
    framesInForwardWindow: rf1 - rf0,
    framesInRightWindow: rf2 - rf1,
  };
  report.phases.push(row);
  console.log('[fps-move]', tag, JSON.stringify(row));
  return row;
};

await round('r1', 22000, 16000);
await round('r2', 22000, 16000);

await send('Page.stopScreencast');

report.castFrames = castFrames;
report.rafFrames = await evaluate('window.__rafN');
report.hud = await evaluate(`(() => [...document.querySelectorAll('div')].map(d => d.textContent || '').filter(t => /fps ·/.test(t)).slice(0, 2))()`);
const frames = await evaluate('window.__frames');
if (Array.isArray(frames) && frames.length) {
  const calls = frames.map((f) => f.calls);
  report.frameStats = {
    sceneFrames: frames.length,
    framesRequestingShadow: frames.filter((f) => f.shadowWanted).length,
    framesWithShadowPass: frames.filter((f) => f.calls > 900).length,
    callsMin: Math.min(...calls), callsMax: Math.max(...calls),
    trisMaxK: Math.round(Math.max(...frames.map((f) => f.tris)) / 1000),
  };
}

// 判定：任一方向只要**有一轮**走出距离即算通过；两轮都为 0 才算真回归。
// 另外把「该窗口 0 帧」的情况单独标出来——那是环境问题，不是代码问题。
const fwd = report.phases.map((p) => p.movedForward);
const rgt = report.phases.map((p) => p.movedRight);
const fwdFrames = report.phases.map((p) => p.framesInForwardWindow);
report.verdict = {
  enteredFirstPerson: report.pointerLock === 'LOCKED',
  movedForward: Math.max(...fwd) > 0.2,
  movedRight: Math.max(...rgt) > 0.1,
  forwardWindowsWithoutFrames: fwdFrames.filter((n) => n === 0).length,
  noPageErrors: errors.length === 0,
  framesRendered: report.rafFrames,
  note: 'forwardWindowsWithoutFrames>0 说明该窗口一帧没跑，读数无意义（环境限制，非代码问题）',
};

console.log(JSON.stringify(report, null, 2));
ws.close(); chrome.kill(); await sleep(600);
await rm(profile, { recursive: true, force: true }).catch(() => {});
