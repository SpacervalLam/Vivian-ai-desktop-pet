import * as THREE from 'three';
import { GLTFLoader } from 'three/examples/jsm/loaders/GLTFLoader.js';
import manifest from './blenderFurnitureManifest.json';

export const blenderFurnitureIds = new Set(manifest.map((item) => item.id));
export type FurnitureSlot = {
  root: THREE.Object3D;
  spec: { id: string; kind: string; size: [number, number, number] };
};

/** The procedural furniture remains usable until a complete asset has loaded. */
export function loadBlenderFurniture(slots: FurnitureSlot[], onLoaded: () => void, prepareAsset?: (asset: THREE.Object3D, id: string) => void): () => void {
  let disposed = false;
  const textures = new Set<THREE.Texture>();
  const ownedGeometries = new Set<THREE.BufferGeometry>();
  const ownedMaterials = new Set<THREE.Material>();
  const release = () => {
    ownedGeometries.forEach((g) => g.dispose());
    ownedMaterials.forEach((m) => m.dispose());
    textures.forEach((t) => t.dispose());
    ownedGeometries.clear(); ownedMaterials.clear(); textures.clear();
  };
  new GLTFLoader().load(`${import.meta.env.BASE_URL}room/models/vivian-furniture.glb`, (gltf) => {
    gltf.scene.traverse((o) => {
      if (!(o instanceof THREE.Mesh)) return;
      ownedGeometries.add(o.geometry);
      const materials = Array.isArray(o.material) ? o.material : [o.material];
      for (const material of materials) {
        ownedMaterials.add(material);
        for (const value of Object.values(material)) {
          if (value instanceof THREE.Texture) textures.add(value);
        }
      }
      o.castShadow = o.userData.castShadow !== false;
      o.receiveShadow = true;
    });
    if (disposed) { release(); return; }
    const assets = new Map<string, THREE.Object3D>();
    gltf.scene.traverse((o) => {
      if (typeof o.userData.roomAssetId === 'string') assets.set(o.userData.roomAssetId, o);
    });
    let replaced = 0;
    for (const { root, spec } of slots) {
      const asset = assets.get(spec.id);
      // A changed layout size uses its builder until the Blender source is regenerated.
      const source = manifest.find((entry) => entry.id === spec.id);
      if (!asset || !source || source.kind !== spec.kind ||
          !source.size.every((value, i) => Math.abs(value - spec.size[i]) < 0.0001)) continue;
      // Furniture builders own their local lamps; GLB exports intentionally omit lights.
      const lights: THREE.Light[] = [];
      root.traverse((o) => { if (o instanceof THREE.Light) lights.push(o); });
      root.updateMatrixWorld(true);
      lights.forEach((light) => root.attach(light));
      for (const child of [...root.children]) {
        if (child instanceof THREE.Light) continue;
        child.traverse((o) => {
          if (!(o instanceof THREE.Mesh)) return;
          o.geometry.dispose();
          for (const m of Array.isArray(o.material) ? o.material : [o.material]) {
            if (!m.userData.__roomCached) m.dispose();
          }
        });
        root.remove(child);
      }
      asset.removeFromParent();
      asset.position.set(0, 0, 0);
      asset.quaternion.identity();
      asset.scale.set(1, 1, 1);
      prepareAsset?.(asset, spec.id);
      root.add(asset);
      asset.updateMatrixWorld(true);
      asset.traverse((o) => { o.updateMatrix(); o.matrixAutoUpdate = false; });
      root.userData.blenderFurniture = true;
      replaced++;
    }
    console.info(`[room] Blender furniture: ${replaced}/${slots.length}`);
    onLoaded();
  }, undefined, (error) => {
    if (!disposed) console.warn('[room] Blender furniture unavailable; keeping procedural furniture.', error);
  });
  return () => { disposed = true; release(); };
}

