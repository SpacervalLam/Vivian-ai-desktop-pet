import assert from 'node:assert/strict';
import { build } from 'esbuild';

// Drive IPC completions explicitly so overlapping startup and reload reads are deterministic.
const requests = [];
const bundle = await build({
  stdin: {
    contents: `export * from './src/characterPackages'; export * from './src/characters';`,
    resolveDir: process.cwd(),
  },
  bundle: true,
  write: false,
  format: 'esm',
  plugins: [{
    name: 'package-status-fixture',
    setup(builder) {
      builder.onResolve({ filter: /^@tauri-apps\/api\/core$/ }, () => ({ path: 'core', namespace: 'fixture' }));
      builder.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({
        contents: 'export const invoke = () => new Promise(globalThis.enqueueStatus); export const convertFileSrc = path => path;',
      }));
    },
  }],
});
globalThis.enqueueStatus = resolve => requests.push(resolve);
const api = await import('data:text/javascript;base64,' + Buffer.from(bundle.outputFiles[0].text).toString('base64'));
const ids = api.CHARACTER_IDS;
const entry = (id, name, offset) => ({
  root: id,
  manifest: { id, name, files: { 'assets/shared.webp': '' }, anchors: { shared: [[offset]] } },
});
const first = entry('a', 'First', 1);
const second = entry('b', 'Second', 2);
const startup = api.initializeCharacterPackages();
const reload = api.initializeCharacterPackages();
requests[1]({ active: [first, second] });
await reload;
requests[0]({ active: [] });
await startup;
assert.deepEqual(ids, ['vivian', 'nana', 'a', 'b']);
assert.equal(api.characterName('a'), 'First');
assert.equal(api.characterAssetSource('/shared.webp'), 'a/assets/shared.webp');
assert.deepEqual(api.characterFrameData('shared', 'anchors'), [[1]]);
assert.equal(api.characterFrameData('shared', 'scales'), undefined);

const clear = api.initializeCharacterPackages();
requests[2]({ active: [] });
await clear;
assert.deepEqual(ids, ['vivian', 'nana']);
assert.equal(api.characterName('a'), 'a');
assert.equal(api.characterName('constructor'), 'constructor');
assert.equal(api.characterAssetSource('/shared.webp'), null);
assert.equal(api.characterFrameData('shared', 'anchors'), undefined);
console.log('Character registry reload ordering, path precedence and removal verified.');
