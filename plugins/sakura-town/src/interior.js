import * as THREE from 'three';
import { Batch, railing } from './batch.js';
import { APT } from './buildings.js';

/* 图 3：木质室内 → 木地板阳台 → 黑细栏杆 → 樱花街巷 → 东京天际线。
 * 这一条视轴必须真的贯通，所以单元是公寓 3F 端部的真空腔（见 buildings.js），
 * 落地玻璃、窗帘、家具全部按「相机在室内离玻璃 1.7m」布置。 */
export const UNIT = { x0: APT.x0, x1: APT.x1, z0: 2.0, z1: APT.z1, y0: APT.floors[2], y1: APT.floors[3] };

function chair(scene, M, x, z, rotY, withThrow = false) {
  const g = new THREE.Group(); g.position.set(x, UNIT.y0 + .22, z); g.rotation.y = rotY;
  const wood = M.wood, weave = M.curtain;
  const leg = (px, pz, h = .42) => {
    const m = new THREE.Mesh(new THREE.BoxGeometry(.055, h, .055), wood);
    m.position.set(px, h / 2, pz); g.add(m);
  };
  for (const sx of [-.3, .3]) for (const sz of [-.28, .28]) leg(sx, sz);
  const seat = new THREE.Mesh(new THREE.BoxGeometry(.66, .06, .6), weave);
  seat.position.y = .45; g.add(seat);
  // 靠背后仰
  const back = new THREE.Mesh(new THREE.BoxGeometry(.66, .62, .05), weave);
  back.position.set(0, .76, -.28); back.rotation.x = -.18; g.add(back);
  for (const sx of [-.33, .33]) {
    const post = new THREE.Mesh(new THREE.BoxGeometry(.055, .72, .055), wood);
    post.position.set(sx, .78, -.28); post.rotation.x = -.18; g.add(post);
    const arm = new THREE.Mesh(new THREE.BoxGeometry(.055, .05, .62), wood);
    arm.position.set(sx, .7, 0); g.add(arm);
    const armLeg = new THREE.Mesh(new THREE.BoxGeometry(.05, .26, .05), wood);
    armLeg.position.set(sx, .57, .27); g.add(armLeg);
  }
  if (withThrow) {
    const throwCloth = new THREE.Mesh(new THREE.BoxGeometry(.5, .58, .035), M.curtain);
    throwCloth.position.set(-.06, .82, -.245); throwCloth.rotation.x = -.18;
    g.add(throwCloth);
    const tail = new THREE.Mesh(new THREE.BoxGeometry(.5, .22, .035), M.curtain);
    tail.position.set(-.06, .5, -.2); g.add(tail);
  }
  g.traverse(o => { if (o.isMesh) o.castShadow = true; });
  scene.add(g);
  return g;
}

function roundTable(scene, M, x, z) {
  const g = new THREE.Group(); g.position.set(x, UNIT.y0 + .22, z);
  const top = new THREE.Mesh(new THREE.CylinderGeometry(.34, .34, .055, 16), M.wood);
  top.position.y = .46; top.castShadow = true; g.add(top);
  for (let i = 0; i < 3; i++) {
    const a = i * Math.PI * 2 / 3;
    const leg = new THREE.Mesh(new THREE.CylinderGeometry(.028, .038, .45, 6), M.woodDark);
    leg.position.set(Math.cos(a) * .22, .23, Math.sin(a) * .22); g.add(leg);
  }
  const cup = new THREE.Mesh(new THREE.CylinderGeometry(.055, .045, .1, 10), M.flowerWhite);
  cup.position.set(.1, .54, -.06); g.add(cup);
  const book = new THREE.Mesh(new THREE.BoxGeometry(.24, .035, .18), M.plasterWarm);
  book.position.set(-.11, .51, .08); book.rotation.y = .3; g.add(book);
  scene.add(g);
}

