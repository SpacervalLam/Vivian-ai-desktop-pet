import * as THREE from 'three';
import { Batch } from './batch.js';
import { ALLEY } from './buildings.js';
import { glowTexture, petalTexture, groundTexture } from './palette.js';

/* 街面与「自然生活细节」。图 1/2 的辨识度其实很依赖这些小东西：
 * 电线杆与架空线、街灯、自行车、自动贩卖机、路缘白线、湿反光、落樱。 */

function pole(scene, M, x, z, height = 8.2, seed = 0) {
  const g = new THREE.Group(); g.name = 'utility-pole'; g.position.set(x, 0, z);
  const shaft = new THREE.Mesh(new THREE.CylinderGeometry(.13, .19, height, 8), M.concrete);
  shaft.position.y = height / 2; shaft.castShadow = true; g.add(shaft);
  // 横担与绝缘子
  for (const [dy, len] of [[height - .55, 1.7], [height - 1.35, 1.3]]) {
    const arm = new THREE.Mesh(new THREE.BoxGeometry(.1, .09, len), M.woodDark);
    arm.position.y = dy; arm.castShadow = true; g.add(arm);
    for (const dz of [-len / 2 + .14, len / 2 - .14]) {
      const ins = new THREE.Mesh(new THREE.CylinderGeometry(.045, .045, .13, 6), M.metal);
      ins.position.set(0, dy + .11, dz); g.add(ins);
    }
  }
  // 变压器（图 1 电杆上那个方盒）
  if (seed % 3 === 0) {
    const t = new THREE.Mesh(new THREE.BoxGeometry(.55, .72, .48), M.metal);
    t.position.set(.32, height - 2.25, 0); t.castShadow = true; g.add(t);
  }
  // 街灯弯臂 + 灯罩
  const arm = new THREE.Mesh(new THREE.BoxGeometry(1.05, .06, .06), M.blackMetal);
  arm.position.set(-.5, height - 1.85, 0); g.add(arm);
  const shade = new THREE.Mesh(new THREE.CylinderGeometry(.24, .3, .16, 10), M.blackMetal);
  shade.position.set(-1.0, height - 1.95, 0); g.add(shade);
  const bulb = new THREE.Mesh(new THREE.CylinderGeometry(.2, .2, .04, 10), M.windowLit);
  bulb.position.set(-1.0, height - 2.05, 0); g.add(bulb);
  scene.add(g);
  return { group: g, lampX: x - 1.0, lampY: height - 2.05, lampZ: z };
}

function wire(scene, M, a, b, sag = .9) {
  const mid = a.clone().add(b).multiplyScalar(.5); mid.y -= sag;
  const curve = new THREE.CatmullRomCurve3([a, mid, b]);
  const tube = new THREE.Mesh(new THREE.TubeGeometry(curve, 14, .008, 4, false), M.blackMetal);
  scene.add(tube);
}

function bicycle(scene, M, x, z, rotY = 0, seed = 0) {
  const g = new THREE.Group(); g.position.set(x, 0, z); g.rotation.y = rotY;
  const rim = new THREE.TorusGeometry(.34, .026, 6, 18);
  const tyre = M.blackMetal;
  for (const dx of [-.52, .52]) {
    const w = new THREE.Mesh(rim, tyre); w.rotation.y = Math.PI / 2; w.position.set(dx, .36, 0); g.add(w);
    for (let k = 0; k < 6; k++) {
      const spoke = new THREE.Mesh(new THREE.CylinderGeometry(.008, .008, .66, 4), M.metal);
      spoke.rotation.x = k * Math.PI / 6; spoke.position.set(dx, .36, 0); g.add(spoke);
    }
  }
  const frame = [
    [.0, .72, 0, .62, .06, .06, .5], [-.26, .55, 0, .6, .06, .06, -.5],
    [.26, .55, 0, .55, .06, .06, .5],
  ];
  for (const [px, py, pz, w, h, d, rz] of frame) {
    const m = new THREE.Mesh(new THREE.BoxGeometry(w, h, d), seed % 2 ? M.metal : M.woodDark);
    m.position.set(px, py, pz); m.rotation.z = rz; g.add(m);
  }
  const seat = new THREE.Mesh(new THREE.BoxGeometry(.34, .06, .16), M.blackMetal);
  seat.position.set(-.2, .95, 0); g.add(seat);
  const bar = new THREE.Mesh(new THREE.BoxGeometry(.06, .06, .62), M.metal);
  bar.position.set(.52, 1.0, 0); g.add(bar);
  const basket = new THREE.Mesh(new THREE.BoxGeometry(.34, .24, .42), M.wood);
  basket.position.set(.62, .85, 0); g.add(basket);
  g.traverse(o => { if (o.isMesh) o.castShadow = true; });
  scene.add(g);
}

