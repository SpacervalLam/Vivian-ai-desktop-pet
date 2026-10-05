import assert from 'node:assert/strict';
import { buildSync } from 'esbuild';

const bundle = buildSync({
  entryPoints: ['src/components/mind-inspector/pages/StreamFadeText.tsx'],
  bundle: true, write: false, format: 'esm', platform: 'browser',
  define: { 'process.env.NODE_ENV': '"production"' },
});
const { advanceFade, STREAM_FADE_MS } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`,
);
const empty = { text: '', stable: '', chunks: [], nextId: 0 };
const first = advanceFade(empty, '你好 🌟', 0);
const next = advanceFade(first, '你好 🌟 world', 100);
assert.equal(next.chunks[0].id, first.chunks[0].id, 'existing text keeps its animation identity');
assert.equal(next.chunks[1].text, ' world', 'only new text fades in');
const settled = advanceFade(next, next.text, STREAM_FADE_MS + 100);
assert.equal(settled.stable, next.text);
assert.equal(settled.chunks.length, 0);
const replaced = advanceFade(next, '新的回复', 200);
assert.equal(replaced.stable + replaced.chunks.map(c => c.text).join(''), '新的回复');
let stream = empty;
for (let i = 0; i < 1000; i++) {
  stream = advanceFade(stream, stream.text + '字', i * 20);
  assert.ok(stream.chunks.length <= Math.ceil(STREAM_FADE_MS / 20));
  assert.equal(stream.stable + stream.chunks.map(c => c.text).join(''), stream.text);
}
console.log('Stream fade: append identity, Unicode, completion, replacement and bounded live tail passed.');
