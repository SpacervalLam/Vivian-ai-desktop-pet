/**
 * 书店（street-bookstore）碰撞与可走性验证（只读，不改源码；scripts/ 不入库）
 *
 * 背景：书店 1F 原先是一整块实心体量（`gbox(W,H1,D)`），玻璃后面贴块暖光板冒充店内。
 * 这一轮把 1F 挖成真营业厅（地板/吊顶/两侧内墙/三段墙垛 + 书架/陈列台/柜台），
 * 碰撞改用**声明式盒子**（exterior.ts `solid()` / `mass()`，与几何同一行登记）。
 *
 * 为什么必须单独验一次：
 *   1. 声明式盒子是手写坐标，写错一位就是「门被堵死」或「橱窗能穿过去」，
 *      而这两种错误在截图里都看不出来（前者是能看见店内但走不进去）。
 *   2. 橱窗玻璃透明度 0.085，buildSceneColliders 的「transparent && opacity<0.5 跳过」
 *      规则会把整块玻璃跳过——如果这里没登记盒子，玩家会直接穿过橱窗走进店里。
 *   3. 地板刻意用**平面**而不是厚盒（厚盒 0.13m 高，而落脚面仍是街道 floor y=0，
 *      进店会陷进地板 12cm）。这条只有跑 supportY 才验得出来。
 *
 * 本脚本在真实场景里拿 window.__ROOM__.colliders（RoomScene.tsx 挂的自检钩子），
 * 复刻 fpsControls.ts 的 collidesAt / supportY 规则，做四件事：
 *   A. 声明式盒子确实进了盒表（按 source 前缀 bookstore- 点名核对）；
 *   B. 直线行走：门口能进、橱窗/侧墙/后段实心体量/柜台货架都挡得住；
 *   C. 洪水填充：从街面出发能不能走到店内深处（没有气密密封）；
 *   D. 落脚面：店内 supportY == 0（街道地面），不陷不抬。
 *
 * 用法：node plugins/3d-apartment/tools/room/verify-bookstore.mjs   （需要 1420 dev server 在跑）
 * 退出码：任一断言失败 → 1
 */
import { spawn } from 'node:child_process';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const URL = 'http://localhost:1420/plugins/3d-apartment/preview.html';
const PORT = Number(process.env.PROBE_PORT || 9396);
const profile = await mkdtemp(join(tmpdir(), 'bookstore-'));
const chrome = spawn(CHROME, [
  '--headless=new', '--use-gl=angle', '--use-angle=swiftshader',
  '--enable-unsafe-swiftshader', '--no-sandbox', '--no-first-run',
  '--window-size=900,600', '--remote-allow-origins=*',
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
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { const p = pending.get(m.id); pending.delete(m.id); m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result); }
};
const send = (method, params = {}) => new Promise((resolve, reject) => { pending.set(++id, { resolve, reject }); ws.send(JSON.stringify({ id, method, params })); });
const evaluate = async (expression) => {
  const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails).slice(0, 4000));
  return r.result.value;
};
await send('Runtime.enable');
let ready = false;
for (let i = 0; i < 300; i++) {
  if (await evaluate('!!(window.__ROOM__ && window.__ROOM__.colliders && window.__ROOM__.colliders.length)')) { ready = true; break; }
  await sleep(500);
}
if (!ready) { console.error('✗ 等不到 window.__ROOM__.colliders（自检钩子没挂上？）'); chrome.kill(); process.exit(1); }
await sleep(1500);

