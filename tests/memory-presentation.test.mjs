import assert from 'node:assert/strict';
import { conversationThreads, unifiedConversationThreads, sharedEventTimeline, isMemoryFact, isMemorySummary, isVisibleMemoryFact } from '../src/components/mind-inspector/pages/memoryPresentation.ts';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
const mobile = runInNewContext(readFileSync(new URL('../src-tauri/src/remote/frontend/memory-presentation.js', import.meta.url), 'utf8') + '; MemoryPresentation;');
const base = { id: 'old', content: '原文', memory_type: 'long_term', importance: .7, created_at: 1, tags: ['topic_summary'] };
assert.equal(isMemoryFact(base), false);
assert.equal(isMemorySummary(base), false);
assert.equal(isMemoryFact({ ...base, metadata: { record_kind: 'fact' } }), true);
assert.equal(isMemorySummary({ ...base, metadata: { record_kind: 'session_summary' } }), true);
assert.equal(isMemoryFact({ ...base, metadata: { record_kind: 'subjective' } }), false);
const threads = conversationThreads([{ id: 'session', title: '与你的对话', started_at: 1, ended_at: 2, participants: ['user','vivian'], channels: ['direct'], turns: [
  { id: 'a', speaker: 'user', listener: 'vivian', text: '你好（挥手）', timestamp: 1, channel: 'direct' },
  { id: 'b', speaker: 'user', listener: 'vivian', text: '你好（挥手）', timestamp: 2, channel: 'direct' },
] }], 'vivian');
assert.equal(threads.length, 1);
assert.deepEqual(threads[0].turns.map(t => t.id), ['a', 'b']);
assert.equal(threads[0].turns[0].text, '你好（挥手）');
assert.equal(conversationThreads([], 'vivian').length, 0);
for (const [language, expected] of [['en', 'Conversation between you and Vivian'], ['ja', 'あなたとVivianの会話']]) {
  const original = { id: 'localized', title: '与你的对话', started_at: 1, ended_at: 2, participants: ['user','vivian'], channels: ['direct'], turns: [] };
  assert.equal(conversationThreads([original], 'vivian', language)[0].title, expected);
  assert.equal(conversationThreads([{ ...original, title: '我自己写的标题' }], 'vivian', language)[0].title, '我自己写的标题');
}
assert.equal(threads[0].title, '你与Vivian的对话');
const userChat = (owner, speaker) => ({ id: `${owner}-chat`, title: '与你的对话', participants: ['user', owner],
  channels: ['direct'], started_at: 1, ended_at: 1,
  turns: [{ id: `${owner}-first`, speaker, listener: speaker === 'user' ? owner : 'user', text: '你好', timestamp: 1, channel: 'direct' }] });
for (const [owner, name] of [['vivian', 'Vivian'], ['nana', 'Nana']]) {
  assert.equal(conversationThreads([userChat(owner, owner)], owner)[0].title, `${name}与你的对话`);
  assert.equal(conversationThreads([userChat(owner, 'user')], owner)[0].title, `你与${name}的对话`);
}
const petChat = (owner, first = 'vivian') => ({ id: `${owner}:pets`, source_session_id: 'shared-pet-session',
  title: owner === 'vivian' ? '与Nana的对话' : '与Vivian的对话', participants: ['nana', 'vivian'],
  channels: ['cross_character'], started_at: 10, ended_at: 20, turns: [
    { id: `${owner}-a`, utterance_id: 'speech-a', speaker: first, listener: first === 'vivian' ? 'nana' : 'vivian', text: '你好', timestamp: 10, channel: 'cross_character' },
    { id: `${owner}-b`, utterance_id: 'speech-b', speaker: first === 'vivian' ? 'nana' : 'vivian', listener: first, text: '你好', timestamp: 20, channel: 'cross_character' },
  ] });
