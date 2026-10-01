/**
 * 街面积水反射面（store-wet-reflection）的**剔除正确性**验证（只读，不改源码；scripts/ 不入库）
 *
 * 背景：那块 13.2 × 4.8m 的水平镜面是雨夜街景里最贵的一件 —— `Reflector.onBeforeRender`
 * 会把**整个场景**按镜像相机再提交一遍（实测 400+ 次 draw call、69 万三角形，占整帧三成）。
 * 这一轮给它加了两层早退（都在 storeDetails.ts 的 onBeforeRender 里）：
 *
 *   1. **视锥剔除**：世界 AABB × 视锥。用 AABB 而不是 three 的包围球 —— 扁平四边形的
 *      包围球半径 7.02m，相机 6.7m 外平视时球体角半径仍有 46°，"积水其实在视锥外"
 *      这种最常见的情形照样能过球体测试。
 *   2. **楼板遮挡剔除**：站在 203 室中间低头看街面时，积水**在视锥里**却被 3.4m 的楼板
 *      挡得一个像素都露不出来。判据见 collider.ts 的 pickOverheadSlabs + storeDetails 的
 *      reflectionOccluded：取反射面 AABB 的 8 个角，只要有一个角「在画面里 且 没被任何
 *      楼板挡住」就跑反射。
 *
 * 为什么这两条必须单独验一次：它们的收益是"省掉一整趟场景重画"，代价是**画面里凭空
 * 少一块倒影** —— 而"少一块倒影"在雨夜积水这种低对比、又叠在湿地光斑上的东西里，
 * 盯截图是看不出来的。所以这里不看像素"像不像"，只看一条可证的判据：
 *
 *      「剔除开」和「剔除关」两帧，必须**逐像素完全相同**。
 *
 * 为什么这条判据是充分的：剔除只在"积水一个像素都露不出来"的机位触发。真触发了不该
 * 触发的机位，两帧立刻分叉。反过来，如果剔除在该触发的机位没触发，只损失性能、不会
 * 分叉 —— 所以本脚本另有一组**定向断言**：室内那几个机位必须触发、阳台/观察者必须不触发。
 *
 * 顺带回归两件事（都是这一轮改的）：
 *   C. 公告板实例集（props.ts makeBillboardSet）的逐实例属性落在源码公式的取值域内 ——
 *      它是「294 滴屋檐水 = 294 次提交」那次优化的产物，属性写错就是整排滴水变形/消失。
 *   D. 碰撞表：总数不变、且公告板批**没有**混进碰撞表（合批后它横跨整栋楼，混进去就是
 *      一条罩住半条街的隐形墙）。
 *
 * 用法：node plugins/3d-apartment/tools/room/verify-wet-reflection.mjs   （需要 1420 dev server 在跑）
 * 退出码：任一断言失败 → 1
 */
import { spawn } from 'node:child_process';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const PORT = 9381;
const URL = 'http://[::1]:1420/plugins/3d-apartment/preview.html';
const CHROME = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const W = 1280, H = 800;

/* ---------------- 浏览器接线 ---------------- */
const profile = mkdtempSync(join(tmpdir(), 'vivian-wetrefl-'));
const chrome = spawn(CHROME, [
  '--headless=new', '--disable-gpu', '--remote-allow-origins=*',
  `--remote-debugging-port=${PORT}`, `--user-data-dir=${profile}`,
  '--use-gl=angle', '--use-angle=swiftshader', '--enable-unsafe-swiftshader',
  '--no-sandbox', `--window-size=${W},${H}`, 'about:blank',
], { stdio: 'ignore' });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function target() {
  for (let i = 0; i < 120; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
      const p = list.find((t) => t.type === 'page' && t.webSocketDebuggerUrl);
      if (p) return p.webSocketDebuggerUrl;
    } catch { /* 还没起来 */ }
    await sleep(500);
  }
  throw new Error('no chrome target');
}
const ws = new WebSocket(await target());
await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
let id = 0; const pending = new Map(); const logs = [];
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); return; }
  if (m.method === 'Runtime.exceptionThrown') logs.push('exception: ' + JSON.stringify(m.params.exceptionDetails).slice(0, 300));
};
function send(method, params = {}) {
  const mid = ++id;
  return new Promise((res, rej) => {
    pending.set(mid, (m) => (m.error ? rej(new Error(method + ': ' + JSON.stringify(m.error))) : res(m.result)));
    ws.send(JSON.stringify({ id: mid, method, params }));
    setTimeout(() => { if (pending.has(mid)) { pending.delete(mid); rej(new Error('timeout ' + method)); } }, 240000);
  });
}
async function ev(expression) {
  const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error('eval: ' + JSON.stringify(r.exceptionDetails).slice(0, 900));
  return r.result?.value;
}