const out = await evaluate(`
(() => {
  const R = window.__ROOM__;
  const THREE = R.THREE;   // 页面里没挂全局 THREE，必须从自检钩子拿
  // ---- 把盒表转成纯数组（Vector3 走 returnByValue 会变成 {x,y,z}，这里统一拍平）----
  const all = R.colliders.map(c => ({
    id: c.source || '', kind: c.kind,
    x0: c.min.x, x1: c.max.x, y0: c.min.y, y1: c.max.y, z0: c.min.z, z1: c.max.z,
  }));
  const bad = all.filter(c => [c.x0,c.x1,c.y0,c.y1,c.z0,c.z1].some(v => !Number.isFinite(v)));
  const mine = all.filter(c => c.id.startsWith('bookstore-'));

  // ---- 复刻 fpsControls.ts ----
  const STEP_UP = 0.45, R_ = 0.15;
  /** collidesAt：footY=0（玩家站在街道地面），headY=1.7 */
  function blocked(x, z, footY = 0) {
    const headY = footY + 1.6 + 0.1;
    for (const c of all) {
      if (c.kind === 'ramp') continue;
      if (c.y0 > headY) continue;
      if (c.y1 < footY + 0.12) continue;
      const boxH = c.y1 - c.y0;
      if (boxH <= STEP_UP && c.y1 <= footY + STEP_UP) continue;
      const cx = Math.max(c.x0, Math.min(x, c.x1));
      const cz = Math.max(c.z0, Math.min(z, c.z1));
      const dx = x - cx, dz = z - cz;
      if (dx * dx + dz * dz < R_ * R_) return true;
    }
    return false;
  }
  /** supportY：返回脚下最高且不超过 footNow+STEP 的落脚面 */
  function support(x, z, footNow = 0) {
    let best = null;
    for (const c of all) {
      if (c.kind === 'ramp') continue;
      const cx = Math.max(c.x0, Math.min(x, c.x1));
      const cz = Math.max(c.z0, Math.min(z, c.z1));
      const dx = x - cx, dz = z - cz;
      if (dx * dx + dz * dz > R_ * R_) continue;
      const top = c.y1;
      if (top <= footNow + STEP_UP && (best == null || top > best)) best = top;
    }
    return best;
  }
  /** 从 (x0,z0) 沿 (dx,dz) 方向一步一步走，步长 0.02，返回停下来的坐标与走了多远 */
  function walk(x0, z0, dx, dz, maxDist) {
    const len = Math.hypot(dx, dz); dx /= len; dz /= len;
    let x = x0, z = z0, d = 0;
    const step = 0.02;
    while (d < maxDist) {
      const nx = x + dx * step, nz = z + dz * step;
      if (blocked(nx, nz)) break;
      x = nx; z = nz; d += step;
    }
    return { x: +x.toFixed(3), z: +z.toFixed(3), dist: +d.toFixed(3) };
  }
  /** 洪水填充：从街面出发点扩散，返回可达集合（用于「门口没被密封」） */
  function flood(sx, sz, x0, x1, z0, z1, step) {
    const nx = Math.round((x1 - x0) / step) + 1, nz = Math.round((z1 - z0) / step) + 1;
    const seen = new Uint8Array(nx * nz);
    const idx = (i, j) => i * nz + j;
    const si = Math.round((sx - x0) / step), sj = Math.round((sz - z0) / step);
    if (si < 0 || sj < 0 || si >= nx || sj >= nz) return { seen, nx, nz, x0, z0, step, ok: false };
    const stack = [[si, sj]];
    seen[idx(si, sj)] = 1;
    while (stack.length) {
      const [i, j] = stack.pop();
      for (const [di, dj] of [[1,0],[-1,0],[0,1],[0,-1]]) {
        const ii = i + di, jj = j + dj;
        if (ii < 0 || jj < 0 || ii >= nx || jj >= nz) continue;
        const k = idx(ii, jj);
        if (seen[k]) continue;
        if (blocked(x0 + ii * step, z0 + jj * step)) { seen[k] = 2; continue; }
        seen[k] = 1; stack.push([ii, jj]);
      }
    }
    return { seen, nx, nz, x0, z0, step, ok: true };
  }

  // ================= A. 声明式盒子是否进了盒表 =================
  const WANT = ['bookstore-back-mass','bookstore-wall-w','bookstore-wall-e',
    'bookstore-pier-w','bookstore-pier-m','bookstore-pier-e','bookstore-shopfront',
    'bookstore-shelf-a','bookstore-shelf-b','bookstore-shelf-c','bookstore-shelf-back',
    'bookstore-table','bookstore-rack','bookstore-counter'];
  const have = new Set(mine.map(c => c.id));
  const missing = WANT.filter(w => !have.has(w));
  const kinds = [...new Set(mine.map(c => c.kind))];

  // ================= B. 直线行走 =================
  // 体量：x 12.7..19.7 / z 15.0..26.1 / 4 层 12.4m；门洞 x 17.90..18.80（轴 18.35）；
  // 橱窗玻璃 zG=14.99（挡人面 14.87）；内墙面 x 12.84 / 19.56；后墙 zB=24.0。
  // B1 门口：从人行道 (18.35,13.20) 往 +z 走，穿过门洞一直走到柜台前（z≈23.29）
  const door = walk(18.35, 13.20, 0, 1, 11);
  // B2 橱窗：从人行道 (14.94,13.20) 往 +z 走，应当被橱窗玻璃挡在 z≈14.72
  const glass = walk(14.94, 13.20, 0, 1, 4);
  // B3 西内墙：书架 A/B 之间的横走道 (15.40,18.60) 往 -x 走，应挡在 x≈12.99
  //    走道净宽 0.92（A 南面 18.14 → B 北面 19.06），玩家半径 0.15 ⇒ 可站区间
  //    z 18.29..18.91。取 18.60 居中；取 19.00 会贴着 B 的北面（0.06 < R）判成卡死。
  const wallW = walk(15.40, 18.60, -1, 0, 5);
  // B4 东内墙：同一条横走道 (17.00,18.60) 往 +x 走，应挡在 x≈19.41
  const wallE = walk(17.00, 18.60, 1, 0, 5);
  // B5 柜台：东侧主走道 (18.35,22.00) 往 +z 走，应挡在 z≈23.29
  const counter = walk(18.35, 22.00, 0, 1, 5);
  // B6 书架：书架 C 以南 (15.40,22.50) 往 -z 走，应挡在 z≈21.09
  const shelf = walk(15.40, 22.50, 0, -1, 5);
  // B7 门洞走廊逐点扫：x=18.35，z 从 13.60 到 15.40，不许有一点被挡（气密密封检测）
  const corridor = [];
  for (let z = 13.60; z <= 15.401; z += 0.05) corridor.push([+z.toFixed(2), blocked(18.35, z)]);

  // ================= C. 洪水填充：街面 → 店内 =================
  const F = flood(18.35, 13.80, 11.0, 21.5, 13.0, 25.2, 0.1);
  const reach = (x, z) => {
    const i = Math.round((x - F.x0) / F.step), j = Math.round((z - F.z0) / F.step);
    if (i < 0 || j < 0 || i >= F.nx || j >= F.nz) return -1;
    return F.seen[i * F.nz + j];
  };
  // 店内的四个点：东侧主走道 / 中横走道 / 前场 / 后场
  const inner = [
    { id: '东侧主走道', x: 18.35, z: 21.50 },
    { id: '中横走道', x: 15.40, z: 18.60 },
    { id: '前场陈列区', x: 14.00, z: 17.20 },
    { id: '后场柜台前', x: 14.00, z: 22.50 },
  ].map(p => ({ ...p, state: reach(p.x, p.z) }));
  /* 对照两条，方向相反，缺一不可：
   *   街面 (13.00,13.40) 必须可达 —— 证明洪水填充没跑偏（全 0 也能"通过"可达性断言）；
   *   后段实心体量内部 (16.20,25.00) 必须不可达 —— 证明西/北两面墙是真封住的。
   * 旧版的对照点 (11.60,21.00) 现在**本来就不可达**：居酒屋（x 7.5..11.7 / z 14..17.8）
   * 把 x<11.7 的南北通路截断了，要绕到居酒屋西侧才过得去，那已经在洪水窗口之外。 */
  const ctrlStreet = { x: 13.00, z: 13.40, state: reach(13.00, 13.40) };
  const ctrlSolid = { x: 16.20, z: 25.00, state: reach(16.20, 25.00) };

  // ================= D. 落脚面 =================
  const floorAt = [
    { id: '店内东走道', x: 18.35, z: 21.00 },
    { id: '店内中走道', x: 15.40, z: 18.60 },
    // 门口取 z=15.40（前墙内表面 15.06 以南），不取 14.60 —— 14.60 还在人行道
    // 铺装带（12.6..15.0）上，落脚面是 0.048，会被当成"店内不一致"。
    { id: '店门口', x: 18.35, z: 15.40 },
    { id: '街面', x: 16.20, z: 13.20 },
  ].map(p => ({ ...p, support: support(p.x, p.z, 0), stuck: blocked(p.x, p.z) }));

  // 店内不可卡死的采样点
  const samples = [
    [18.35, 16.00], [18.35, 19.00], [18.35, 21.50], [18.35, 23.00],
    [15.40, 18.60], [15.40, 20.00], [14.00, 17.20], [14.00, 22.50],
  ].map(([x, z]) => ({ x, z, blocked: blocked(x, z) }));

  /* 店前人行道（z=14.60，檐下）沿 x 走一遍：整条临街面不该有空气墙。
   * z 取 14.60 而不是更靠北的 14.20：居酒屋北立面碰撞盒顶到 z≈14.1，
   * 14.20 距它 0.10 < R=0.15，起手就被判卡死（那是居酒屋，不是书店的墙）。 */
  const frontage = walk(12.00, 14.60, 1, 0, 11);
  const streetFree = [[12.00, 14.60], [13.40, 14.60], [16.20, 14.60], [18.35, 14.60], [19.60, 14.60]]
    .map(([x, z]) => ({ x, z, blocked: blocked(x, z) }));
  // 0.02 的落脚面是谁给的（街面与店内应当同源，否则进店会「上一级台阶」）
  const supportSrc = all.filter(c => c.kind !== 'ramp' && Math.abs(c.y1 - 0.02) < 1e-6).map(c => c.id || c.kind);

  // ================= E. 四面净空（"不穿模"的硬证据）=================
  // 从新体量四个面外 0.1m 处朝外打水平射线，跳过书店自己的命中，报第一堵别人的墙。
  // 体量 x 12.7..19.7 / z 15.0..26.1。扩建方向就是靠这张表定的，改体量必须重跑。
  const bookGroup = R.scene.getObjectByName('street-bookstore');
  const isMine = (o) => { for (let p = o; p; p = p.parent) if (p === bookGroup) return true; return false; };
  /* 只收「有 position 的 Mesh」当靶子：全场景 intersectObjects(children, true) 会踩到
   * 某个没有 geometry 的占位对象，在 Mesh.raycast 里抛 TypeError，而报错栈里看不出是谁。 */
  const casters = [];
  R.scene.traverse(o => {
    if (!o.isMesh || !o.geometry || !o.geometry.attributes.position) return;
    if (isMine(o)) return;
    casters.push(o);
  });
  const rc = new THREE.Raycaster(); rc.far = 40;
  const org = new THREE.Vector3(), dir = new THREE.Vector3();
  const shoot = (x, y, z, dx, dy, dz) => {
    org.set(x, y, z); dir.set(dx, dy, dz).normalize(); rc.set(org, dir);
    const hits = rc.intersectObjects(casters, false);
    if (!hits.length) return null;
    const h = hits[0];
    const chain = []; for (let p = h.object; p && chain.length < 5; p = p.parent) chain.push(p.name || '(anon)');
    return { d: +h.distance.toFixed(3), p: [+h.point.x.toFixed(2), +h.point.y.toFixed(2), +h.point.z.toFixed(2)], who: chain.reverse().join(' > ') };
  };
  const clear = { N: [], E: [], S: [], W: [] };
  for (const y of [0.5, 2.0, 6.0, 12.0]) {
    for (const t of [0.15, 0.5, 0.85]) {
      clear.N.push({ y, at: +(12.7 + 7.0 * t).toFixed(2), hit: shoot(12.7 + 7.0 * t, y, 14.90, 0, 0, -1) });
      clear.S.push({ y, at: +(12.7 + 7.0 * t).toFixed(2), hit: shoot(12.7 + 7.0 * t, y, 26.20, 0, 0, 1) });
      clear.W.push({ y, at: +(15.0 + 11.1 * t).toFixed(2), hit: shoot(12.60, y, 15.0 + 11.1 * t, -1, 0, 0) });
      clear.E.push({ y, at: +(15.0 + 11.1 * t).toFixed(2), hit: shoot(19.80, y, 15.0 + 11.1 * t, 1, 0, 0) });
    }
  }

  // 组内几何量（确认内饰真的建出来了：合批后名字没了，只能看三角形）
  let meshes = 0, tris = 0, lights = 0;
  const g = R.scene.getObjectByName('street-bookstore');
  if (g) g.traverse(o => {
    if (o.isLight) lights++;
    if (!o.isMesh) return;
    meshes++;
    const geo = o.geometry, idx = geo.index;
    tris += (idx ? idx.count : (geo.attributes.position?.count || 0)) / 3;
  });

  return {
    total: all.length, bad: bad.length,
    mine: mine.length, missing, kinds,
    boxes: mine.map(c => ({ id: c.id, x: [+c.x0.toFixed(2), +c.x1.toFixed(2)],
      y: [+c.y0.toFixed(2), +c.y1.toFixed(2)], z: [+c.z0.toFixed(2), +c.z1.toFixed(2)] })),
    door, glass, wallW, wallE, counter, shelf, corridor,
    inner, ctrlStreet, ctrlSolid, floorAt, samples, streetFree, frontage, supportSrc, clear,
    geo: { meshes, tris: Math.round(tris), lights, exists: !!g },
  };
})()
`);

