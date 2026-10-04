import assert from 'node:assert/strict';
import { build } from 'esbuild';
const listeners = new Map(), calls = [], pending = [];
globalThis.window = globalThis;
globalThis.__speechTest = {
  listen: async (name, cb) => { listeners.set(name, cb); return () => listeners.delete(name); },
  invoke: async (name, args) => {
    if (name !== 'speak_text') return;
    calls.push(args);
    await new Promise((resolve, reject) => pending.push({ resolve, reject }));
  },
};
let frames = new Map(), frameId = 0, clock = 0;
globalThis.requestAnimationFrame = cb => { frames.set(++frameId, cb); return frameId; };
globalThis.cancelAnimationFrame = id => frames.delete(id);
const store = { currentBubble: null, settledBubbles: [], clearBubbleTimer() {},
  addSettledBubble(b) { this.settledBubbles.push(b); }, removeSettledBubble(id) { this.settledBubbles = this.settledBubbles.filter(b => b.id !== id); } };
globalThis.__bubbleTest = { getState: () => store, setState: patch => Object.assign(store, patch) };
const result = await build({ stdin: { contents: `export { TtsStreamQueue } from './src/controllers/TtsStreamQueue'; export * from './src/controllers/speechPresentation'; export { BubbleController } from './src/controllers/BubbleController';`, resolveDir: process.cwd() }, bundle: true, write: false, platform: 'node', format: 'esm', plugins: [{ name: 'mock-native', setup(b) {
  b.onResolve({ filter: /^(?:@tauri-apps\/api\/|.*characterContext$|.*stores\/useAppStore$)/ }, args => ({ path: args.path, namespace: 'mock' }));
  b.onLoad({ filter: /.*/, namespace: 'mock' }, args => ({ contents: args.path.includes('characterContext') ? `export const getCharacterId = () => 'vivian';` : args.path.includes('useAppStore') ? `export const useAppStore = globalThis.__bubbleTest;` : args.path.endsWith('/event') ? `export const listen = globalThis.__speechTest.listen;` : `export const invoke = globalThis.__speechTest.invoke;`, loader: 'js' }));
} }] });
const mod = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
const { TtsStreamQueue: q, BubbleController: bubble } = mod;
const tick = async () => { for (let i=0;i<8;i++) await Promise.resolve(); };
await tick();
q.setEnabled(true);
q.beginStream('A', 'vivian');
q.setPresentation({ expression: 'happy', motion: 'idle', expressionDurationMs: 4500 });
q.feed('first!'); q.feed('second!');
q.setPresentation({ expression: 'shy', motion: '' }); q.feed('third!');
listeners.get('tts:finished')({ payload: { character_id: 'vivian' } });
q.feed('fourth!');
assert.equal(calls.length, 1, 'finished event cannot open a concurrent pump');
assert.equal(calls[0].presentation.expression_duration_ms, 4500);
pending.shift().resolve(); await tick();
assert.equal(calls[1].text, 'second!'); assert.equal(calls[1].presentation.expression, 'happy');
pending.shift().resolve(); await tick();
assert.equal(calls[2].text, 'third!'); assert.equal(calls[2].presentation.expression, 'shy');
pending.shift().resolve(); await tick();
assert.equal(calls[3].text, 'fourth!');
pending.shift().resolve(); await tick();
q.beginStream('B', 'vivian'); q.feed('new!');
assert.equal(calls[4].presentation, null, 'a new stream cannot inherit expression');
const old = pending.shift(); await q.stop();
q.beginStream('C', 'vivian'); q.feed('after cancellation!');
old.reject(new Error('cancelled')); await tick();
q.feed('next!'); assert.equal(calls.length, 6, 'old pump cannot release new serialization guard');
pending.shift().resolve(); await tick(); assert.equal(calls[6].text, 'next!');
pending.shift().resolve(); await tick(); await q.stop();
const happy = mod.mergePresentation(null, { expression: 'happy', expressionDurationMs: 3000 });
assert.equal(mod.mergePresentation(happy, { motion: 'wave' }).expression, 'happy');
const segment = mod.speechSegment('one', happy, 'vivian', 'A'); happy.expression = 'angry';
assert.equal(segment.presentation.expression, 'happy');
const compressed = mod.compactSpeechSegments([segment, {...segment, text: 'two'}, mod.speechSegment('three', happy, 'vivian', 'A')]);
assert.equal(compressed.length, 2); assert.equal(segment.text, 'one');
function advance(count) { for(let i=0;i<count;i++) { clock += 64; const cbs = [...frames.values()]; frames.clear(); cbs.forEach(cb => cb(clock)); } }
bubble.showBubble('hello!'); const releaseOld = bubble.holdForSpeech(); advance(150);
assert.equal(store.currentBubble, 'hello!', 'text stays visible while audio drains');
bubble.showBubble('new!', undefined, { sticker: { id:'test', label:'test', meaning:'test', src:'/test.webp' } });
const releaseNew = bubble.holdForSpeech(); releaseOld(); advance(150);
assert.equal(store.currentBubble, 'new!'); assert.equal(store.settledBubbles.length, 0, 'sticker waits for current speech');
releaseNew(); advance(2);
assert.equal(store.settledBubbles.filter(b => b.sticker).length, 1); bubble.closeAll();
console.log('PASS: FIFO speech, frozen expression, cancellation, metadata merge, bubble ownership and sticker/audio ordering');
