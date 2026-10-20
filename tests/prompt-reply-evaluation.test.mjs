import assert from 'node:assert/strict';
import { evaluatePromptReply as evaluate } from '../scripts/prompt-reply-evaluation.mjs';
const response = (text, extra = {}) => JSON.stringify({ text, intent: 'reply', ...extra });
const json = { format: 'json' };

assert.ok(evaluate('just text', json).violations.length);
assert.ok(evaluate('```json\n' + response('嗯') + '\n```', json).violations.length);
for (const payload of ['null', '[]', '{}', '{"text":1,"intent":"reply"}', response('嗯', { intent: 'unknown' })]) {
  assert.ok(evaluate(payload, json).violations.length, payload);
}
assert.ok(evaluate(response('继续聊', { intent: 'no_reply' }), json).violations.length);
assert.deepEqual(evaluate(response('', { intent: 'no_reply' }), json).violations, []);
assert.ok(evaluate(response(''), { ...json, reply_min_chars: 2 }).violations.length, 'JSON wrapper must not satisfy a spoken minimum');
assert.deepEqual(evaluate(response('嗯', { private_note: '建议？' }), {
  ...json, reply_forbidden: ['建议'], reply_max_chars: 1, reply_max_questions: 0,
}).violations, [], 'metadata is not speech');
assert.ok(evaluate(response('嗯', { private_note: '需要的答案' }), { ...json, reply_required: ['需要的答案'] }).violations.length);
assert.deepEqual(evaluate(response('🙂'), { ...json, reply_max_chars: 1 }).violations, [], 'count Unicode code points');
assert.ok(evaluate(response('一二三'), { ...json, reply_max_chars: 2 }).violations.length);
assert.ok(evaluate(response('怎么了？Want to talk?'), { ...json, reply_max_questions: 1 }).violations.length);
assert.deepEqual(evaluate(response('一个建议。'), { ...json, reply_required: ['建议'], reply_max_questions: 0 }).violations, []);
for (const reply of ['DONT_NOTIFY', '', response('', { intent: 'no_reply' })]) {
  assert.deepEqual(evaluate(reply, { proactive_must_decline: true }).violations, []);
}
assert.ok(evaluate('DONT_NOTIFY but here is my reminder', { proactive_must_decline: true }).violations.length);
console.log('Prompt reply evaluation: JSON protocol, spoken-only bounds, metadata isolation and silence verified.');