// ---------------- 断言 ----------------
const fails = [];
const ok = [];
const t = (cond, msg) => (cond ? ok : fails).push(msg);

t(out.bad === 0, `盒表无非有限值（${out.total} 个盒）`);
t(out.missing.length === 0, `14 个书店声明式盒全部在表内${out.missing.length ? ' — 缺: ' + out.missing.join(', ') : ''}`);
t(out.kinds.length === 1 && out.kinds[0] === 'wall', `书店盒 kind 全为 wall（实际 ${out.kinds.join('/')}）`);

// B1 门口能进（一直走到柜台前 z≈23.29）
t(out.door.z > 23.0, `门口可直行进入店内深处：走到 z=${out.door.z}（期望 > 23.0，柜台挡停）`);
// B2 橱窗挡人（玻璃挡人面 14.87 − R = 14.72）
t(out.glass.z < 14.85 && out.glass.z > 14.55, `橱窗玻璃挡人：停在 z=${out.glass.z}（期望 ≈14.72）`);
// B3 西内墙
t(out.wallW.x > 12.90 && out.wallW.x < 13.10, `西内墙挡人：停在 x=${out.wallW.x}（期望 ≈12.99）`);
// B4 东内墙
t(out.wallE.x > 19.30 && out.wallE.x < 19.50, `东内墙挡人：停在 x=${out.wallE.x}（期望 ≈19.41）`);
// B5 柜台
t(out.counter.z > 23.20 && out.counter.z < 23.40, `柜台挡人：停在 z=${out.counter.z}（期望 ≈23.29）`);
// B6 书架 C 南端
t(out.shelf.z > 21.00 && out.shelf.z < 21.20, `书架挡人：停在 z=${out.shelf.z}（期望 ≈21.09）`);
// B7 门洞走廊不被密封
const corridorBlocked = out.corridor.filter(([, b]) => b).map(([z]) => z);
t(corridorBlocked.length === 0, `门洞走廊 x=18.35 全程无阻挡${corridorBlocked.length ? ' — 被挡 z=' + corridorBlocked.join(',') : ''}`);