await send('Page.enable'); await send('Runtime.enable');
await send('Page.navigate', { url: URL });
/** 就绪判据要严：dev 下 StrictMode 会 mount→unmount→remount，`__ROOM__` 会短暂消失。 */
async function waitRoom() {
  for (let i = 0; i < 300; i++) {
    const ok = await ev('!!(window.__ROOM__ && window.__ROOM__.scene && window.__ROOM__.colliders && window.__ROOM__.colliders.length > 0 && typeof window.__ROOM__.setWetOccluders === "function" && document.querySelector("canvas") && document.querySelector("canvas").width > 0)').catch(() => false);
    if (ok) return true;
    await sleep(1000);
  }
  return false;
}
if (!(await waitRoom())) throw new Error('room never became ready（1420 dev server 在跑吗？）');
await sleep(4000);

/* ---------------- 装探针 ---------------- */
const BOOT = `(() => {
  const R = window.__ROOM__;
  /* 冻结应用自己的 rAF：否则应用每帧的 info.reset() 会和手工测量打架，而且
     雨/涟漪/滴水的 uniform 会一直变，"两帧逐像素比对"就没有意义了。 */
  window.requestAnimationFrame = () => 0;
  const info = R.renderer.info;
  info.autoReset = false;
  const gl = R.renderer.getContext();
  const canvas = R.renderer.domElement;
  const CW = canvas.width, CH = canvas.height;
  const bufA = new Uint8Array(CW * CH * 4), bufB = new Uint8Array(CW * CH * 4);

  const key = (() => { let k = null; R.scene.traverse((o) => { if (o.isDirectionalLight && o.castShadow) k = k || o; }); return k; })();
  const reflectors = [], billboards = [];
  R.scene.traverse((o) => {
    if (o.type === 'Reflector') reflectors.push(o);
    if (o.name === '__billboards') billboards.push(o);
  });

  const frame = () => { info.reset(); R.renderer.setRenderTarget(null); R.renderer.render(R.scene, R.camera); return info.render.calls; };

  const hash = (buf) => { let h = 0x811c9dc5; for (let i = 0; i < buf.length; i += 4) { h ^= buf[i] + buf[i + 1] * 3 + buf[i + 2] * 7; h = Math.imul(h, 16777619); } return (h >>> 0).toString(16).padStart(8, '0'); };

  window.__W = {
    R, info, key, reflectors, billboards, canvas: [CW, CH],
    /** 关掉遮挡剔除（传空表）——判据里 overheadSlabs 为空即恒返回"没被挡" */
    cull(on) { R.setWetOccluders(on ? R.colliders : []); return on ? (R.colliders || []).length : 0; },
    setCam(px, py, pz, tx, ty, tz, fov) {
      R.controls.enabled = false;
      R.camera.position.set(px, py, pz);
      R.camera.up.set(0, 1, 0);
      R.camera.lookAt(tx, ty, tz);
      if (fov) { R.camera.fov = fov; R.camera.updateProjectionMatrix(); }
      R.camera.updateMatrixWorld(true);
      /* 断言读回值 == 设定值：OrbitControls 的 damping/minDistance 会把相机拽回去 */
      const p = R.camera.position;
      return Math.abs(p.x - px) < 1e-6 && Math.abs(p.y - py) < 1e-6 && Math.abs(p.z - pz) < 1e-6;
    },
    shadow(on) { key.shadow.needsUpdate = !!on; },
    grab(slot) { frame(); gl.readPixels(0, 0, CW, CH, gl.RGBA, gl.UNSIGNED_BYTE, slot === 'a' ? bufA : bufB); return hash(slot === 'a' ? bufA : bufB); },
    /** A/B 槽的逐像素差（RGB 任一通道差 > 阈值即算"亮着的像素"） */
    diff(threshold) {
      const th = threshold ?? 2;
      let count = 0, maxd = 0;
      for (let i = 0; i < bufA.length; i += 4) {
        const d = Math.max(Math.abs(bufA[i] - bufB[i]), Math.abs(bufA[i + 1] - bufB[i + 1]), Math.abs(bufA[i + 2] - bufB[i + 2]));
        if (d > th) { count++; if (d > maxd) maxd = d; }
      }
      return { count, maxDelta: maxd };
    },
    /** 反射那一趟的净提交量（开关 Reflector 的 visible 取差） */
    reflCost() {
      const was = reflectors.map((o) => o.visible);
      reflectors.forEach((o) => { o.visible = true; });
      const on = frame();
      reflectors.forEach((o) => { o.visible = false; });
      const off = frame();
      reflectors.forEach((o, i) => { o.visible = was[i]; });
      frame();
      return on - off;
    },
  };
  frame(); frame(); frame();
  return { canvas: [CW, CH], reflectors: reflectors.length, billboards: billboards.length };
})()`;

