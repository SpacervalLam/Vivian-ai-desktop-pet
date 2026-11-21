import assert from 'node:assert/strict';
import { buildSync } from 'esbuild';
const result = buildSync({ entryPoints: ['src/components/settings/requestBodyConfig.ts'], bundle: true, write: false, format: 'esm', platform: 'node' });
const { decodeRequestBody, encodeRequestBody, dynamicRequestTemplate } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
const legacy = { temperature: 0.7 };
assert.deepEqual(decodeRequestBody(legacy), { mode: 'legacy', body: legacy });
assert.deepEqual(encodeRequestBody('legacy', JSON.stringify(legacy)), legacy);
assert.deepEqual(decodeRequestBody(null), { mode: 'merge', body: {} });
for (const mode of ['merge', 'replace']) {
  const body = { messages: { $requestRef: '/messages' }, tools: [], custom: null };
  const config = encodeRequestBody(mode, JSON.stringify(body));
  assert.deepEqual(config, { $request: { mode, body } });
  assert.deepEqual(decodeRequestBody(config), { mode, body });
}
assert.equal(encodeRequestBody('merge', ''), null);
assert.deepEqual(encodeRequestBody('merge', ' \n { "model" : "test",\n\t"temperature":  0.5 } \r\n'), {$request: {mode:'merge',body:{model:'test',temperature:0.5}}});
assert.throws(() => encodeRequestBody('replace', ''));
for (const text of ['null', '[]', 'true', '"text"', '{invalid']) assert.throws(() => encodeRequestBody('merge', text));
assert.deepEqual(dynamicRequestTemplate({ messages: [], model: 'example', 'a/b~c': true }), {
  messages: { $requestRef: '/messages' }, model: { $requestRef: '/model' }, 'a/b~c': { $requestRef: '/a~1b~0c' },
});
console.log('Request body UI: legacy compatibility, full merge/replacement, validation and dynamic references passed.');