// C 洪水填充
t(out.inner.every(p => p.state === 1), `店内四个走道点从街面可达（${out.inner.map(p => p.id + '=' + p.state).join(' ')}）`);
t(out.ctrlStreet.state === 1, `对照① 街面 (${out.ctrlStreet.x},${out.ctrlStreet.z}) 可达（洪水填充没跑偏）`);
t(out.ctrlSolid.state !== 1, `对照② 后段实心体量内 (${out.ctrlSolid.x},${out.ctrlSolid.z}) 不可达（state=${out.ctrlSolid.state}，西/北两面墙真封住）`);

// D 落脚面：关键不是「等于 0」，而是「店内各点彼此同高」，且与店外差不超过一个
//   人行道台（铺装面 collider 顶 0.048，基底 collider 顶 0.02，差 0.028）。
//   书店北墙落在人行道南缘（z=15.0）之后，门槛正好压在铺装带边上，进店会掉
//   2.8cm —— 这是全场人行道边界都有的既有台阶，不是本轮引入的，所以放宽到 0.03。
const base = out.floorAt.find(p => p.id === '街面');
const innerPts = out.floorAt.filter(p => p.id !== '街面');
const spread = Math.max(...innerPts.map(p => p.support)) - Math.min(...innerPts.map(p => p.support));
t(spread < 1e-6, `店内四个落脚点彼此同高（极差 ${spread.toFixed(4)}，` +
  out.floorAt.map(p => p.id + '=' + p.support).join(' ') + `）`);