let boot = null;
for (let a = 0; a < 6 && !boot; a++) {
  try { boot = await ev(BOOT); } catch (e) { console.log(`[retry ${a}] ${String(e).slice(0, 140)}`); await waitRoom(); await sleep(3000); }
}
if (!boot) throw new Error('probe boot failed');

/* ---------------- 机位表 ---------------- */
const FOV = 60;
/* 场景布局：203 室 z ≤ 6.3（阳台 4.8~6.3 朝南露天），街道 z 6.3~14，
 * 便利店门面 z=14、店内 z 14~21。积水镜面在 z 7.65~12.45 / y=0.009。
 *
 * 第三列是**期望由哪一层**把这一趟剔掉 —— 注意本脚本只能关掉楼板层
 * （`setWetOccluders([])`），视锥层没有对应的钩子，所以对「视锥层负责」的机位，
 * 剔除开/关两种状态的净成本都接近 0，这本身是对的（楼板层无事可做）。
 *
 *   frustum-cull  积水压根不在画面里（背对街心、或朝里看）→ 视锥层就挡住了
 *   slab-cull     积水在画面里，但被 3.4m 楼板挡死（室内低头看街面）→ 楼板层挡住
 *   keep          积水真的看得见（阳台边缘俯视 / 高处观察者）→ 两层都不能挡
 */
const SHOTS = [
  ['fp-living',      [-0.65, 5.0, 3.7,  -0.65, 5.0, -1.0,  FOV], 'frustum-cull'],
  ['fp-center',      [0.0,   5.0, 0.0,   0.0,   5.0, -6.0,  FOV], 'frustum-cull'],
  ['fp-corner',      [-5.0,  5.0, -5.0,  0.0,   5.0, 3.0,   FOV], 'slab-cull'],
  ['fp-kitchen',     [4.5,   5.0, -3.0, -2.0,   5.0, 0.0,   FOV], 'slab-cull'],
  ['fp-window-lvl',  [0.0,   5.0, 5.6,   0.0,   5.0, 30.0,  FOV], 'frustum-cull'],
  ['fp-window-down', [0.0,   5.0, 6.1,   0.0,   0.6, 12.0,  FOV], 'keep'],
  ['fp-sidewalk',    [0.775, 1.6, 12.9,  0.775, 1.6, 20.0,  FOV], 'frustum-cull'],
  ['fp-street-back', [0.775, 1.6, 12.9,  0.0,   4.0, 5.0,   FOV], 'frustum-cull'],
  ['fp-store-in',    [0.775, 1.6, 16.5,  0.775, 1.6, 21.0,  FOV], 'frustum-cull'],
  ['fp-observer',    [13,    19,  30,    0,     1.5, 6,     40],  'keep'],
];

