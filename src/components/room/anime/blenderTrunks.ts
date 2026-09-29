import * as THREE from 'three';
import { GLTFLoader } from 'three/examples/jsm/loaders/GLTFLoader.js';
import { trunkFields, type TrunkVariant } from './foliage';
import { apartmentAssetUrl } from '../apartmentAssets';

/**
 * 把 Blender 出的树干几何装进各模块的干场。
 *
 * 分工：`scripts/trees/build_trunks.py` 出几何，`foliage.trunkFields` 铺实例，
 * 这里只负责**异步换几何**。样板是 `blenderVehicles.ts`：载入前先用程序化回退
 * （`foliage` 的 `unitTrunkGeometry`，带锥度 + 根部外扩的旋转体），载入后替换 ——
 * 场景在这期间是可看的，GLB 拿不到也不会退回一根光秃秃的圆柱。
 *
 * 三件必须做对的事：
 *
 *  1. **几何要烘进世界矩阵**。`export_yup=True` 到底把 Y-up 转换烘进顶点还是挂在
 *     根节点上，随导出器版本而变；直接取 `mesh.geometry` 有可能拿到一根躺倒的干。
 *     这里统一 `geometry.applyMatrix4(mesh.matrixWorld)`，两种情形都对。
 *  2. **尺寸要归一**。GLB 按**真实尺寸**出（2.42m 的干就是 2.42m，可以直接打开量），
 *     而实例矩阵用的是"基部半径 1、高 1、原点在树脚"的约定（`scale(r, len, r)`）——
 *     不除回来整片林子会大 2.42 倍。标称尺寸由导出脚本写进节点 `extras`
 *     （`trunkH` / `trunkR`），从节点及其父链上读。
 *  3. **材质不来自 GLB**。场景是 toon 渲染，MeshStandardMaterial 会格格不入，
 *     而且会丢掉各模块自己的树皮色。只取几何 + UV，材质由 `TrunkField.build()`
 *     用 `toon(色, {map: trunkBarkTexture()})` 建。
 *
 * 同一变体的所有实例共用一份几何，所以 `install()` 换一次就全变，draw call 不增。
 */
const SOURCE: Record<TrunkVariant, string> = {
  sakura: 'room/models/trees/trunk-sakura.glb',
  green: 'room/models/trees/trunk-green.glb',
};

/** 从节点往父链上找导出脚本写下的标称尺寸。 */
function nominalSize(object: THREE.Object3D): { h: number; r: number } {
  let h = 0, r = 0;
  for (let o: THREE.Object3D | null = object; o; o = o.parent) {
    h = Number(o.userData?.trunkH) || h;
    r = Number(o.userData?.trunkR) || r;
    if (h > 0 && r > 0) break;
  }
  return { h, r };
}

export function loadBlenderTrunks(): () => void {
  let disposed = false;
  const geometries = new Set<THREE.BufferGeometry>();

  for (const variant of ['sakura', 'green'] as const) {
    // 没有哪个模块用到这个变体就不去取 —— 别为几棵不存在的树付一次请求。
    if (!trunkFields.some((f) => f.wants(variant))) continue;
    new GLTFLoader().load(
      apartmentAssetUrl(SOURCE[variant]),
      (gltf) => {
        gltf.scene.updateMatrixWorld(true);
        // 收进数组而不是往一个 `let mesh` 里赋值：闭包里的赋值 TS 的控制流分析
        // 看不到，后面 `if (!mesh) return` 会把 mesh 收窄成 never。
        const meshes: THREE.Mesh[] = [];
        gltf.scene.traverse((o) => {
          const m = o as THREE.Mesh;
          if (m.isMesh) meshes.push(m);
        });
        const source = meshes[0];
        if (!source) return;
        // 一个 glTF primitive 出一个 mesh。资产是单材质的（见 build_trunks.py），
        // 所以正常只有 1 个；多出来说明导出时拆了材质，静默只取第一个会缺一块。
        if (meshes.length > 1) {
          console.warn(`[trunks] ${variant} GLB has ${meshes.length} primitives; using the first. `
            + 'The asset is meant to be single-material.');
        }
        const geometry = source.geometry.clone();
        // 见文件头第 1 条：把 Y-up 转换（不管它烘在哪一层）折进顶点。
        geometry.applyMatrix4(source.matrixWorld);
        // 见文件头第 2 条：归一。X/Z 除以标称基部半径而不是高度 —— 干的水平尺度
        // （板根外扩、树皮棱沟、轴线的弯）全都以 R 为基准，除错轴会把板根压扁。
        const { h, r } = nominalSize(source);
        if (h > 0 && r > 0) {
          geometry.scale(1 / r, 1 / h, 1 / r);
        } else {
          // extras 丢了（换导出器/手工重导）时的降级路径：按包围盒推。原点就在
          // 树脚，所以 max.y 是地面以上的高度；R/H 在本次资产里是 0.060~0.062。
          geometry.computeBoundingBox();
          const height = geometry.boundingBox?.max.y || 1;
          const radius = height * 0.061;
          geometry.scale(1 / radius, 1 / height, 1 / radius);
          console.warn(`[trunks] ${variant} GLB has no trunkH/trunkR extras; `
            + 'fell back to bounding-box normalisation');
        }
        geometry.computeBoundingSphere();
        if (disposed) { geometry.dispose(); return; }
        geometries.add(geometry);
        for (const field of trunkFields) field.install(variant, geometry);
      },
      undefined,
      (error) => {
        if (!disposed) {
          console.warn(`[trunks] ${variant} GLB unavailable; keeping procedural fallback`, error);
        }
      },
    );
  }

  return () => {
    disposed = true;
    geometries.forEach((g) => g.dispose());
    geometries.clear();
  };
}
