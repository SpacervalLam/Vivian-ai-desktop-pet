import assert from 'node:assert/strict';
import { conversationThreads, sharedEventTimeline, isMemoryFact, isMemorySummary } from '../src/components/mind-inspector/pages/memoryPresentation.ts';
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
