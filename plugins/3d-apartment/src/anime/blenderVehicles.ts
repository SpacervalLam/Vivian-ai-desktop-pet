import * as THREE from 'three';
import { GLTFLoader } from 'three/examples/jsm/loaders/GLTFLoader.js';
import { apartmentAssetUrl } from '../apartmentAssets';

/** Replace only the visual children: vehicle roots, traffic poses and colliders
 * remain owned by the existing street systems. GLB geometry is shared by clones. */
export function loadBlenderVehicles(scene: THREE.Scene): () => void {
  const slots: THREE.Object3D[] = [];
  scene.traverse(o => { if (o.userData.vehicleAsset) slots.push(o); });
  let disposed = false;
  const geometries = new Set<THREE.BufferGeometry>();
  const materials = new Set<THREE.Material>();
  const textures = new Set<THREE.Texture>();
  const installed: THREE.Object3D[] = [];
  const paints = ['#c9cdd2', '#e9e5dd', '#2f3b46', '#8d3b39', '#33556e', '#4a5a4a', '#d9b45c', '#3a3a3e'];
  const release = () => {
    geometries.forEach(g => g.dispose()); materials.forEach(m => m.dispose()); textures.forEach(t => t.dispose());
    geometries.clear(); materials.clear(); textures.clear();
  };
  for (const kind of ['sedan', 'van'] as const) {
    if (!slots.some(o => o.userData.vehicleAsset === kind)) continue;
    new GLTFLoader().load(apartmentAssetUrl(`models/vehicles/${kind}.glb`), gltf => {
      const template = gltf.scene;
      template.traverse(o => {
        if (!(o instanceof THREE.Mesh)) return;
        geometries.add(o.geometry); o.castShadow = true; o.receiveShadow = true;
        for (const m of Array.isArray(o.material) ? o.material : [o.material]) {
          materials.add(m); m.userData.outlineWeight = 0;
          for (const value of Object.values(m)) if (value instanceof THREE.Texture) textures.add(value);
        }
      });
      if (disposed) { release(); return; }
      // Match the pre-existing +X-forward collision envelope including mirrors.
      const bounds = new THREE.Box3().setFromObject(template), size = bounds.getSize(new THREE.Vector3());
      const center = bounds.getCenter(new THREE.Vector3());
      if (size.x <= 0 || size.y <= 0 || size.z <= 0) return;
      const dimensions = kind === 'sedan' ? [4.30, 1.42, 1.76] : [5.20, 2.78, 1.94];
      const palette = new Map<number, THREE.Material>();
      for (const slot of slots.filter(o => o.userData.vehicleAsset === kind)) {
        const model = template.clone(true);
        const wrapper = new THREE.Group(); wrapper.name = `blender-${kind}`; wrapper.userData.noMerge = true;
        wrapper.scale.set(dimensions[0]/size.x, dimensions[1]/size.y, dimensions[2]/size.z);
        model.position.set(-center.x, -bounds.min.y, -center.z); wrapper.add(model);
        const tone = Math.abs(slot.userData.vehiclePaint ?? 0) % paints.length;
        model.traverse(o => {
          if (!(o instanceof THREE.Mesh)) return;
          const recolor = (m: THREE.Material) => {
            if (m.name !== 'BodyPaint') return m;
            if (!palette.has(tone)) {
              const colored = (m as THREE.MeshStandardMaterial).clone();
              colored.color.set(paints[tone]); colored.userData.outlineWeight = 0;
              palette.set(tone, colored); materials.add(colored);
            }
            return palette.get(tone)!;
          };
          o.material = Array.isArray(o.material) ? o.material.map(recolor) : recolor(o.material);
        });
        // Fallback resources are still owned by PropKit and its merge cache.
        for (const child of [...slot.children]) slot.remove(child);
        slot.add(wrapper); slot.userData.blenderVehicleLoaded = true; installed.push(wrapper);
      }
    }, undefined, error => {
      if (!disposed) console.warn(`[vehicles] ${kind} GLB unavailable; keeping procedural fallback`, error);
    });
  }
  return () => { disposed = true; installed.forEach(o => o.removeFromParent()); release(); };
}
