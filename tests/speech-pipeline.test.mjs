import assert from 'node:assert/strict';
import { build } from 'esbuild';

globalThis.window = globalThis;
const calls = [];
const pending = [];
globalThis.__speechInvoke = (name, args) => {
  calls.push({ name, args });
  return name === 'speak_text' ? new Promise(resolve => pending.push(resolve)) : Promise.resolve();
};
const bundle = await build({ entryPoints: ['src/controllers/TtsStreamQueue.ts'], bundle: true,
  write: false, format: 'esm', platform: 'node', plugins: [{ name: 'speech-native-mock', setup(b) {
    b.onResolve({ filter: /^@tauri-apps\/api\/(core|event)$|characterContext$/ }, args => ({ path: args.path, namespace: 'native' }));
    b.onLoad({ filter: /.*/, namespace: 'native' }, args => ({ contents: args.path.endsWith('/event')
      ? 'export const listen = async () => () => {};' : args.path.endsWith('/core')
        ? 'export const invoke = (...args) => globalThis.__speechInvoke(...args);'
        : 'export const getCharacterId = () => "vivian";' }));
  } }] });
const queue = (await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`)).TtsStreamQueue;
queue.setEnabled(true);
const settle = async () => { for (let n = 0; n < 12; n++) await Promise.resolve(); };
for (const text of ['こんにちは。', 'Привет!', 'مرحبا!']) {
  queue.speak(text);
  assert.equal(calls.at(-1).args.text, text, 'Unicode letters must be spoken');
  pending.shift()(); await settle();
}
calls.length = 0;
queue.feed('Hello world.');
assert.equal(calls.at(-1).name, 'speak_text', 'English full stop flushes a sentence');
pending.shift()(); await settle();
calls.length = 0;
queue.feed('A'.repeat(450));
queue.flush();
while (pending.length) { pending.shift()(); await settle(); }
assert.deepEqual(calls.filter(c => c.name === 'speak_text').map(c => c.args.text.length), [200, 200, 50]);
assert.equal(calls.filter(c => c.name === 'prefetch_tts').length, 0, 'queue must not race a duplicate synthesis');
const started = queue.speakSync('Stop this sentence.');
await queue.stop();
await started;
pending.shift()?.();
queue.dispose();
delete globalThis.__speechInvoke;
console.log('Speech pipeline: Unicode, English sentences, bounded slices, no duplicate prefetch, cancellation settlement passed.');
