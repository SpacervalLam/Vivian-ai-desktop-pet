import assert from 'node:assert/strict';
import { build } from 'esbuild';

const bundle = await build({ entryPoints: ['src/utils/chatStreamSettlement.ts'], bundle: true,
  platform: 'node', format: 'esm', write: false });
const { settleChatStream } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
const row = (id, content) => ({ id, content });
const previous = [row('user', 'question'), row('a', 'old paragraph'), row('other', 'other character'), row('b', 'old tail'), row('next', 'next user turn')];
assert.deepEqual(settleChatStream(previous, ['a', 'b'], [row('new', 'corrected')]), [
  previous[0], row('a', 'corrected'), previous[2], previous[4],
]);
assert.deepEqual(settleChatStream(previous, ['a', 'b'], []), [previous[0], previous[2], previous[4]], 'silence removes this stream only');
assert.deepEqual(settleChatStream(previous, [], [row('done-only', 'complete')]), [...previous, row('done-only', 'complete')]);
assert.equal(settleChatStream(previous, ['a', 'b'], [{ id: 'voice', content: '', voice: { audioPath: 'voice.wav' } }])[1].voice.audioPath, 'voice.wav');
assert.equal(previous[1].content, 'old paragraph', 'settlement must not mutate existing state');
console.log('Stream settlement: corrections, silence, final-only, voice and interleaved characters preserved.');
