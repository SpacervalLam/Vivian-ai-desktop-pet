import * as THREE from 'three';

/* 樱花与绿植。花冠用 InstancedMesh 的实体花簇而不是 alpha 面片：
 * 这样 DirectionalLight 能把斑驳的树影真的投到公寓灰泥墙上（图 2 的核心氛围），
 * 也不需要 customDepthMaterial 去处理镂空阴影。 */

function rng(seed) {
  let s = seed >>> 0;
  return () => ((s = (Math.imul(s, 1664525) + 1013904223) >>> 0) / 4294967296);
}

// 花簇走「淡粉白 + 小颗粒 + 圆润几何」：第一版用低面数二十面体 + 饱和粉，
// 近景读成一堆粉色石块。细分一次并压低饱和度后才有花团感。
const BLOSSOM_COLORS = ['#fff1f5', '#ffe3ec', '#fff7f8', '#fbd6e2', '#ffe9f0'];

function branch(g, M, from, to, r0, r1) {
  const dir = new THREE.Vector3().subVectors(to, from);
  const len = dir.length();
  const seg = new THREE.Mesh(new THREE.CylinderGeometry(r1, r0, len, 6), M.bark);
  seg.position.copy(from).add(to).multiplyScalar(.5);
  seg.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), dir.clone().normalize());
  seg.castShadow = true;
  g.add(seg);
  return seg;
}

/** 一棵樱花：弯曲主干 → 3~5 主枝 → 每枝 2 段 → 枝端花簇云。 */
export function cherryTree(scene, M, x, z, scale = 1, seed = 1, opts = {}) {
  const r = rng(seed * 7919 + 11);
  const g = new THREE.Group();
  g.name = 'sakura-tree';
  g.position.set(x, 0, z);
  g.scale.setScalar(scale);

  const H = 2.35 + r() * .5;
  const lean = new THREE.Vector3((r() - .5) * .6, 0, (r() - .5) * .6);
  const p0 = new THREE.Vector3(0, 0, 0);
  const p1 = new THREE.Vector3(lean.x * .4, H * .52, lean.z * .4);
  const p2 = new THREE.Vector3(lean.x, H, lean.z);
  branch(g, M, p0, p1, .27, .21);
  branch(g, M, p1, p2, .21, .16);
  // 根部几块石头与苔
  for (let i = 0; i < 4; i++) {
    const rock = new THREE.Mesh(new THREE.IcosahedronGeometry(.22 + r() * .22, 0), M.stone);
    rock.position.set((r() - .5) * 1.3, .1, (r() - .5) * 1.3);
    rock.rotation.set(r() * 3, r() * 3, r() * 3);
    rock.castShadow = true; g.add(rock);
  }

  const tips = [];
  const nBranch = opts.branchCount || 5;
  for (let i = 0; i < nBranch; i++) {
    const a = (i / nBranch) * Math.PI * 2 + r() * .6;
    const spread = 1.5 + r() * 1.35;
    const up = 1.15 + r() * 1.1;
    const q1 = new THREE.Vector3(p2.x + Math.cos(a) * spread * .48, H + up * .55, p2.z + Math.sin(a) * spread * .48);
    const q2 = new THREE.Vector3(p2.x + Math.cos(a) * spread, H + up, p2.z + Math.sin(a) * spread);
    branch(g, M, p2, q1, .115, .075);
    branch(g, M, q1, q2, .075, .035);
    tips.push(q2);
    // 次级短枝
    for (let k = 0; k < 2; k++) {
      const t = q1.clone().lerp(q2, .45 + r() * .4);
      const e = t.clone().add(new THREE.Vector3((r() - .5) * 1.1, .35 + r() * .6, (r() - .5) * 1.1));
      branch(g, M, t, e, .045, .018);
      tips.push(e);
    }
  }

  // 花簇云：每枝端周围撒一团，整体连成冠幅 5~7m（概念图的体量，不是粉贴图）
  const perTip = opts.perTip || 20;
  const total = tips.length * perTip;
  const geo = new THREE.IcosahedronGeometry(.27, 1);
  const inst = new THREE.InstancedMesh(geo, M.blossomA, total);
  inst.name = 'sakura-blossoms';
  inst.castShadow = true;
  inst.receiveShadow = true;
  const dummy = new THREE.Object3D();
  const col = new THREE.Color();
  let idx = 0;
  for (const t of tips) {
    for (let k = 0; k < perTip; k++) {
      const rad = .2 + r() * 1.3;
      const th = r() * Math.PI * 2, ph = (r() - .5) * Math.PI * .8;
      dummy.position.set(
        t.x + Math.cos(th) * Math.cos(ph) * rad,
        t.y + Math.sin(ph) * rad * .75 + .15,
        t.z + Math.sin(th) * Math.cos(ph) * rad
      );
      const k2 = .5 + r() * .72;
      dummy.scale.setScalar(k2);
      dummy.rotation.set(r() * 6, r() * 6, r() * 6);
      dummy.updateMatrix();
      inst.setMatrixAt(idx, dummy.matrix);
      col.set(BLOSSOM_COLORS[(idx * 7 + seed) % BLOSSOM_COLORS.length]);
      inst.setColorAt(idx, col);
      idx++;
    }
  }
  inst.instanceMatrix.needsUpdate = true;
  if (inst.instanceColor) inst.instanceColor.needsUpdate = true;
  g.add(inst);
  scene.add(g);
  return g;
}

