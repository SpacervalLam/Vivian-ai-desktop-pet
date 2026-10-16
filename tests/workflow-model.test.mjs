import assert from 'node:assert/strict';
import { buildSync } from 'esbuild';

const { outputFiles } = buildSync({ entryPoints: ['src/components/mind-inspector/pages/workflowModel.ts'], bundle: true, write: false, format: 'esm', platform: 'node' });
const { deriveWorkflow, mergeWorkflowRequests, workflowDuration } = await import(`data:text/javascript;base64,${Buffer.from(outputFiles[0].text).toString('base64')}`);
const user = (content, timestamp = 100) => ({ role: 'user', content, timestamp });
const use = (id, arguments_ = { path: id }) => ({ role: 'tool_use', content: '', timestamp: 200, tool_name: 'read_file', tool_call_id: id, tool_arguments: arguments_ });
const result = (id, success = true) => ({ role: 'tool_result', content: `${id}:out`, timestamp: 350, tool_name: 'read_file', tool_call_id: id, tool_success: success, tool_duration_ms: 150 });
const request = (id, overrides = {}) => ({ id, turn_message_index: 0, turn: 0, step: 1, attempt: 1, purpose: 'assistant', model: 'test', started_at: 150, completed_at: 250, duration_ms: 100, status: 'complete', response_preview: 'checking', tool_calls: [], usage: null, error: null, ...overrides });

// Live replacements and persisted separate call/result rows yield identical request links.
const call = request('first', { tool_calls: [{ id: 'a', name: 'read_file' }, { id: 'b', name: 'read_file' }] });
const messages = [user('Inspect files'), { role: 'thinking', content: 'considering', timestamp: 120 }, use('a'), use('b'), result('b'), result('a')];
let model = deriveWorkflow(messages, [call], false);
assert.equal(model.turns.length, 1);
assert.equal(model.toolCount, 2);
assert.deepEqual(model.turns[0].calls[0].tools.map(tool => [tool.id, tool.message.content]), [['a', 'a:out'], ['b', 'b:out']]);
assert.deepEqual(model.turns[0].calls[0].tools[0].message.tool_arguments, { path: 'a' });
assert.equal(model.turns[0].status, 'complete');
assert.equal(model.turns[0].durationMs, 250);
model = deriveWorkflow([user('Legacy result'), use('a'), { ...result('a'), tool_name: null }], [call], false);
assert.equal(model.turns[0].calls[0].tools[0].status, 'complete');
assert.equal(model.turns[0].calls[0].tools[0].name, 'read_file');

// Mid-task user steering does not split a model request into a synthetic new turn.
model = deriveWorkflow([user('Task'), { ...user('Guidance'), guided: true }, { ...user('More'), interjected: true }, result('a')], [call], true);
assert.equal(model.turns.length, 1);
assert.equal(model.turns[0].status, 'running');
assert.equal(model.turns[0].calls[0].tools[1].status, 'running');
assert.equal(deriveWorkflow([user('Task')], [call], false).turns[0].calls[0].tools[0].status, 'interrupted');

// A provider may reuse an opaque tool ID in a later turn; names and IDs stay turn scoped.
model = deriveWorkflow([user('One'), result('a'), user('Two', 600), { ...result('a', false), content: 'second result', timestamp: 800 }], [call, request('second', { turn: 1, turn_message_index: 3, started_at: 650, tool_calls: [{ id: 'a', name: 'read_file' }] })], false);
assert.equal(model.turns[0].calls[0].tools[0].message.content, 'a:out');
assert.equal(model.turns[1].calls[0].tools[0].message.content, 'second result');
assert.equal(model.turns[1].status, 'error');
assert.equal(model.turns[1].calls.length, 1, 'turn identity survives different live and persisted message indices');

// Aggregate assistant tool-call envelopes and neutral host notices are not duplicate tools.
model = deriveWorkflow([user('Task'), { role: 'tool_use', tool_arguments: [{ id: 'a' }], content: '', timestamp: 160 }, { role: 'notice', content: 'compaction', timestamp: 170 }, result('a')], [call], false);
assert.equal(model.toolCount, 2); // one result and one announced, unexecuted call
assert.equal(model.turns[0].tools.length, 1);

// Retries remain distinct; a recovered request does not mark the whole turn failed.
const failed = request('retry-1', { status: 'error', completed_at: 180, error: 'connection reset' });
const recovered = request('retry-2', { attempt: 2, started_at: 200, completed_at: 400 });
model = deriveWorkflow([user('Retry')], [recovered, failed], false);
assert.deepEqual(model.turns[0].calls.map(call => call.request.attempt), [1, 2]);
assert.equal(model.turns[0].status, 'complete');
assert.equal(model.turns[0].calls[0].request.status, 'error');
const running = request('retry-2', { status: 'running', completed_at: null, duration_ms: null });
assert.equal(mergeWorkflowRequests([recovered], [running])[0].status, 'complete');
assert.equal(mergeWorkflowRequests([running], [recovered])[0].status, 'complete');
assert.equal(deriveWorkflow([user('Crashed')], [running], false).turns[0].status, 'interrupted');

// History without observations is preserved without inventing model requests or token usage.
model = deriveWorkflow([user('Old'), { role: 'assistant', content: 'Reply', timestamp: 250 }], [], false);
assert.equal(model.requestCount, 0);
assert.equal(model.turns[0].calls.length, 0);
assert.equal(model.turns[0].status, 'complete');
assert.equal(deriveWorkflow([], [], false).turns.length, 0);
model = deriveWorkflow([user('Program'), result('a')], [request('code', { purpose: 'code' })], false);
assert.equal(model.turns[0].calls[0].tools.length, 1);

// Orphan results and pending model-announced calls both count, without counting a linked result twice.
model = deriveWorkflow([user('Task'), result('orphan')], [call], false);
assert.equal(model.toolCount, 3);
assert.equal(call.usage, null);
assert.equal(workflowDuration(null), '—');
assert.equal(workflowDuration(1200), '1.2 s');

// The projection handles long sessions without recursion over the request chain.
const many = Array.from({ length: 2000 }, (_, index) => request(`r${index}`, { started_at: 200 + index }));
assert.equal(deriveWorkflow([user('Long task')], many, true).requestCount, 2000);
console.log('Workflow: replay, parallel/repeated tools, steering, retries, interruption, old sessions and long histories passed.');
