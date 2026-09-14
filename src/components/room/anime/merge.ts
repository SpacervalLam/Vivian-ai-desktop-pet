/**
 * 静态几何合批。
 *
 * 房间是"微缩模型"：几千个零件全都是钉死的，一件都不会动，却被当成几千个
 * 独立物体逐个提交给 GPU。每个 draw call 的 CPU 成本（状态切换 + 队列排序）
 * 远高于它画的那几十个三角形，所以这里的瓶颈从来不是填充率，是提交次数。
 *
 * 做法：把一件道具里**材质相同**的 mesh 合成一个几何。变换烘焙进顶点，材质
 * 相同就不需要在中间切状态，几千次提交压成几十次。
 *
 * 边界（刻意不越界的地方）：
 *  - 只在一件道具内部合，不跨道具合。跨道具合会吃掉按道具显隐、按道具剔除的
 *    能力，而挂墙装饰必须能跟着各自的墙一起让开。
 *  - 透明材质不参与。透明物体要按到相机的距离逐个排序，合成一个之后排序就
 *    失去意义，会出现前后叠错的穿帮。
 *  - userData.noMerge 的子树整体跳过（装饰组由调用方自己决定怎么合）。
 *    注意这个标记只对"父级发起的合批"生效：以该子树为 root 单独调用本函数时，
 *    它自己和子树照常合并 —— "别把我并进父级那批"不等于"我内部也不许合"。
 *  - 合并失败的桶原样退回。少赚一次合批远好过整个道具渲染不出来，
 *    更远好过把它挪错层级。
 */

import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';

type Bucket = {
  material: THREE.Material | THREE.Material[];
  castShadow: boolean;
  receiveShadow: boolean;
  renderOrder: number;
  meshes: THREE.Mesh[];
};

type Candidate = { mesh: THREE.Mesh; parent: THREE.Object3D };

function attributesSignature(geo: THREE.BufferGeometry): string {
  return Object.keys(geo.attributes).sort().join(',');
}

/**
 * mergeGeometries 对输入很挑剔：属性集合必须完全一致，且要么全是索引几何、
 * 要么全不是。不满足就返回 null。这里先自查一遍，不满足就不合。
 */
function canMerge(geos: THREE.BufferGeometry[]): boolean {
  if (geos.length < 2) return false;
  const sig = attributesSignature(geos[0]);
  if (!geos.every((g) => attributesSignature(g) === sig)) return false;
  return geos.every((g) => !!g.index) || geos.every((g) => !g.index);
}

/**
 * 把 root 下面的实体 mesh 按材质合并。
 *
 * 必须在挂描边之前调用：描边要从合并后的少数几个 mesh 上长出来，
 * 顺序反过来会把几千个描边壳也一起卷进来。
 *
 * @returns 合并前后的实体 mesh 数量
 */
