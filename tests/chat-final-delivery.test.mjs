import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';
import { transformSync } from 'esbuild';

const source = (await readFile('src/controllers/ChatController.ts', 'utf8')).replaceAll('\r\n', '\n');
const start = source.indexOf("}>('chat:done', async (event) => {");
const body = source.indexOf('(event) => {', start) + '(event) => {'.length;
const end = source.indexOf('\n      }),', body);
assert.ok(start >= 0 && end > body, 'production done-handler boundary');
const handler = transformSync('handler = async function(event) {' + source.slice(body, end) + '\n}.bind(controller);', { loader: 'ts' }).code;

async function done(streamed, final, cancelWhileStopping = false) {
  const calls = [];
  const context = {
    controller: {
      sessions: new Map([['turn', { text: streamed, characterId: 'vivian' }]]),
      handlers: {},
      finishSessionEmpty: sid => calls.push(['silent', sid]),
      finishSession: (_sid, text) => calls.push(['final', text]),
    },
    TtsStreamQueue: {
      beginStream() {}, setPresentation() {}, flush() {}, isEnabled: () => false,
      stop: async () => {
        calls.push(['stop']);
        if (cancelWhileStopping) context.controller.sessions.delete('turn');
      }, feed: text => calls.push(['speech', text]),
    },
  };
  vm.createContext(context); vm.runInContext(handler, context);
  await context.handler({ payload: { stream_id: 'turn', text: final } });
  return calls;
}
assert.deepEqual(await done('旧台词', ''), [['stop'], ['silent', 'turn']], 'an empty final must discard provisional speech');
assert.deepEqual(await done('旧台词', '修正后的回复'), [['stop'], ['speech', '修正后的回复'], ['final', '修正后的回复']]);
assert.deepEqual(await done('', '只有最终回复'), [['speech', '只有最终回复'], ['final', '只有最终回复']]);
assert.deepEqual(await done('旧台词', '修正后的回复', true), [['stop']], 'cancellation during audio stop must not resurrect the session');
console.log('Desktop final delivery: silence, corrections and final-only speech use the authoritative reply.');