function vending(scene, M, x, z, rotY = 0) {
  const g = new THREE.Group(); g.position.set(x, 0, z); g.rotation.y = rotY;
  const body = new THREE.Mesh(new THREE.BoxGeometry(.95, 1.85, .78), M.plasterWarm);
  body.position.y = .93; body.castShadow = true; g.add(body);
  const panel = new THREE.Mesh(new THREE.PlaneGeometry(.78, 1.42), M.windowLit);
  panel.position.set(0, 1.06, .395); g.add(panel);
  for (let r = 0; r < 4; r++) {
    const shelf = new THREE.Mesh(new THREE.BoxGeometry(.74, .03, .02), M.metal);
    shelf.position.set(0, .55 + r * .32, .402); g.add(shelf);
  }
  const base = new THREE.Mesh(new THREE.BoxGeometry(1.0, .1, .82), M.blackMetal);
  base.position.y = .05; g.add(base);
  scene.add(g);
}

export function buildStreet(scene, M, atmo) {
  const b = new Batch(M, 'sakura-street');
  const { half, near, far } = ALLEY;
  const length = near - far;

  // 街区基底：巷子以外必须有一块真实地面，否则会直接看到天空球的下半球，
  // 变成一整片浅米色「假地板」（第二版截图的右下角就是这个问题）。
  const ground = new THREE.Mesh(
    new THREE.PlaneGeometry(560, 560),
    new THREE.MeshStandardMaterial({ color: '#ffffff', map: groundTexture(), roughness: 1 })
  );
  ground.rotation.x = -Math.PI / 2;
  ground.position.set(0, -.14, -40);
  ground.receiveShadow = true;
  scene.add(ground);

  // 横街 + 入口前广场：图 1 的路口视角站在这里，左侧街墙在 z∈[1,25] 断开。
  b.box('asphalt', 0, -.04, 19.5, 38, .08, 10.5);
  for (const zz of [14.3, 24.7]) b.box('flowerWhite', 0, .012, zz, 34, .012, .07);
  b.box('stone', 0, .012, 7.8, 30, .05, 13.6);
  for (let k = 0; k < 5; k++) b.box('concrete', -12 + k * 6, .042, 7.8, .04, .008, 13.6);

  // 车行道 / 人行道 / 路缘 / 白线
  b.box('asphalt', 0, -.045, (near + far) / 2, half * 2, .09, length);
  for (const s of [-1, 1]) {
    b.box('stone', s * (half + .62), .035, (near + far) / 2, 1.24, .14, length);
    b.box('concrete', s * (half + .02), .075, (near + far) / 2, .12, .17, length);
    b.box('flowerWhite', s * (half - .22), .012, (near + far) / 2, .075, .012, length);
  }
  // 巷口弧形转角（图 1 那道曲线）
  for (const s of [-1, 1]) {
    const pts = [];
    for (let k = 0; k <= 10; k++) {
      const a = (k / 10) * Math.PI / 2;
      pts.push(new THREE.Vector3(s * (half - 2.6 + 2.6 * Math.cos(a)), .014, near - 3.4 + 3.4 * Math.sin(a)));
    }
    const tube = new THREE.Mesh(new THREE.TubeGeometry(new THREE.CatmullRomCurve3(pts), 18, .035, 4, false), M.flowerWhite);
    scene.add(tube);
  }
  // 排水格栅
  for (let z = far + 6; z < near; z += 9) {
    for (const s of [-1, 1]) {
      b.box('blackMetal', s * (half - .1), .008, z, .34, .012, .55);
      for (let k = 0; k < 5; k++) b.box('metal', s * (half - .1), .016, z - .2 + k * .1, .3, .008, .018);
    }
  }
  b.flush(scene);

  // 电线杆 + 架空线：图 1/2 里最有生活感的线条
  const leftPoles = [], rightPoles = [];
  for (let z = 34; z > -92; z -= 19) {
    // 阳台视轴正对巷子这一段：电杆会在图 3 的画面正中插一根白柱，跳过。
    if (z > -6 && z < 18) continue;
    leftPoles.push(pole(scene, M, -4.25, z, 8.4 + (Math.abs(z) % 3) * .25, Math.abs(z) | 0));
    if (!(z > -10 && z < 12)) rightPoles.push(pole(scene, M, 4.35, z - 9, 8.0, 7));
  }
  for (let i = 0; i < leftPoles.length - 1; i++) {
    const a = leftPoles[i], c = leftPoles[i + 1];
    for (const [dy, dz, sag] of [[-.55, -.6, .75], [-1.35, .55, .6]]) {
      wire(scene, M, new THREE.Vector3(-4.25, 8.4 + dy, a.group.position.z + dz),
        new THREE.Vector3(-4.25, 8.4 + dy, c.group.position.z + dz), sag);
    }
    if (i < rightPoles.length) {
      const r = rightPoles[i];
      wire(scene, M, new THREE.Vector3(-4.25, 7.85, a.group.position.z),
        new THREE.Vector3(4.35, 7.45, r.group.position.z), 1.15);
    }
  }
  for (let i = 0; i < rightPoles.length - 1; i++) {
    const a = rightPoles[i], c = rightPoles[i + 1];
    wire(scene, M, new THREE.Vector3(4.35, 7.45, a.group.position.z),
      new THREE.Vector3(4.35, 7.45, c.group.position.z), .7);
  }

  // 街灯暖光（按时段由 atmosphere 统一调）
  for (let i = 0; i < leftPoles.length; i += 2) {
    const p = leftPoles[i];
    const l = new THREE.PointLight('#ffc27a', 1, 15, 2);
    l.position.set(p.lampX, p.lampY, p.lampZ);
    scene.add(l);
    atmo.register(l, { day: 1.2, golden: 5, dusk: 15 });
  }

  // 自行车 / 自动贩卖机 / 垃圾桶（沿巷散落）
  bicycle(scene, M, -3.05, 22, .15, 1);
  bicycle(scene, M, -3.05, 19.3, -.2, 2);
  bicycle(scene, M, 3.15, -18, -.12, 3);
  bicycle(scene, M, -3.1, -33, .25, 4);
  vending(scene, M, -3.0, 8.2, .1);
  vending(scene, M, -3.0, -26, .1);

  // 湿路面暖光反射：每个光源下方一条 additive 光带，dusk 才亮
  const glow = glowTexture();
  const glows = [];
  const glowSources = [
    ...leftPoles.filter((_, i) => i % 2 === 0).map(p => [p.lampX, p.lampZ, 3.2]),
    [2.0, -3.35, 3.0], [2.0, 3.35, 3.0], [2.0, 0, 4.2],
    [-3.0, 8.2, 2.2], [-3.0, -26, 2.2],
  ];
  for (const [x, z, w] of glowSources) {
    for (let k = 0; k < 3; k++) {
      const m = new THREE.Mesh(
        new THREE.PlaneGeometry(w * (1 + k * .35), 5.2 + k * 2.6),
        new THREE.MeshBasicMaterial({
          map: glow, transparent: true, opacity: 0,
          blending: THREE.AdditiveBlending, depthWrite: false, side: THREE.DoubleSide,
          color: '#ffc889',
        })
      );
      m.rotation.x = -Math.PI / 2;
      m.position.set(x, .022 + k * .002, z - .6 - k * .5);
      m.renderOrder = 3;
      scene.add(m);
      atmo.register(m, { opacity: .7 - k * .14 });
      glows.push(m);
    }
  }

  // 落樱：静态铺地花瓣 + 空中缓慢飘落的 Points
  const petalGeo = new THREE.BufferGeometry();
  const N = 420;
  const pos = new Float32Array(N * 3);
  const vel = new Float32Array(N);
  let s = 918273;
  const rnd = () => ((s = (Math.imul(s, 1664525) + 1013904223) >>> 0) / 4294967296);
  for (let i = 0; i < N; i++) {
    pos[i * 3] = (rnd() - .5) * 22;
    pos[i * 3 + 1] = rnd() * 12;
    pos[i * 3 + 2] = far + rnd() * length;
    vel[i] = .22 + rnd() * .45;
  }
  petalGeo.setAttribute('position', new THREE.BufferAttribute(pos, 3));
  const petals = new THREE.Points(petalGeo, new THREE.PointsMaterial({
    map: petalTexture(), size: .17, transparent: true, opacity: .92,
    depthWrite: false, sizeAttenuation: true, color: '#ffe2ec',
  }));
  petals.name = 'falling-petals';
  scene.add(petals);

  function update(dt) {
    const p = petalGeo.attributes.position.array;
    for (let i = 0; i < N; i++) {
      p[i * 3 + 1] -= vel[i] * dt;
      p[i * 3] += Math.sin((p[i * 3 + 1] + i) * 1.3) * .006;
      if (p[i * 3 + 1] < .02) {
        p[i * 3 + 1] = 9 + rnd() * 4;
        p[i * 3] = (rnd() - .5) * 22;
        p[i * 3 + 2] = far + rnd() * length;
      }
    }
    petalGeo.attributes.position.needsUpdate = true;
  }

  return { update, glows };
}