for (const [first, title] of [['vivian', 'Vivian与Nana的对话'], ['nana', 'Nana与Vivian的对话']]) {
  const joined = unifiedConversationThreads([petChat('vivian', first)], [petChat('nana', first)]);
  assert.equal(joined.length, 1);
  assert.equal(joined[0].title, title);
  assert.equal(joined[0].owner, first);
  assert.equal(joined[0].turns.length, 2);
  assert.deepEqual(new Set(joined[0].sourceIds), new Set(['vivian:pets', 'nana:pets']));
}
const extra = { ...petChat('nana').turns[1], id: 'new-repeat', utterance_id: 'speech-c', timestamp: 21 };
const partial = { ...petChat('nana'), ended_at: 21, turns: [...petChat('nana').turns.slice(1), extra] };
const union = unifiedConversationThreads([petChat('vivian')], [partial]);
assert.equal(union[0].turns.length, 3, 'partial histories contribute missing speech and preserve actual repeated words');
assert.equal(union[0].time, 21);
const separate = { ...petChat('nana'), source_session_id: 'different-session', turns: petChat('nana').turns.map(t => ({ ...t, utterance_id: `different-${t.utterance_id}` })) };
assert.equal(unifiedConversationThreads([petChat('vivian')], [separate]).length, 2, 'matching text does not merge different conversations');
const legacy = owner => ({ ...petChat(owner), turns: petChat(owner).turns.map(({ utterance_id, ...turn }) => turn) });
assert.equal(unifiedConversationThreads([legacy('vivian')], [legacy('nana')])[0].turns.length, 2, 'legacy same-session mirrors merge one-for-one');
assert.equal(unifiedConversationThreads([userChat('vivian', 'user')], [userChat('nana', 'user')]).length, 2, 'user conversations retain their own pet identity');
console.log('unified conversations: initiator titles, mirrored IDs, partial histories, source aliases and repeated speech passed');
console.log('memory presentation: explicit categories and original history only');

const record = (id, messageId, text, time) => ({ id, title: '会话', started_at: time, ended_at: time,
  participants: ['user','vivian'], channels: ['direct'], turns: [{ id: messageId, speaker: 'user', listener: 'vivian', text, timestamp: time, channel: 'direct' }] });
const originals = [record('first', 'm1', '我们一起完成桌宠项目吧', 1), record('second', 'm2', '桌宠项目今天完成了', 2)];
const progress = (source, quote, phase) => ({ id: 'pet-project', title: '完成桌宠项目', phase, detail: quote,
  source_message_id: source, source_quote: quote, recorded_at: 99999 });
const summary = (id, events) => ({ ...base, id, content: '摘要不应该变成事件', metadata: { record_kind: 'session_summary', event_schema_version: 1, summary_parts: [{ events }] } });
const events = [summary('s1', [progress('m1', '一起完成桌宠项目', 'planned')]), summary('s2', [progress('m2', '桌宠项目今天完成了', 'completed')])];
const timeline = sharedEventTimeline([...events, events[0]], originals);
assert.equal(timeline.length, 1);
assert.equal(timeline[0].progress.length, 2);
assert.deepEqual(timeline[0].conversationIds, ['first','second']);
assert.equal(timeline[0].time, 2); // original report time, not model-provided dates
assert.equal(timeline[0].progress[0].phase, 'planned');
assert.equal(sharedEventTimeline([summary('invalid', [progress('m1', '从未说过的话', 'completed')])], originals).length, 0);
assert.equal(sharedEventTimeline(events, []).length, 0);
assert.equal(sharedEventTimeline(events.map(e => ({...e, consolidated: true})), originals).length, 0);
assert.equal(sharedEventTimeline([{...base, metadata:{record_kind:'session_summary'}}], originals).length, 0);
const similarOther = summary('other', [{...progress('m2', '桌宠项目今天完成了', 'completed'), id:'different-project'}]);
assert.equal(sharedEventTimeline([events[0], similarOther], originals).length, 2);
console.log('event timeline: cross-session progress, deduplication, exact evidence, distinct events');
// Both clients must render canonical turns and evidence-backed event progress identically.
assert.equal(JSON.stringify(mobile.conversationThreads(originals,'vivian')),JSON.stringify(conversationThreads(originals,'vivian')));
assert.equal(JSON.stringify(mobile.sharedEventTimeline(events,originals)),JSON.stringify(timeline));
for (const item of [
  {...base,metadata:{record_kind:'fact'}},
  {...base,metadata:{record_kind:'fact',source:'system_seed'}},
  {...base,metadata:{record_kind:'fact',source:'environment_preset'}},
  {...base,consolidated:true,metadata:{record_kind:'fact'}},
  {...base,metadata:{record_kind:'subjective'}},
  {...base,content:' ',metadata:{record_kind:'fact'}},
]) assert.equal(mobile.isVisibleMemoryFact(item),isVisibleMemoryFact(item));
assert.equal(isVisibleMemoryFact({...base,metadata:{record_kind:'fact',source:'system_seed'}}),false);
assert.equal(isVisibleMemoryFact({...base,metadata:{record_kind:'fact'}}),true);
console.log('desktop/mobile memory projections agree');