const rows = [];
for (const [label, cam, expect] of SHOTS) {
  const r = await ev(`(() => {
    const W = window.__W;
    const ok = W.setCam(${cam.join(',')});
    W.shadow(false);
    const out = { label: ${JSON.stringify(label)}, expect: ${JSON.stringify(expect)}, camOk: ok };

    /* --- 剔除开 --- */
    W.cull(true); W.grab('a'); W.grab('a'); W.grab('a');
    const hOn = W.grab('a');
    out.reflCostOn = W.reflCost();

    /* --- 剔除关（空表 = 关掉遮挡层，视锥层还在） --- */
    W.cull(false); W.grab('b'); W.grab('b'); W.grab('b');
    const hOff = W.grab('b');
    out.reflCostOff = W.reflCost();

    out.hashOn = hOn; out.hashOff = hOff;
    out.pixelDiff = W.diff(2);

    W.cull(true); W.grab('a');
    return out;
  })()`);
  rows.push(r);
  process.stdout.write(`  ${label} … 剔除开 ${r.reflCostOn} / 剔除关 ${r.reflCostOff} 次提交，两帧差 ${r.pixelDiff.count} px\n`);
}

/* ---------------- C. 公告板实例集属性 + D. 碰撞表 ---------------- */
const audit = await ev(`(() => {
  const R = window.__ROOM__;
  const bb = [];
  R.scene.traverse((o) => { if (o.name === '__billboards') bb.push(o); });
  const sets = bb.map((m) => {
    const g = m.geometry, p = g.attributes.iPos.array, s = g.attributes.iSize.array, a = g.attributes.iAlpha.array;
    const rng = (arr, stride, off) => { let lo = Infinity, hi = -Infinity; for (let i = off; i < arr.length; i += stride) { if (arr[i] < lo) lo = arr[i]; if (arr[i] > hi) hi = arr[i]; } return [+lo.toFixed(4), +hi.toFixed(4)]; };
    return {
      parent: m.parent?.name, count: g.instanceCount,
      alpha: rng(a, 1, 0), sizeW: rng(s, 2, 0), sizeH: rng(s, 2, 1),
      collideSkip: m.userData.sceneCollideSkip === true,
      renderOrder: m.renderOrder,
      transparent: m.material.transparent, depthWrite: m.material.depthWrite,
      sphereR: g.boundingSphere ? +g.boundingSphere.radius.toFixed(2) : null,
      /* 位置范围用来确认实例真的摆开了，不是全堆在原点 */
      posX: rng(p, 3, 0), posY: rng(p, 3, 1), posZ: rng(p, 3, 2),
    };
  });
  let sprites = 0;
  R.scene.traverse((o) => { if (o.isSprite) sprites++; });
  const cols = R.colliders || [];
  const bbInColliders = cols.filter((c) => c.source === '__billboards' || /billboard/i.test(c.source || '')).length;
  return { sets, sprites, colliders: cols.length, bbInColliders };
})()`);

/* ---------------- 断言 ---------------- */
const fails = [];
const ok = (cond, msg) => { if (!cond) fails.push(msg); return cond; };
const fmt = (v) => JSON.stringify(v);