export function buildInterior(scene, M, atmo) {
  const { x0, x1, z0, z1, y0, y1 } = UNIT;
  const b = new Batch(M, 'sakura-interior');
  const glassX = x0;

  // ---- 室内地面 / 顶棚木条 / 地毯 ----
  b.box('floor', (x0 + x1) / 2, y0 + .015, (z0 + z1) / 2, x1 - x0, .03, z1 - z0);
  for (let z = z0 + .35; z < z1; z += .46)
    b.box('woodDark', (x0 + x1) / 2, y0 + .033, z, x1 - x0, .006, .012);
  b.box('curtain', 8.6, y0 + .038, 5.6, 5.2, .012, 4.0);   // 浅色地毯（用布材质近似）
  for (let x = x0 + .5; x < x1; x += .34)
    b.box('wood', x, y1 - .2, (z0 + z1) / 2, .18, .12, z1 - z0 - .3);  // 木格栅吊顶

  // ---- 整面落地玻璃 + 黑细框（图 3 的画框） ----
  b.box('glass', glassX + .02, (y0 + y1) / 2, (z0 + z1) / 2, .03, y1 - y0 - .12, z1 - z0 - .12);
  const frameX = glassX - .04;
  b.box('blackMetal', frameX, y0 + .06, (z0 + z1) / 2, .1, .12, z1 - z0);
  b.box('blackMetal', frameX, y1 - .06, (z0 + z1) / 2, .1, .12, z1 - z0);
  for (const z of [z0, z0 + (z1 - z0) / 3, z0 + 2 * (z1 - z0) / 3, z1])
    b.box('blackMetal', frameX, (y0 + y1) / 2, z, .1, y1 - y0, .1);
  b.box('blackMetal', frameX, y0 + 1.05, (z0 + z1) / 2, .08, .055, z1 - z0);

  // ---- 窗帘：左右两幅米色布，垂到地面 ----
  // 窗帘只压在玻璃左右两端，不铺褶皱：第一版五条褶皱 × 两幅直接把
  // 整面落地玻璃糊成灰竖条，图 3 要的是「透」，不是遮挡。
  for (const [z, w] of [[z0 + .24, .34], [z1 - .24, .34]]) {
    b.box('curtain', glassX + .16, (y0 + y1) / 2, z, .1, y1 - y0 - .1, w);
    b.box('curtain', glassX + .2, (y0 + y1) / 2, z + (z < 5 ? .04 : -.04), .05, y1 - y0 - .16, w * .8);
  }
  b.box('woodDark', glassX + .2, y1 - .1, (z0 + z1) / 2, .12, .1, z1 - z0);

  // ---- 阳台：木地板 + 黑细栏杆（视轴的第二层） ----
  const bx0 = x0 - 1.55, bx1 = x0;
  b.box('concrete', (bx0 + bx1) / 2, y0 - .09, (z0 + z1) / 2, x1 - x0 + 1.7, .18, z1 - z0 + .1);
  b.box('floor', (bx0 + bx1) / 2, y0 + .015, (z0 + z1) / 2, bx1 - bx0, .03, z1 - z0);
  for (let x = bx0 + .3; x < bx1; x += .34)
    b.box('woodDark', x, y0 + .033, (z0 + z1) / 2, .012, .006, z1 - z0);
  railing(b, 'z', z0, z1, bx0 - .02, y0 + .02, 1.08);
  railing(b, 'x', bx0, bx1, z0 + .02, y0 + .02, 1.08);
  railing(b, 'x', bx0, bx1, z1 - .02, y0 + .02, 1.08);

  // ---- 阳台花池与室内边柜 ----
  for (const [px, pz] of [[bx0 + .38, z0 + .65], [bx0 + .38, z1 - .65]]) {
    b.box('pot', px, y0 + .28, pz, .78, .56, .62);
    b.box('soil', px, y0 + .565, pz, .68, .02, .52);
  }
  // 边柜（图 3 左侧木格栅柜 + 台灯）
  b.box('wood', x0 + .75, y0 + .42, z0 + .75, 1.5, .84, .62);
  for (let k = 0; k < 12; k++)
    b.box('woodDark', x0 + .1 + k * .12, y0 + .42, z0 + .44, .045, .78, .03);
  b.box('woodDark', x0 + .75, y0 + .86, z0 + .75, 1.6, .06, .7);
  // 挂画
  b.box('woodDark', x0 + 2.0, y0 + 1.85, z1 - .16, .9, 1.12, .06);
  b.box('flowerWhite', x0 + 2.0, y0 + 1.85, z1 - .12, .78, 1.0, .02);
  b.flush(scene);

  // ---- 独立家具：藤椅、小圆桌、台灯、花瓶 ----
  chair(scene, M, bx0 + .62, z0 + 2.1, -.35, true);
  chair(scene, M, bx0 + .62, z1 - 2.0, .38, false);
  roundTable(scene, M, bx0 + .68, (z0 + z1) / 2);

  const lamp = new THREE.Group();
  lamp.position.set(x0 + .75, y0 + .86, z0 + .75);
  const shade = new THREE.Mesh(new THREE.CylinderGeometry(.19, .23, .34, 12), M.curtain);
  shade.position.y = .22; lamp.add(shade);
  const bulb = new THREE.Mesh(new THREE.SphereGeometry(.1, 10, 8),
    new THREE.MeshBasicMaterial({ color: '#ffd9a0' }));
  bulb.position.y = .22; lamp.add(bulb);
  const stem = new THREE.Mesh(new THREE.CylinderGeometry(.025, .035, .12, 6), M.woodDark);
  stem.position.y = .05; lamp.add(stem);
  scene.add(lamp);
  const lampLight = new THREE.PointLight('#ffcb8d', 1, 5, 2);
  lampLight.position.set(x0 + .75, y0 + 1.1, z0 + .75);
  scene.add(lampLight);
  atmo.register(lampLight, { day: 1.5, golden: 3.5, dusk: 6 });

  // 花瓶 + 樱花枝（室内近景的一枝）
  const vase = new THREE.Mesh(new THREE.CylinderGeometry(.1, .14, .34, 10), M.pot);
  vase.position.set(x0 + 1.3, y0 + .17, z1 - 1.1); scene.add(vase);
  const twigMat = M.bark;
  for (let i = 0; i < 5; i++) {
    const a = i * 1.3;
    const twig = new THREE.Mesh(new THREE.CylinderGeometry(.008, .018, .9, 5), twigMat);
    twig.position.set(x0 + 1.3 + Math.cos(a) * .22, y0 + .72, z1 - 1.1 + Math.sin(a) * .22);
    twig.rotation.z = Math.cos(a) * .38; twig.rotation.x = Math.sin(a) * .38;
    scene.add(twig);
    for (let k = 0; k < 4; k++) {
      const bloom = new THREE.Mesh(new THREE.IcosahedronGeometry(.075, 0), k % 2 ? M.blossomB : M.blossomC);
      bloom.position.set(twig.position.x + Math.cos(a) * (.12 + k * .07),
        y0 + .88 + k * .12, twig.position.z + Math.sin(a) * (.12 + k * .07));
      scene.add(bloom);
    }
  }

  // 阳台植物：绣球花球 + 直立绿植
  for (const [px, pz, seed] of [[bx0 + .38, z0 + .65, 1], [bx0 + .38, z1 - .65, 2]]) {
    const bush = new THREE.Mesh(new THREE.IcosahedronGeometry(.38, 1), M.shrub);
    bush.position.set(px, y0 + .82, pz); bush.castShadow = true; scene.add(bush);
    let s = seed * 7331;
    const r = () => ((s = (Math.imul(s, 1664525) + 1013904223) >>> 0) / 4294967296);
    const n = 14;
    const inst = new THREE.InstancedMesh(new THREE.IcosahedronGeometry(.085, 0), M.flowerPink, n);
    const d = new THREE.Object3D();
    for (let i = 0; i < n; i++) {
      d.position.set(px + (r() - .5) * .72, y0 + .82 + (r() - .5) * .55, pz + (r() - .5) * .55);
      d.scale.setScalar(.7 + r() * .7); d.updateMatrix(); inst.setMatrixAt(i, d.matrix);
    }
    inst.castShadow = true; scene.add(inst);
  }

  // 阳台地板上的落瓣
  const petals = new THREE.InstancedMesh(new THREE.CircleGeometry(.035, 5), M.blossomC, 60);
  const pd = new THREE.Object3D();
  let ps = 4451;
  const pr = () => ((ps = (Math.imul(ps, 1664525) + 1013904223) >>> 0) / 4294967296);
  for (let i = 0; i < 60; i++) {
    pd.position.set(bx0 + pr() * (bx1 - bx0), y0 + .038, z0 + pr() * (z1 - z0));
    pd.rotation.set(-Math.PI / 2, 0, pr() * 6.28);
    pd.updateMatrix(); petals.setMatrixAt(i, pd.matrix);
  }
  scene.add(petals);

  return { glassX, bx0 };
}
