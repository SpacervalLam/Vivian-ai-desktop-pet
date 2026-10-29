import assert from 'node:assert/strict';
import { buildSync } from 'esbuild';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const directory = mkdtempSync(join(tmpdir(), 'vivian-street-originality-'));
try {
  const fixture = join(directory, 'check.cjs');
  buildSync({
    stdin: {
      resolveDir: resolve(dirname(fileURLToPath(import.meta.url)), '..'),
      contents: `
        import assert from 'node:assert/strict';
        import { createHash } from 'node:crypto';
        import * as THREE from 'three';
        import { ARTISAN_SHOPS, createArtisanShops } from './src/anime/artisanShops';
        import { STREET_STORIES, createStreetStories } from './src/anime/streetStories';
        import { INFILL_TENANTS } from './src/anime/urbanInfill';
        const context = new Proxy({}, {get: (target,key) => target[key] ?? (() => ({addColorStop(){}}))});
        global.document = {createElement: () => ({getContext: () => context})};
        assert.equal(new Set(ARTISAN_SHOPS.map(s=>s[0])).size, ARTISAN_SHOPS.length);
        const names=[...ARTISAN_SHOPS.map(s=>s[0]),...INFILL_TENANTS.map(s=>s[0]),...STREET_STORIES.map(s=>s.title)];
        assert.equal(new Set(names).size,names.length,'identities must also be unique across the different street systems');
        const scene = new THREE.Scene(), builder = createArtisanShops(), colliders = [], signatures = new Set();
        ARTISAN_SHOPS.forEach((identity,id)=>{
          const x=id*5, shop=builder.build(scene,x,0,5.9,id,colliders), hash=createHash('sha256');
          assert.equal(shop.userData.business,identity[0]);
          shop.traverse(o=>{if(!o.isMesh)return;
            // Compare actual local geometry, excluding names, colors and world placement.
            for(const a of [...Object.values(o.geometry.attributes),o.geometry.index])if(a)hash.update(Buffer.from(a.array.buffer,a.array.byteOffset,a.array.byteLength));
          });
          const signature=hash.digest('hex');assert.ok(!signatures.has(signature),'a renamed or recolored cloned storefront');signatures.add(signature);
          const bounds=new THREE.Box3().setFromObject(shop);
          assert.ok(bounds.max.x-bounds.min.x<3.34,'projecting details must not penetrate the adjacent 3.36m frontage: '+identity[0]+' '+(bounds.max.x-bounds.min.x));
          const door=id%2?-.99:.99;
          for(const offset of[-.4,0,.4])for(const y of[.3,1.1,1.8]){
            const point=new THREE.Vector3(x+door+offset,y,.04);
            assert.ok(!colliders.some(c=>c.kind==='wall'&&new THREE.Box3(c.min,c.max).containsPoint(point)),'doorway must retain a continuous walking opening');
          }
        });
        assert.throws(()=>builder.build(scene,0,0,6,0,colliders),/unused identity/);
        assert.throws(()=>builder.build(scene,0,0,6,ARTISAN_SHOPS.length,colliders),/unused identity/);
        builder.setEnvironment('night');builder.dispose();
        const street=new THREE.Scene(), stories=createStreetStories(street,[]);
        const plan=street.getObjectByName('one-off-street-stories').userData.plan;
        assert.equal(plan.installed.length,STREET_STORIES.length);
        assert.equal(new Set(plan.installed.map(p=>p.id)).size,STREET_STORIES.length);
        for(let i=0;i<plan.installed.length;i++)for(let j=0;j<i;j++){
          const a=plan.installed[i].bounds,b=plan.installed[j].bounds;
          assert.ok(!(a.x0<b.x1&&a.x1>b.x0&&a.z0<b.z1&&a.z1>b.z0),'one-off scenes overlap');
        }
        const moving=[];street.traverse(o=>{if(o.name.endsWith('-moving'))moving.push(o.children[0]);});
        stories.update(0);street.updateMatrixWorld(true);const matrices=moving.map(o=>o.matrixWorld.clone());
        stories.update(10);street.updateMatrixWorld(true);
        assert.equal(moving.length,3);moving.forEach((o,i)=>assert.ok(!o.matrixWorld.equals(matrices[i]),'static batching froze a kinetic sculpture'));
        stories.dispose();assert.equal(street.children.length,0);
        const blocked=new THREE.Scene(), blockedStories=createStreetStories(blocked,[{min:new THREE.Vector3(-100,0,-100),max:new THREE.Vector3(170,10,170)}]);
        assert.equal(blocked.getObjectByName('one-off-street-stories').userData.plan.installed.length,0,'must never force objects into occupied space');
        blockedStories.dispose();
        console.log('Street originality: 40 distinct geometries, unique identities, clear doorways, 12 isolated scenes and 3 live sculptures passed');
      `,
    },
    bundle: true, platform: 'node', format: 'cjs', outfile: fixture, logLevel: 'silent',
  });
  const result = spawnSync(process.execPath, [fixture], { encoding: 'utf8', timeout: 30_000 });
  process.stdout.write(result.stdout ?? ''); process.stderr.write(result.stderr ?? '');
  assert.equal(result.status, 0);
} finally {
  assert.equal(dirname(resolve(directory)), resolve(tmpdir()));
  rmSync(directory, { recursive: true, force: true });
}