console.log('\n=== A. 剔除开 / 剔除关 两帧逐像素比对 ===');
console.log('label'.padEnd(16), '期望'.padEnd(13), '剔除开'.padEnd(8), '剔除关'.padEnd(8), '楼板层生效'.padEnd(11), '两帧差'.padEnd(10), '判定');
for (const r of rows) {
  /* 楼板层是否真的在干活：关掉它之后这一趟就冒出来了（>50 次提交） */
  const slabFired = r.reflCostOn <= 1 && r.reflCostOff > 50;
  const diffOk = r.pixelDiff.count === 0;
  let verdict = 'OK';
  if (!r.camOk) verdict = '!! 相机没到位';
  else if (!diffOk) verdict = '!! 画面分叉（剔错了）';
  else if (r.expect === 'slab-cull' && !slabFired) verdict = '!! 楼板层该剔没剔（只损失性能）';
  else if (r.expect === 'frustum-cull' && r.reflCostOn > 1) verdict = '!! 视锥层该剔没剔（只损失性能）';
  else if (r.expect === 'keep' && r.reflCostOn <= 1) verdict = '!! 不该剔却剔了（画面少一块倒影）';
  console.log(
    r.label.padEnd(16),
    r.expect.padEnd(13),
    String(r.reflCostOn).padEnd(8),
    String(r.reflCostOff).padEnd(8),
    String(slabFired).padEnd(11),
    String(r.pixelDiff.count + 'px').padEnd(10),
    verdict
  );
  ok(r.camOk, `${r.label}: 相机位置读回与设定不符（OrbitControls 把相机拽走了）`);
  ok(diffOk, `${r.label}: 剔除开/关两帧分叉 ${r.pixelDiff.count}px（maxDelta ${r.pixelDiff.maxDelta}）—— 剔除条件不保守`);
  ok(r.expect !== 'slab-cull' || slabFired, `${r.label}: 期望楼板遮挡层触发，实际没触发（reflCostOn ${r.reflCostOn} / reflCostOff ${r.reflCostOff}）`);
  ok(r.expect !== 'frustum-cull' || r.reflCostOn <= 1, `${r.label}: 期望视锥层挡住，实际这一趟还在跑（reflCostOn ${r.reflCostOn}）`);
  ok(r.expect !== 'keep' || r.reflCostOn > 1, `${r.label}: 不该剔却剔了（reflCostOn ${r.reflCostOn}）`);
}

console.log('\n=== B. 遮挡体清单 ===');
const slabInfo = await ev(`(() => {
  const R = window.__ROOM__;
  /* 只读地复刻 pickOverheadSlabs 的筛选，用来报"到底挑出了哪些板" */
  const refl = (() => { let r = null; R.scene.traverse((o) => { if (o.type === 'Reflector') r = o; }); return r; })();
  const aboveY = refl.position.y;
  const out = [];
  for (const c of R.colliders || []) {
    const dy = c.max.y - c.min.y, dx = c.max.x - c.min.x, dz = c.max.z - c.min.z;
    if (dy < 0.35 && dx > 4 && dz > 4 && c.min.y > aboveY + 0.05) {
      out.push({ name: c.source || '(anon)', y: [+c.min.y.toFixed(2), +c.max.y.toFixed(2)], x: [+c.min.x.toFixed(1), +c.max.x.toFixed(1)], z: [+c.min.z.toFixed(1), +c.max.z.toFixed(1)] });
    }
  }
  return { aboveY: +aboveY.toFixed(4), slabs: out };
})()`);
console.log(`镜面高度 y=${slabInfo.aboveY}，挑出 ${slabInfo.slabs.length} 块头顶水平板：`);
for (const s of slabInfo.slabs) console.log(`  ${s.name.padEnd(24)} y${fmt(s.y)}  x${fmt(s.x)}  z${fmt(s.z)}`);
ok(slabInfo.slabs.length > 0, 'B: 一块头顶水平板都没挑出来 —— 遮挡层等于没开（镜面高度或碰撞表变了？）');

