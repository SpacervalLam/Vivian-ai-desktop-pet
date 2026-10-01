// One-time migration from Vivian's existing furniture builders. Run Vite first.
// PLAYWRIGHT_PATH may point to the bundled playwright directory.
import { createRequire } from 'node:module';
import { mkdir, writeFile } from 'node:fs/promises';
const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_PATH || 'playwright');
const root = new URL('../../', import.meta.url);
await mkdir(new URL('art/room', root), { recursive: true });
const browser = await chromium.launch({ channel: 'chrome', headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
  await page.goto('http://127.0.0.1:1420/plugins/3d-apartment/preview.html');
  await page.waitForTimeout(12000);
  await page.screenshot({ path: new URL('art/room/before.png', root).pathname.replace(/^\/(\w:)/, '$1') });
  await page.goto('http://127.0.0.1:1420/plugins/3d-apartment/tools/room/export-base.html');
  const result = await page.evaluate(async () => {
    const THREE = await import('/node_modules/three/build/three.module.js');
    const props = await import('/plugins/3d-apartment/src/anime/props.ts');
    const textures = await import('/plugins/3d-apartment/src/anime/toon.ts');
    const layout = await (await fetch('/plugins/3d-apartment/src/dormLayout.json')).json();
    const { GLTFExporter } = await import('/node_modules/three/examples/jsm/exporters/GLTFExporter.js');
    props.setArtStyle(layout.palette, layout.style);
    const scene = new THREE.Scene();
    const materials = new Map();
    const manifest = [];
    let triangles = 0;
    // Keep interactive doors, split wall decorations and irregular collider props procedural.
    const skip = new Set(['jpLifestyle', 'jpBath', 'jpSlatWall', 'jpKitchenShelf', 'jpPendant', 'jpMirror', 'jpPlant', 'jpPlanter', 'jpFloorClutter']);
    for (const spec of layout.furniture) {
      if (!spec.kind.startsWith('jp') || skip.has(spec.kind)) continue;
      const build = props['build' + spec.kind[0].toUpperCase() + spec.kind.slice(1)];
      if (!build) continue;
      const group = build(spec);
      group.name = spec.id;
      group.userData = { roomAssetId: spec.id, kind: spec.kind, size: spec.size };
      group.position.fromArray(spec.pos);
      group.rotation.y = spec.rot || 0;
      let n = 0;
      group.traverse(mesh => {
        if (!mesh.isMesh) return;
        mesh.name = spec.id + '_part_' + n++;
        mesh.geometry.computeBoundingBox();
        const size = mesh.geometry.boundingBox.getSize(new THREE.Vector3());
        mesh.userData = { primitive: mesh.geometry.type, dimensions: size.toArray(), castShadow: mesh.castShadow, receiveShadow: mesh.receiveShadow };
        triangles += (mesh.geometry.index?.count || mesh.geometry.attributes.position.count) / 3;
        mesh.material = (Array.isArray(mesh.material) ? mesh.material : [mesh.material]).map(old => {
          if (materials.has(old)) return materials.get(old);
          const c = old.color;
          // Existing JP palette: desaturated blue/gray cloth, warm timber, dark hardware.
          const fabric = old.map === textures.jpFabricTexture() || old.map === textures.jpFutonTexture();
          const metal = old.userData.outlineWeight === 1 && !old.transparent;
          const mat = new THREE.MeshStandardMaterial({
            color: old.color, map: old.map || null, roughness: fabric ? 0.94 : metal ? 0.34 : 0.65,
            metalness: metal ? 0.55 : 0, transparent: old.transparent, opacity: old.opacity,
            side: old.side, depthWrite: old.depthWrite,
            emissive: old.emissive || (old.isMeshBasicMaterial ? old.color : 0),
            emissiveMap: old.isMeshBasicMaterial ? old.map : old.emissiveMap,
            emissiveIntensity: old.isMeshBasicMaterial ? 0.65 : old.emissiveIntensity || 0,
          });
          mat.name = `MAT_${fabric ? 'Fabric' : metal ? 'Metal' : 'Surface'}_${materials.size}`;
          materials.set(old, mat);
          return mat;
        });
        if (mesh.material.length === 1) mesh.material = mesh.material[0];
      });
      scene.add(group);
      manifest.push({ id: spec.id, kind: spec.kind, size: spec.size, pos: spec.pos, rot: spec.rot || 0, parts: n });
    }
    const glb = await new GLTFExporter().parseAsync(scene, { binary: true, onlyVisible: false });
    const data = await new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result.split(',')[1]);
      reader.onerror = () => reject(reader.error);
      reader.readAsDataURL(new Blob([glb]));
    });
    return { data, manifest, triangles, materials: materials.size };
  });
  await writeFile(new URL('art/room/procedural-base.glb', root), Buffer.from(result.data, 'base64'));
  delete result.data;
  await writeFile(new URL('art/room/migration.json', root), JSON.stringify(result, null, 2));
  console.log(JSON.stringify({ objects: result.manifest.length, triangles: result.triangles, materials: result.materials }));
} finally { await browser.close(); }