const badFloor = out.floorAt.filter(p => p.support == null || Math.abs(p.support - base.support) > 0.03);
t(badFloor.length === 0,
  `店内与街面高差 ≤ 0.03（街面=${base.support}，0.02 来自 ${out.supportSrc.join('/')}）`);
const sunk = out.floorAt.filter(p => p.stuck);
t(sunk.length === 0, `落脚点无卡死${sunk.length ? ' — ' + sunk.map(p => p.id).join(',') : ''}`);

const stuckSamples = out.samples.filter(s => s.blocked);
t(stuckSamples.length === 0, `店内 8 个采样点均可站立${stuckSamples.length ? ' — 卡死: ' + stuckSamples.map(s => '(' + s.x + ',' + s.z + ')').join(' ') : ''}`);

const streetBlocked = out.streetFree.filter(s => s.blocked);
t(streetBlocked.length === 0, `店前檐下人行道无空气墙${streetBlocked.length ? ' — ' + streetBlocked.map(s => '(' + s.x + ',' + s.z + ')').join(' ') : ''}`);
// 走满 11m 说明整条临街面（含书店 7m 面宽）无空气墙；东邻楼的碰撞盒只覆盖
// z 15.68..23.52，而这条扫描线在 z=14.60，所以不会被它挡停。
t(out.frontage.x > 19.8, `临街面 x=12.0 起向东 11m 无空气墙：走到 x=${out.frontage.x}（期望 >19.8）`);

