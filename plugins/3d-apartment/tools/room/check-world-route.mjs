/**
 * 跨场景可达性哨兵（浏览器版）——改公寓公共区 / 街区 / 店铺几何后必跑。
 *
 * 为什么单独一个脚本：跨层栅格是**运行时从碰撞表派生的**（navWorld），node 里没有
 * 场景装配就建不出来，所以「角色能不能走到外廊/大堂/书店」这件事只能在浏览器里验。
 * 室内那一层的校验在 check:agent-track（node 版）里。
 *
 * 跑法（需要预览环境已就绪）：
 *   1) npm run dev              # vite 起在 1420
 *   2) bash tmp/_cdp-start.sh   # 常驻无头 Chrome + CDP 9222
 *   3) node plugins/3d-apartment/tools/room/check-world-route.mjs
 */
const PORT = Number(process.env.CDP_PORT || 9222);
const sleep = (ms) => new Promise(r => setTimeout(r, ms));

const list = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
const tab = list.find(t => t.type === 'page' && /3d-apartment\/preview/.test(t.url));
if (!tab) {
  console.error('没找到房间预览页签：先跑 npm run dev 与 bash tmp/_cdp-start.sh');
  process.exit(2);
}

const ws = new WebSocket(tab.webSocketDebuggerUrl);
let msgId = 0;
const pending = new Map();
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
};
await new Promise((res, rej) => { ws.onopen = res; ws.onerror = () => rej(new Error('ws 连接失败')); });
const send = (method, params = {}) => {
  const id = ++msgId;
  ws.send(JSON.stringify({ id, method, params }));
  return new Promise((res, rej) => pending.set(id, (m) => (m.error ? rej(new Error(JSON.stringify(m.error))) : res(m.result))));
};
const evaluate = async (expr) => {
  const res = await send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true });
  if (res.exceptionDetails) throw new Error(res.exceptionDetails.exception?.description ?? 'eval failed');
  return res.result.value;
};

const PRECHECK = `!!(window.__ROOM__ && window.__ROOM__.navWorld && window.__ROOM__.agents?.length >= 2)`;
let ready = false;
for (let i = 0; i < 60; i++) {
  ready = await evaluate(PRECHECK).catch(() => false);
  if (ready) break;
  await sleep(1000);
}
if (!ready) { console.error('预览页没就绪（缺 __ROOM__.navWorld 或 agents）'); process.exit(2); }

/** 在页面里跑：层统计 + 门户自检 + 关键路线（离线快进状态机，不依赖真实时间）。 */
const report = await evaluate(`(() => {
  const R = window.__ROOM__, W = R.navWorld;
  const free = (l) => { let n = 0; for (let i = 0; i < l.grid.res; i++) for (let j = 0; j < l.grid.resZ; j++) if (!l.grid.blocked[i][j]) n++; return n; };
  const layers = W.layers.map(l => ({ id: l.id, y: l.y, cell: l.grid.cell, free: free(l), obs: l.obstacles.length }));
  const deadPortals = W.portals.filter(p => p.walkable === false).map(p => p.kind + ':' + p.id);
  const V = R.agents[0];
  const go = (spot, layer, target, ticks) => {
    V.setLayer(layer); V.pos.x = spot[0]; V.pos.z = spot[1]; V.state = 'idle'; V.idleTimer = 0;
    V.goTo(target);
    const hops = new Set(); let liftTicks = 0, last = V.layerId, arrived = 0;
    for (let i = 0; i < ticks; i++) {
      V.tick(0.05);
      if (V.state === 'lift') liftTicks++;
      if (V.layerId !== last) { hops.add(last + '→' + V.layerId); last = V.layerId; }
      if (V.state === 'stay') { arrived = +(i * 0.05).toFixed(0); break; }
    }
    return { target, hit: V.stateLabel === 'stay@' + target, hops: [...hops], liftSec: +(liftTicks * 0.05).toFixed(1), sec: arrived,
             pos: [+V.pos.x.toFixed(1), +V.pos.y.toFixed(2), +V.pos.z.toFixed(1)] };
  };
  const routes = [
    go([-0.95, 2.0], 'apt2', 'corridor-view', 4000),
    go([-0.95, 2.0], 'apt2', 'lobby-south', 8000),
    go([-0.95, 2.0], 'apt2', 'book-front', 8000),
    go([-0.95, 2.0], 'apt2', 'street-door', 8000),
    go([-34.6, -2.2], 'apt2out', 'lift-lobby-1f', 3000),
  ];
  return { layers, deadPortals, routes };
})()`);

let bad = 0;
const fail = (m) => { console.log(`FAIL  ${m}`); bad++; };

console.log('\n— 导航层 —');
for (const l of report.layers) {
  console.log(`  ${l.id.padEnd(8)} y=${l.y.toFixed(2)} 格宽=${l.cell.toFixed(2)} 可走格=${l.free} 障碍=${l.obs}`);
  if (l.free <= 0) fail(`${l.id} 层没有可走格（地板没接上，或障碍把整层封死）`);
}

console.log('\n— 门户 —');
/**
 * 已知的死门户：登记在这里的只提示不判失败，**新出现的死边照样 FAIL**。
 * 让哨兵常年变红只会让人对它麻木，但把这些边静默跳过又等于没验——
 * 所以走"显式登记 + 写清原因"这条路。
 */
const KNOWN_DEAD_PORTALS = {
  'door:cvs-door': '便利店货架正对门口，店内只剩 2m² 可走；要改便利店几何（挪货架给门口留通道）',
};
const newDead = report.deadPortals.filter(p => !(p in KNOWN_DEAD_PORTALS));
for (const p of report.deadPortals) {
  if (newDead.includes(p)) continue;
  console.log(`  skip ${p} —— 已知：${KNOWN_DEAD_PORTALS[p]}`);
}
if (newDead.length) fail(`新出现死门户（落脚点不在本层可走区）：${newDead.join(', ')}`);
else if (!report.deadPortals.length) console.log('  ok   所有门户两侧落脚点都可走');

console.log('\n— 路线（离线快进状态机）—');
for (const r of report.routes) {
  const line = `${r.target.padEnd(15)} ${r.hit ? 'ok  ' : 'FAIL'} 换层 ${r.hops.join(' → ') || '（同层）'}  电梯 ${r.liftSec}s  总耗时 ${r.sec}s  终 ${r.pos.join(',')}`;
  if (r.hit) console.log('  ' + line);
  else fail(`${r.target} 走不到：${line}`);
}
// 电梯那条必须真的坐梯：走楼梯也能到 1F 电梯厅，但那条路要多绕 170m
const liftRoute = report.routes.find(r => r.target === 'lift-lobby-1f');
if (liftRoute && liftRoute.hit && liftRoute.liftSec <= 0) {
  fail('2F→1F 电梯厅没走电梯（门户代价算错了，会绕一整条街）');
}

console.log(bad === 0 ? '\nALL PASS' : `\n${bad} FAILED`);
ws.close();
await sleep(50);
process.exit(bad ? 1 : 0);
