import assert from 'node:assert/strict';
import { buildRecentThreads, splitSpeechPrefix } from '../src/components/mind-inspector/pages/memoryPresentation.ts';

const record = (id, content, created_at, tags, metadata) => ({
  id, content, created_at, tags, metadata, memory_type: 'casual_conversation', importance: 0.4,
});

const records = [
  record('summary-1', '我和 Nana 聊了聊：我对她说：喂 Nana，一上来就查户口啊；她回复我：我哪查户口了，就说了你一句', 1000, ['cross_character', 'topic_summary'], { channel: 'cross_character' }),
  record('raw-1', '[Nana says to me] 我哪查户口了，就说了你一句\n\n[Current conversation thread: 查户口]\nTreat this as context, not an obligation to keep talking.', 1001, ['cross_character'], { channel: 'cross_character', speaker: 'nana' }),
  record('summary-2', 'Nana 和我聊天：她说：我哪查户口了，就说了你一句；我回复她：行吧，不记了', 1040, ['cross_character', 'topic_summary'], { channel: 'cross_character' }),
  record('raw-2', '[I say to Nana] 行吧，不记了', 1041, ['cross_character'], { channel: 'cross_character', speaker: 'vivian' }),
  record('scaffold', '[Current conversation thread: 查户口]\nTreat this as context', 1042, [], { channel: 'cross_character' }),
];

const threads = buildRecentThreads(records, 'vivian');
assert.equal(threads.length, 1);
assert.equal(threads[0].title, '与 Nana 的对话');
assert.deepEqual(threads[0].turns.map(({ speaker, text }) => [speaker, text]), [
  ['Vivian', '喂 Nana，一上来就查户口啊'],
  ['Nana', '我哪查户口了，就说了你一句'],
  ['Vivian', '行吧，不记了'],
]);
assert.equal(threads[0].searchText.includes('Current conversation thread'), false);
assert.equal(splitSpeechPrefix('[User says to me] 晚安').body, '晚安');

const direct = buildRecentThreads([
  record('user', '[User says to me] 今天有点累', 2000, [], { channel: 'direct', speaker: 'user' }),
  record('reply', '[I say to User] 那就先休息一下', 2005, [], { channel: 'direct', speaker: 'vivian' }),
], 'vivian');
assert.equal(direct.length, 1);
assert.deepEqual(direct[0].turns.map(({ speaker }) => speaker), ['用户', 'Vivian']);
assert.equal(direct[0].searchText.includes('says to'), false);

console.log('memory presentation: ok');