t(out.geo.exists && out.geo.tris > 2500, `店内几何已建出：${out.geo.meshes} mesh / ${out.geo.tris} 三角形 / ${out.geo.lights} 灯`);
t(out.geo.lights >= 2, `店内点光已加：${out.geo.lights} 盏（9m 进深至少要 2 盏）`);

// E 四面净空：书店四个面外不许有别人的几何贴上来（"不穿模"的硬证据）
const MARGIN = { N: 1.0, E: 0.15, S: 0.35, W: 0.85 };
const tight = [];
for (const side of ['N', 'E', 'S', 'W']) {
  for (const r of out.clear[side]) {
    if (!r.hit) continue;                       // 40m 内没东西 = 最安全
    if (r.hit.d < MARGIN[side]) tight.push(`${side}(y=${r.y},@${r.at}) d=${r.hit.d} → ${r.hit.who}`);
  }
}
t(tight.length === 0, `四面净空全部达标（N≥${MARGIN.N} / E≥${MARGIN.E} / S≥${MARGIN.S} / W≥${MARGIN.W}）` +
  (tight.length ? ' — 过近: ' + tight.join(' | ') : ''));

// ---------------- 报告 ----------------
console.log('\n=== 书店（street-bookstore）碰撞与可走性 ===');
console.log(`体量 7.0 × 11.1 × 12.4m（x 12.7..19.7 / z 15.0..26.1 / 4 层）`);
console.log(`盒表总数 ${out.total}，书店声明式盒 ${out.mine} 个：`);
for (const b of out.boxes) {
  console.log(`  ${b.id.padEnd(22)} x ${String(b.x[0]).padStart(6)}..${String(b.x[1]).padEnd(6)} ` +
    `y ${String(b.y[0]).padStart(5)}..${String(b.y[1]).padEnd(5)} z ${String(b.z[0]).padStart(6)}..${b.z[1]}`);
}
console.log('\n直线行走：');
console.log(`  门口   (18.35,13.20) →+z  ${JSON.stringify(out.door)}`);
console.log(`  橱窗   (14.94,13.20) →+z  ${JSON.stringify(out.glass)}`);
console.log(`  西内墙 (15.40,18.60) →-x  ${JSON.stringify(out.wallW)}`);
console.log(`  东内墙 (17.00,18.60) →+x  ${JSON.stringify(out.wallE)}`);
console.log(`  柜台   (18.35,22.00) →+z  ${JSON.stringify(out.counter)}`);
console.log(`  书架   (15.40,22.50) →-z  ${JSON.stringify(out.shelf)}`);
console.log(`  临街面 (12.00,14.60) →+x  ${JSON.stringify(out.frontage)}   ← 檐下人行道整段走通`);
console.log('\n洪水填充（从街面 18.35,13.80 扩散，步长 0.1）：');
for (const p of out.inner) console.log(`  ${p.id} (${p.x},${p.z}) state=${p.state} ${p.state === 1 ? '可达' : '不可达'}`);
console.log(`  对照① 街面 (${out.ctrlStreet.x},${out.ctrlStreet.z}) state=${out.ctrlStreet.state}`);
console.log(`  对照② 后段实心体量内 (${out.ctrlSolid.x},${out.ctrlSolid.z}) state=${out.ctrlSolid.state}`);
console.log('\n落脚面 / 采样：');
for (const p of out.floorAt) console.log(`  ${p.id.padEnd(10)} support=${p.support} stuck=${p.stuck}`);
console.log('\n四面净空（从面外 0.1m 起打，跳过书店自己；— = 40m 内无遮挡）：');
const fmtH = (h) => (h ? `${String(h.d).padStart(6)}m → (${h.p.join(', ')})  ${h.who}` : '    —');
for (const side of ['N', 'E', 'S', 'W']) {
  for (const r of out.clear[side]) console.log(`  ${side} y=${String(r.y).padStart(4)} @${String(r.at).padStart(5)}  ${fmtH(r.hit)}`);
}
console.log('\n--- 断言 ---');
for (const m of ok) console.log(`  ✓ ${m}`);
for (const m of fails) console.log(`  ✗ ${m}`);
console.log(`\n${fails.length ? '✗ 失败 ' + fails.length + ' 项' : '✓ 全部通过（' + ok.length + ' 项）'}\n`);

ws.close();
chrome.kill();
process.exit(fails.length ? 1 : 0);
