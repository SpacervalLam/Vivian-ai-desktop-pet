import assert from 'node:assert/strict';
import { buildSync } from 'esbuild';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const directory = mkdtempSync(join(tmpdir(), 'vivian-trunk-memory-'));
const fixture = join(directory, 'test.cjs');
try {
  buildSync({
    stdin: {
      resolveDir: resolve(dirname(fileURLToPath(import.meta.url)), '..'),
      contents: `
        import assert from 'node:assert/strict';
        import * as THREE from 'three';
        import { createTrunkField, collectTrunkFields } from './src/anime/foliage';
        // Canvas pixels are irrelevant to the field/mesh ownership graph.
        const context = new Proxy({}, {get: (object, key) => object[key] ?? (() => ({addColorStop(){}}))});
        global.document = {createElement: () => ({getContext: () => context})};
        function makeScene() {
          const scene = new THREE.Scene(), field = createTrunkField();
          field.add({x:0,z:0,y0:0,h:2.8,r:.14,lean:0,leanDir:0,variant:'sakura',seed:1});
          scene.add(field.build('trunks', '#543c30'));
          return {scene, field};
        }
        async function test() {
          const active = makeScene();
          let weakScene;
          (() => {
            const old = makeScene();
            assert.deepEqual(collectTrunkFields(old.scene), [old.field]);
            assert.deepEqual(collectTrunkFields(active.scene), [active.field], 'assets must not target another scene');
            const mesh = old.scene.children[0].children[0];
            const geometry = new THREE.BoxGeometry();
            old.field.install('sakura', geometry);
            assert.equal(mesh.userData.trunkAsset, true);
            assert.notEqual(mesh.geometry, geometry, 'each field owns its installed clone');
            weakScene = new WeakRef(old.scene);
          })();
          for (let i = 0; i < 8; i++) {
            await new Promise(resolve => setImmediate(resolve));
            global.gc();
          }
          assert.equal(weakScene.deref(), undefined, 'the registry must not retain an abandoned scene through mesh parents');
          assert.deepEqual(collectTrunkFields(active.scene), [active.field]);
          console.log('Foliage memory: scene isolation, asset installation and garbage collection passed');
        }
        test().catch(error => {console.error(error); process.exitCode = 1;});
      `,
    },
    outfile: fixture, bundle: true, platform: 'node', format: 'cjs', logLevel: 'silent',
  });
  const result = spawnSync(process.execPath, ['--expose-gc', fixture], { encoding: 'utf8', timeout: 30_000 });
  process.stdout.write(result.stdout ?? '');
  process.stderr.write(result.stderr ?? '');
  assert.equal(result.status, 0, 'trunk scene ownership regression');
} finally {
  assert.equal(dirname(resolve(directory)), resolve(tmpdir()));
  rmSync(directory, { recursive: true, force: true });
}
