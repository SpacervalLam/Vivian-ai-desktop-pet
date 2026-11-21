import * as THREE from 'three';

/* 远景：环形东京楼群 + 晴空塔 + 东京塔 + 高架绿皮列车。
 * 做成环形而不是一面背景板，是因为三个机位朝向不同（图 1 斜看巷口、
 * 图 2 沿巷回望、图 3 从阳台外眺），任何方向都得能读到天际线。 */

function cityFacadeTexture() {
  const c = document.createElement('canvas'); c.width = 64; c.height = 128;
  const x = c.getContext('2d');
  x.fillStyle = '#4a5566'; x.fillRect(0, 0, 64, 128);
  let s = 99173;
  const r = () => ((s = (Math.imul(s, 1664525) + 1013904223) >>> 0) / 4294967296);
  for (let row = 0; row < 22; row++) {
    for (let col = 0; col < 9; col++) {
      const on = r() > .46;
      x.fillStyle = on ? (r() > .78 ? '#ffe0a4' : '#c9d8e8') : '#313b49';
      x.globalAlpha = on ? .55 + r() * .45 : 1;
      x.fillRect(3 + col * 6.6, 3 + row * 5.7, 4.4, 3.7);
    }
  }
  x.globalAlpha = 1;
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  return t;
}

function rnd(seed) {
  let s = seed >>> 0;
  return () => ((s = (Math.imul(s, 1664525) + 1013904223) >>> 0) / 4294967296);
}

function skytree(scene, M, x, z) {
  const g = new THREE.Group(); g.name = 'tokyo-skytree'; g.position.set(x, 0, z);
  const white = M.tower;
  // 三脚底座
  for (let i = 0; i < 3; i++) {
    const a = i * Math.PI * 2 / 3;
    const leg = new THREE.Mesh(new THREE.CylinderGeometry(1.6, 4.2, 34, 8), white);
    leg.position.set(Math.cos(a) * 9, 17, Math.sin(a) * 9);
    leg.rotation.z = -Math.cos(a) * .22;
    leg.rotation.x = Math.sin(a) * .22;
    g.add(leg);
  }
  const body = new THREE.Mesh(new THREE.CylinderGeometry(2.6, 8.5, 118, 14), white);
  body.position.y = 76; g.add(body);
  const core = new THREE.Mesh(new THREE.CylinderGeometry(1.3, 1.3, 118, 8), M.towerLit);
  core.position.y = 76; g.add(core);
  for (const [y, rad, h, lit] of [[68, 10.5, 4.5, true], [101, 7.2, 3.6, true]]) {
    const deck = new THREE.Mesh(new THREE.CylinderGeometry(rad, rad * .82, h, 16), lit ? M.towerLit : white);
    deck.position.y = y; g.add(deck);
    const ring = new THREE.Mesh(new THREE.TorusGeometry(rad, .5, 6, 20), white);
    ring.rotation.x = Math.PI / 2; ring.position.y = y + h / 2; g.add(ring);
  }
  const mast = new THREE.Mesh(new THREE.CylinderGeometry(.35, 1.1, 44, 8), white);
  mast.position.y = 141; g.add(mast);
  const tip = new THREE.Mesh(new THREE.CylinderGeometry(.1, .35, 14, 6), M.towerLit);
  tip.position.y = 169; g.add(tip);
  scene.add(g);
  return g;
}

function tokyoTower(scene, M, x, z) {
  const g = new THREE.Group(); g.name = 'tokyo-tower'; g.position.set(x, 0, z);
  const red = M.cherryRed, white = M.tower;
  const H = 96, baseW = 15, topW = 2.2;
  // 四腿桁架，红白交替
  for (let i = 0; i < 4; i++) {
    const a = Math.PI / 4 + i * Math.PI / 2;
    for (let seg = 0; seg < 6; seg++) {
      const y0 = seg * H / 6, y1 = (seg + 1) * H / 6;
      const w0 = baseW + (topW - baseW) * (y0 / H);
      const w1 = baseW + (topW - baseW) * (y1 / H);
      const p0 = new THREE.Vector3(Math.cos(a) * w0, y0, Math.sin(a) * w0);
      const p1 = new THREE.Vector3(Math.cos(a) * w1, y1, Math.sin(a) * w1);
      const d = new THREE.Vector3().subVectors(p1, p0);
      const m = new THREE.Mesh(new THREE.CylinderGeometry(.55, .8, d.length(), 5), seg % 2 ? white : red);
      m.position.copy(p0).add(p1).multiplyScalar(.5);
      m.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), d.clone().normalize());
      g.add(m);
    }
  }
  for (const [y, w] of [[26, 11.5], [48, 7.5], [66, 4.8]]) {
    const deck = new THREE.Mesh(new THREE.CylinderGeometry(w, w, 2.4, 4), y === 48 ? M.towerLit : red);
    deck.rotation.y = Math.PI / 4; deck.position.y = y; g.add(deck);
    for (let i = 0; i < 4; i++) {
      const a = Math.PI / 4 + i * Math.PI / 2;
      const bar = new THREE.Mesh(new THREE.BoxGeometry(w * 2, .5, .5), red);
      bar.position.y = y; bar.rotation.y = a; g.add(bar);
    }
  }
  const mast = new THREE.Mesh(new THREE.CylinderGeometry(.25, .9, 34, 6), red);
  mast.position.y = H + 15; g.add(mast);
  scene.add(g);
  return g;
}