/** 图 1 / 图 3 画面上沿压下来的那根前景樱花枝。 */
export function foregroundBranch(scene, M, x, y, z, seed = 3) {
  const r = rng(seed * 4127 + 5);
  const g = new THREE.Group(); g.name = 'foreground-sakura'; g.position.set(x, y, z);
  const from = new THREE.Vector3(-3.2, 0, 0);
  const to = new THREE.Vector3(3.4, -.45, .3);
  branch(g, M, from, to, .18, .075);
  const tips = [];
  for (let i = 0; i < 7; i++) {
    const t = from.clone().lerp(to, .12 + i * .13);
    const e = t.clone().add(new THREE.Vector3((r() - .5) * 1.6, -.5 - r() * 1.1, (r() - .5) * 1.5));
    branch(g, M, t, e, .06, .015);
    tips.push(e);
  }
  const per = 22;
  const n = tips.length * per;
  const inst = new THREE.InstancedMesh(new THREE.IcosahedronGeometry(.25, 1), M.blossomA, n);
  inst.castShadow = true;
  const d = new THREE.Object3D(), c = new THREE.Color();
  let k = 0;
  for (const t of tips) for (let j = 0; j < per; j++) {
    d.position.set(t.x + (r() - .5) * 1.45, t.y + (r() - .5) * 1.0, t.z + (r() - .5) * 1.3);
    d.scale.setScalar(.48 + r() * .72); d.rotation.set(r() * 6, r() * 6, r() * 6);
    d.updateMatrix(); inst.setMatrixAt(k, d.matrix);
    c.set(BLOSSOM_COLORS[(k * 3 + seed) % BLOSSOM_COLORS.length]); inst.setColorAt(k, c);
    k++;
  }
  inst.instanceMatrix.needsUpdate = true;
  g.add(inst); scene.add(g);
  return g;
}

/** 街屋门前的盆栽与花球，用真实小几何，近景不会糊成一片。 */
export function potsAndFlowers(scene, M, positions, seed = 1) {
  const r = rng(seed * 3313 + 7);
  const potGeo = new THREE.CylinderGeometry(.26, .2, .42, 9);
  const bloomGeo = new THREE.IcosahedronGeometry(.1, 0);
  for (const [x, z] of positions) {
    const pot = new THREE.Mesh(potGeo, M.pot);
    pot.position.set(x, .21, z); pot.castShadow = true; scene.add(pot);
    const leaf = new THREE.Mesh(new THREE.IcosahedronGeometry(.34, 0), M.shrub);
    leaf.position.set(x, .52, z); leaf.scale.y = .72; leaf.castShadow = true; scene.add(leaf);
    const n = 9;
    const inst = new THREE.InstancedMesh(bloomGeo, r() > .5 ? M.flowerPink : M.flowerWhite, n);
    const d = new THREE.Object3D();
    for (let i = 0; i < n; i++) {
      d.position.set(x + (r() - .5) * .62, .56 + r() * .28, z + (r() - .5) * .62);
      d.scale.setScalar(.7 + r() * .9); d.updateMatrix(); inst.setMatrixAt(i, d.matrix);
    }
    inst.castShadow = true; scene.add(inst);
  }
}

export function buildNature(scene, M) {
  const trees = [];
  // 关键一棵：站在公寓西侧、主光来路上，树影正好落在 1F/2F 灰泥墙（图 2）
  trees.push(cherryTree(scene, M, -4.35, 5.2, 1.18, 11, { branchCount: 6, perTip: 15 }));
  trees.push(cherryTree(scene, M, -5.2, 24.5, 1.05, 3));
  trees.push(cherryTree(scene, M, -4.8, -17, 1.0, 5));
  trees.push(cherryTree(scene, M, -5.0, -43, 1.12, 7));
  trees.push(cherryTree(scene, M, -5.4, -68, .95, 9));
  trees.push(cherryTree(scene, M, 5.0, -24, 1.0, 13));
  trees.push(cherryTree(scene, M, 5.2, -46, 1.05, 17));
  trees.push(cherryTree(scene, M, -9.0, 36, 1.5, 23, { branchCount: 7, perTip: 16 }));
  // 图 3 阳台外的中景樱花
  trees.push(cherryTree(scene, M, -9.5, -28, 1.2, 29, { branchCount: 6 }));
  trees.push(cherryTree(scene, M, 10.5, -34, 1.1, 31));

  // 路口广场边一棵樱。第一版放 (-6.5,16)，离巷口相机只有 4m，
  // 花簇被透视放大到糊满整张图；退到 18m 外才只是框住画面一角。
  trees.push(cherryTree(scene, M, -10.5, 2, 1.12, 41, { branchCount: 6, perTip: 16 }));
  // 前景压枝：必须落在相机前方（不是后方），否则「樱花框住画面」不成立
  foregroundBranch(scene, M, -1, 9.8, 5, 3);      // 图 1 巷口机位
  foregroundBranch(scene, M, 1.2, 10.4, 12, 8);   // 图 3 阳台外上沿

  const potPos = [];
  for (let z = 36; z > -70; z -= 7.5) {
    potPos.push([-3.15, z], [3.2, z + 3.1]);
  }
  potsAndFlowers(scene, M, potPos, 5);
  return { trees };
}
