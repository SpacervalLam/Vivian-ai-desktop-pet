import assert from 'node:assert/strict';
import { build } from 'esbuild';

async function loadQueue(listen) {
  globalThis.__ttsTestListen = listen;
  const bundle = await build({ entryPoints: ['src/controllers/TtsStreamQueue.ts'],
    bundle: true, write: false, format: 'esm', platform: 'node',
    plugins: [{ name: 'mock-native-events', setup(builder) {
      builder.onResolve({ filter: /^@tauri-apps\/api\/(core|event)$|characterContext$/ }, args =>
        ({ path: args.path, namespace: 'mock' }));
      builder.onLoad({ filter: /.*/, namespace: 'mock' }, args => ({ contents:
        args.path.endsWith('/event') ? 'export const listen = (...args) => globalThis.__ttsTestListen(...args);' :
        args.path.endsWith('/core') ? 'export const invoke = async () => {};' :
        'export const getCharacterId = () => "vivian";' }));
    } }] });
  return (await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}#${Math.random()}`)).TtsStreamQueue;
}

const settle = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
let released = 0;
const queue = await loadQueue(async () => () => { released++; });
await settle();
queue.dispose();
queue.dispose();
assert.equal(released, 3, 'all listeners released exactly once');

released = 0;
let finishRegistration;
const pending = await loadQueue(() => new Promise(resolve => { finishRegistration = resolve; }));
pending.dispose();
for (let i = 0; i < 3; i++) {
  finishRegistration(() => { released++; });
  await settle();
}
assert.equal(released, 3, 'late registrations released after disposal');

released = 0;
let registrations = 0;
const failed = await loadQueue(async () => {
  if (++registrations === 2) throw new Error('native events unavailable');
  return () => { released++; };
});
await settle();
assert.equal(released, 1, 'partial initialization rolled back');
failed.dispose();
assert.equal(released, 1);
delete globalThis.__ttsTestListen;
console.log('TTS listeners: disposal, late registration and partial failure cleanup passed.');