function elevatedRail(scene, M, z = -138) {
  const g = new THREE.Group(); g.name = 'elevated-rail'; g.position.z = z;
  const deck = new THREE.Mesh(new THREE.BoxGeometry(240, 1.1, 6.6), M.concrete);
  deck.position.y = 10.6; g.add(deck);
  for (const dx of [-3.1, 3.1]) {
    const side = new THREE.Mesh(new THREE.BoxGeometry(240, .7, .3), M.metal);
    side.position.set(0, 11.4, dx); g.add(side);
  }
  for (let x = -110; x <= 110; x += 24) {
    const pier = new THREE.Mesh(new THREE.BoxGeometry(2.2, 10.2, 2.2), M.concrete);
    pier.position.set(x, 5.1, 0); g.add(pier);
  }
  // 绿色列车（图 1 高架上的那一列）
  for (let i = 0; i < 5; i++) {
    const car = new THREE.Mesh(new THREE.BoxGeometry(11.6, 2.5, 2.9), M.train);
    car.position.set(i * 12, 12.9, 0); g.add(car);
    const band = new THREE.Mesh(new THREE.BoxGeometry(11.2, .22, 3.0), M.flowerWhite);
    band.position.set(i * 12, 13.15, 0); g.add(band);
    for (let w = 0; w < 7; w++) {
      const win = new THREE.Mesh(new THREE.BoxGeometry(1.15, .85, .04), M.windowDark);
      win.position.set(i * 12 - 4.6 + w * 1.5, 13.1, 1.48); g.add(win);
    }
  }
  scene.add(g);
  return g;
}

export function buildSkyline(scene, M, atmo) {
  const facade = cityFacadeTexture();
  const cityMat = new THREE.MeshStandardMaterial({
    color: '#8593a6', map: facade, emissive: '#ffce8c', emissiveMap: facade,
    emissiveIntensity: .5, roughness: .72, metalness: .12,
  });
  cityMat.name = 'city-facade';
  atmo.register(cityMat, { day: .12, golden: .45, dusk: 1.5 });

  const r = rnd(20261105);
  const box = new THREE.BoxGeometry(1, 1, 1);
  const inst = new THREE.InstancedMesh(box, cityMat, 156);
  inst.name = 'tokyo-skyline';
  const d = new THREE.Object3D();
  const col = new THREE.Color();
  let k = 0;
  // 三圈：近圈矮、远圈高，靠雾自然拉开空气透视
  // 推到 300m 以外：第一版 205m 起，楼群近得像一圈围墙压在街屋后面。
  // 靠 FogExp2 自然拉开空气透视，概念图里天际线是远处淡淡的一层。
  for (const [ring, count, hMin, hMax, wMin, wMax] of [
    [300, 48, 18, 46, 10, 22],
    [440, 54, 34, 82, 13, 28],
    [610, 44, 52, 126, 15, 32],
  ]) {
    for (let i = 0; i < count; i++) {
      const a = (i / count) * Math.PI * 2 + (r() - .5) * .1;
      const rad = ring + (r() - .5) * 70;
      const h = hMin + r() * (hMax - hMin);
      const w = wMin + r() * (wMax - wMin);
      d.position.set(Math.cos(a) * rad, h / 2, Math.sin(a) * rad);
      d.scale.set(w, h, w * (.7 + r() * .5));
      d.rotation.y = (r() - .5) * .35;
      d.updateMatrix(); inst.setMatrixAt(k, d.matrix);
      const shade = .72 + r() * .28;
      col.setRGB(shade * .86, shade * .9, shade);
      inst.setColorAt(k, col);
      k++;
    }
  }
  inst.count = k;
  inst.instanceMatrix.needsUpdate = true;
  scene.add(inst);

  /* 三个机位朝向相差近 180°，一座地标照顾不到全部。东京本来就是多方向
   * 都能看到不同地标：两座白色塔分列西南/正南，红色东京塔在正北，
   * 再靠环形楼群兜底，保证任何朝向都有连续天际线。 */
  skytree(scene, M, -300, 170);      // 图 3 阳台朝 -X/+Z 时的晴空塔
  skytree(scene, M, 60, -400);       // 图 2 沿巷朝 -Z 望去的晴空塔
  tokyoTower(scene, M, -40, -380);   // 图 1 巷口朝 -Z 时的红色东京塔
  elevatedRail(scene, M, -138);
  return { inst };
}