export function mergeByMaterial(root: THREE.Object3D): { before: number; after: number } {
  // 变换烘焙依赖正确的世界矩阵，而此时 root 多半还没挂进场景，先本地算一遍。
  root.updateMatrixWorld(true);
  const invRoot = root.matrixWorld.clone().invert();

  const collected: Candidate[] = [];
  const walk = (o: THREE.Object3D, isRoot = false) => {
    if (!isRoot && o.userData?.noMerge === true) return;
    const mesh = o as THREE.Mesh;
    // 描边壳是上一次调用留下的，重入时不该被当成实体
    if (mesh.isMesh && mesh.name !== '__outline' && mesh.geometry) {
      collected.push({ mesh, parent: o.parent ?? root });
    }
    for (const child of [...o.children]) walk(child);
  };
  walk(root, true);

  const before = collected.length;
  if (before < 2) return { before, after: before };

  /* ---------------- 分桶 ---------------- */

  const buckets: Bucket[] = [];
  for (const { mesh } of collected) {
    const mat = mesh.material as THREE.Material | THREE.Material[];
    // 透明物体必须能按距离单独排序，不参与合批
    if (Array.isArray(mat) ? mat.some((mm) => mm.transparent) : mat.transparent) continue;
    const hit = buckets.find(
      (b) =>
        b.material === mat &&
        b.castShadow === mesh.castShadow &&
        b.receiveShadow === mesh.receiveShadow &&
        b.renderOrder === mesh.renderOrder
    );
    if (hit) hit.meshes.push(mesh);
    else
      buckets.push({
        material: mat,
        castShadow: mesh.castShadow,
        receiveShadow: mesh.receiveShadow,
        renderOrder: mesh.renderOrder,
        meshes: [mesh],
      });
  }

  /* ---------------- 逐桶合并 ---------------- */

  // 几何的引用计数：有的 builder 会把同一个 geometry 实例挂到多个 mesh 上，
  // 合并掉其中一个就 dispose 会让另一批跟着变空白。计数归零才真正释放。
  const geoUse = new Map<THREE.BufferGeometry, number>();
  for (const { mesh } of collected) geoUse.set(mesh.geometry, (geoUse.get(mesh.geometry) ?? 0) + 1);

  const release = (geo: THREE.BufferGeometry) => {
    const left = (geoUse.get(geo) ?? 1) - 1;
    geoUse.set(geo, left);
    if (left <= 0) geo.dispose();
  };

  const rel = new THREE.Matrix4();
  const mergedMeshes: THREE.Mesh[] = [];

  for (const b of buckets) {
    if (b.meshes.length < 2) continue;

    const geos = b.meshes.map((mesh) => {
      const geo = mesh.geometry.clone();
      rel.multiplyMatrices(invRoot, mesh.matrixWorld);
      geo.applyMatrix4(rel);
      return geo;
    });

    // 桶内索引态归一：倒角基元（RoundedBoxGeometry）是非索引几何，和同材质的
    // 索引几何（圆柱、挤压体等）落在同一桶时，mergeGeometries 会因索引态混合
    // 直接拒合，整桶退化成逐 mesh 提交。统一转成非索引就能保住合批——顶点数
    // 会涨，但合批省下的 draw call 远比这点顶点值钱。
    if (geos.some((g) => !g.index)) {
      for (let i = 0; i < geos.length; i++) {
        const g = geos[i];
        if (g.index) {
          geos[i] = g.toNonIndexed();
          g.dispose();
        }
      }
    }

    if (!canMerge(geos)) {
      for (const g of geos) g.dispose();
      continue;
    }

    const combined = mergeGeometries(geos, false);
    for (const g of geos) g.dispose();
    if (!combined) continue; // 合并失败：原件留在原地，什么都不动

    const out = new THREE.Mesh(combined, b.material);
    out.castShadow = b.castShadow;
    out.receiveShadow = b.receiveShadow;
    out.renderOrder = b.renderOrder;
    // 组内只要有一件声明了不描边，合并后整个都不描——
    // 描边是逐 mesh 加的，合并之后没法再区分内部哪一段不描。
    if (b.meshes.some((mm) => mm.userData?.noOutline)) out.userData.noOutline = true;

    for (const mesh of b.meshes) {
      mesh.parent?.remove(mesh);
      // 只放几何。材质走的是共享缓存，在这里 dispose 会把别的道具一起弄坏。
      release(mesh.geometry);
    }
    mergedMeshes.push(out);
  }

  for (const mesh of mergedMeshes) root.add(mesh);

  let after = 0;
  root.traverse((o) => {
    const mesh = o as THREE.Mesh;
    if (mesh.isMesh && mesh.name !== '__outline') after++;
  });
  return { before, after };
}

/**
 * 冻结整棵子树的矩阵更新。
 *
 * three 每帧会对整个场景图 traverse 一次重算世界矩阵。这里几千个零件全是
 * 静止的，逐帧重算纯属浪费。冻结后只在第一次手动算准，之后完全跳过。
 *
 * 只有确认整棵子树都不会再动的时候才能调用（角色、浮尘、光束都不行）。
 */
export function freezeStatic(root: THREE.Object3D): void {
  // 先把父链的 matrixWorld 刷新到最新。下面的 updateMatrixWorld 直接采用
  // parent.matrixWorld 的当前值，不刷新父链——父链过期（单元组刚抬高、
  // 新组刚挂上）就会带着错误的偏移定格；之后父级整树冻结时会跳过这些
  // matrixWorldAutoUpdate=false 的子树，错位永远修不回来（典型：抬高
  // 3.4m 后，挂进 decor/ceilingDecor 的吊灯、窗户、挂钟全部定格在一楼）。
  root.updateWorldMatrix(true, false);
  root.traverse((o) => {
    o.updateMatrix();
    o.matrixAutoUpdate = false;
  });
  root.updateMatrixWorld(true);
  // matrixWorldAutoUpdate=false 会让 three 在父级更新时跳过整棵子树。
  // 只给每个子物体设 matrixAutoUpdate 是不够的——那样只是省掉本地矩阵计算，
  // 世界矩阵的 traverse 一次都不会少。
  root.matrixWorldAutoUpdate = false;
}
