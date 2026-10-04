// Run against npm run dev. Exercise the real renderer via headless Chrome.
// @test-environment live-desktop
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, sep } from 'node:path';

const port = 9300 + Math.floor(Math.random() * 1000);
const profile = mkdtempSync(join(tmpdir(), 'vivian-idle-'));
const chrome = spawn('C:/Program Files/Google/Chrome/Application/chrome.exe', [
  '--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
  '--no-first-run', '--disable-background-timer-throttling',
  '--disable-renderer-backgrounding', '--disable-breakpad', 'about:blank',
], { stdio: 'ignore', windowsHide: true });
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
let socket;
try {
  let target;
  for (let i = 0; i < 60 && !target; i++) {
    try { target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find(p => p.type === 'page'); } catch {}
    if (!target) await delay(250);
  }
  assert(target, 'Chrome did not start');
  socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let id = 0;
  const pending = new Map();
  socket.onmessage = event => {
    const reply = JSON.parse(event.data);
    pending.get(reply.id)?.(reply);
    pending.delete(reply.id);
  };
  const send = (method, params = {}) => new Promise(resolve => {
    const requestId = ++id;
    const timer = setTimeout(() => {
      pending.delete(requestId);
      resolve({ error: { message: `Timed out: ${method}` } });
    }, 30000);
    pending.set(requestId, reply => { clearTimeout(timer); resolve(reply); });
    socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async expression => {
    const reply = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    assert(!reply.error, JSON.stringify(reply.error));
    assert(!reply.result.exceptionDetails, JSON.stringify(reply.result.exceptionDetails));
    return reply.result.result.value;
  };
  const open = async character => {
    await send('Page.navigate', { url: `http://localhost:1420/?view=rig_preview&character=${character}` });
    for (let i = 0; i < 60; i++) {
      if (await evaluate(`!!document.querySelector('.chibi-pet-${character}')`)) return;
      await delay(100);
    }
    throw new Error('Preview not ready');
  };
  const record = (button, duration, action = '', waitForIdle = false) => evaluate(`(async () => {
    const wait = ms => new Promise(r => setTimeout(r, ms));
    const stage = document.querySelector('.chibi-pet-stage');
    const sprite = document.querySelector('.chibi-pet-sprite');
    const samples = [];
    const click = text => [...document.querySelectorAll('button')].find(b => b.textContent.trim() === text).click();
    const timer = setInterval(() => samples.push({pose: stage.className, pos: sprite.style.backgroundPosition, sheet: sprite.style.backgroundImage}), 40);
    click(${JSON.stringify(button)});
    ${action}
    await wait(${duration});
    if (${waitForIdle}) {
      const deadline = performance.now() + 10000;
      while (!stage.className.includes('pose-idle') && performance.now() < deadline) await wait(40);
    }
    samples.push({pose: stage.className, pos: sprite.style.backgroundPosition, sheet: sprite.style.backgroundImage});
    clearInterval(timer); return samples;
  })()`);
  await open('vivian');
  let samples = await record('入睡', 6100);
  const tail = samples.slice(-45);
  assert(tail.every(s => s.pose.includes('pose-sleep')));
  assert.deepEqual([...new Set(tail.map(s => s.pos))].sort(), ['100% 100%', '66.6667% 100%'].sort());
  samples = await record('回到待机', 1500, '', true);
  assert(samples.some(s => s.pose.includes('pose-wake')));
  assert(samples.at(-1).pose.includes('pose-idle'));
  samples = await record('入睡', 6500, `await wait(4100); sprite.dispatchEvent(new MouseEvent('mousedown', {bubbles:true})); sprite.dispatchEvent(new MouseEvent('mouseup', {bubbles:true})); sprite.click();`);
  assert(samples.some(s => s.pose.includes('pose-wake')));
  assert(!samples.at(-1).pose.includes('pose-sleep'));
  // Speed up only the automatic wake timer; retain real frame timings.
  await evaluate(`window.originalTimeout = window.setTimeout; window.setTimeout = (fn, ms, ...args) => window.originalTimeout(fn, ms === 300000 ? 120 : ms, ...args)`);
  samples = await record('入睡', 5300, '', true);
  assert(samples.some(s => s.pose.includes('pose-wake')));
  assert(samples.at(-1).pose.includes('pose-idle'));
  await open('nana');
  samples = await record('浇花', 5300, '', true);
  const poses = [...new Set(samples.map(s => s.pose))];
  assert(poses.findIndex(s => s.includes('pose-tend ' ) || s.endsWith('pose-tend')) < poses.findIndex(s => s.includes('pose-tend-out')));
  assert(samples.some(s => s.sheet.includes('nana-tend-out-sheet.webp')));
  assert(samples.at(-1).pose.includes('pose-idle'));
  console.log('PASS: sleep tail loop, reset/click/timer wake, Nana tend → tend-out → idle');
  // Browser.close may close the socket before replying.
  socket.send(JSON.stringify({ id: 999998, method: 'Browser.close' }));
  await delay(500);
} finally {
  if (socket?.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify({ id: 999999, method: 'Browser.close' }));
    await delay(1000);
  }
  socket?.close();
  chrome.kill();
  // Delete only this test's verified temporary profile.
  assert(resolve(profile).startsWith(resolve(tmpdir()) + sep));
  assert(profile.includes('vivian-idle-'));
  for (let i = 0; i < 12; i++) {
    try { rmSync(profile, { recursive: true, force: true }); break; }
    catch { await delay(250); }
  }
}