console.log('\n=== C. 公告板实例集（props.ts makeBillboardSet）===');
for (const s of audit.sets) {
  console.log(`  ${String(s.parent).padEnd(16)} n=${String(s.count).padStart(4)}  alpha${fmt(s.alpha)}  sizeW${fmt(s.sizeW)}  sizeH${fmt(s.sizeH)}  order=${s.renderOrder}  sphereR=${s.sphereR}`);
  console.log(`  ${''.padEnd(16)}   posX${fmt(s.posX)} posY${fmt(s.posY)} posZ${fmt(s.posZ)}  transparent=${s.transparent} depthWrite=${s.depthWrite} collideSkip=${s.collideSkip}`);
  ok(s.collideSkip, `C: ${s.parent} 的公告板批没标 sceneCollideSkip —— 合批后它横跨整栋楼，会变成隐形墙`);
  ok(s.transparent && s.depthWrite === false, `C: ${s.parent} 的混合状态与 Sprite 版不一致（transparent=${s.transparent} depthWrite=${s.depthWrite}）`);
  ok(s.posX[0] !== s.posX[1] || s.posZ[0] !== s.posZ[1], `C: ${s.parent} 的实例全堆在同一个位置`);
}
const drips = audit.sets.find((s) => s.parent === 'eave-drips');
if (drips) {
  /* 源码公式：alpha = 0.34 + 0.3k (k∈[0,1])；sizeW = 0.05 + rnd*0.04；sizeH = baseH*(0.8+0.5k)，baseH = 0.18 + rnd*0.16 */
  ok(drips.alpha[0] >= 0.339 && drips.alpha[1] <= 0.641, `C: eave-drips 的 alpha ${fmt(drips.alpha)} 超出公式域 [0.34, 0.64]`);
  ok(drips.sizeW[0] >= 0.049 && drips.sizeW[1] <= 0.091, `C: eave-drips 的 sizeW ${fmt(drips.sizeW)} 超出公式域 [0.05, 0.09]`);
  ok(drips.sizeH[0] >= 0.143 && drips.sizeH[1] <= 0.443, `C: eave-drips 的 sizeH ${fmt(drips.sizeH)} 超出公式域 [0.144, 0.442]`);
}
const ripples = audit.sets.find((s) => s.parent === 'puddle-ripples');
if (ripples) {
  /* 源码公式：alpha = sin(pπ)*0.5 ∈ [0, 0.5]；size = base*(0.5+p*0.5)，base = 0.42 + rnd*0.55 */
  ok(ripples.alpha[0] >= 0 && ripples.alpha[1] <= 0.501, `C: puddle-ripples 的 alpha ${fmt(ripples.alpha)} 超出公式域 [0, 0.5]`);
  ok(ripples.sizeW[1] <= 0.971, `C: puddle-ripples 的 sizeW ${fmt(ripples.sizeW)} 超出公式域 [0.21, 0.97]`);
}

console.log('\n=== D. 碰撞表 ===');
console.log(`  总数 ${audit.colliders}（基线 936）  公告板批混入 ${audit.bbInColliders} 条  场景内 Sprite 残留 ${audit.sprites}（基线 8，店面滴水）`);
ok(audit.colliders === 936, `D: 碰撞盒总数从 936 变成 ${audit.colliders} —— 装配路径被动过了`);
ok(audit.bbInColliders === 0, `D: 公告板批混进了碰撞表 ${audit.bbInColliders} 条`);
ok(audit.sprites <= 8, `D: Sprite 数 ${audit.sprites} > 8 —— 有新的逐对象 Sprite 冒出来了（每多一个就是一次提交）`);

/* ---------------- 汇总 ---------------- */
const report = { boot, slabInfo, rows, audit, fails, logs: logs.slice(0, 10) };
writeFileSync('G:/vivian-rs/tmp/_verify-wet-reflection.json', JSON.stringify(report, null, 1));

const savedCalls = rows.reduce((n, r) => n + Math.max(0, r.reflCostOff - r.reflCostOn), 0);
console.log('\n=== 汇总 ===');
console.log(`  触发剔除的机位省下的净提交量合计：${savedCalls} 次（这些帧里积水一个像素都露不出来）`);
console.log(`  两帧分叉的机位：${rows.filter((r) => r.pixelDiff.count > 0).length} 个（必须为 0）`);
if (fails.length) {
  console.log(`\n✗ ${fails.length} 条断言失败：`);
  for (const f of fails) console.log('  - ' + f);
  chrome.kill();
  process.exit(1);
}
console.log('\n✓ 全部断言通过：剔除开/关逐像素一致，遮挡层在应触发处触发、在不该触发处不触发。');
chrome.kill();
process.exit(0);
