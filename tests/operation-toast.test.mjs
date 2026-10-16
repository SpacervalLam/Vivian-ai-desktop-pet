import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { transform } from 'esbuild';
import vm from 'node:vm';

const source = (await readFile('src/App.tsx', 'utf8')).replace(/\r\n/g, '\n');
const callback = async (event, end) => {
  const start = source.indexOf('(event) => {', source.indexOf(`'${event}'`));
  assert.ok(start >= 0);
  return (await transform(`var handler = ${source.slice(start, source.indexOf(end, start))}\n};`, { loader: 'ts' })).code;
};
const toasts = [];
let bubbles = 0, clears = 0, resets = 0;
const context = {
  getCharacterId: () => 'vivian',
  t: (key, values) => values ? `${key}:${values.name}` : key,
  showToast: (...args) => toasts.push(args),
  BubbleController: { showBubble: () => bubbles++, closeAll: () => clears++ },
  TtsStreamQueue: { resetBuffer: () => resets++ },
  crossStreamRoleRef: { current: 'source' }, crossStreamIdRef: { current: 'current' },
  crossStreamTextRef: { current: 'partial' }, crossListenerNameRef: { current: 'Nana' },
};
vm.createContext(context);
vm.runInContext(await callback('cross:error', '\n        });'), context);
context.handler({ payload: { source_id: 'vivian', target_id: 'nana', stream_id: 'old' } });
assert.equal(toasts.length, 0, 'stale errors cannot disrupt a newer conversation');
assert.equal(clears, 0);
context.handler({ payload: { source_id: 'vivian', target_id: 'nana', stream_id: 'current' } });
assert.equal(toasts.length, 1);
assert.equal(toasts[0][1], 'error');
assert.equal(clears, 1); assert.equal(resets, 1);
assert.equal(context.crossStreamTextRef.current, '');
assert.equal(context.crossStreamIdRef.current, '');
context.getCharacterId = () => 'nana';
context.handler({ payload: { source_id: 'vivian', target_id: 'nana', stream_id: 'current' } });
assert.equal(toasts.length, 1, 'only the initiating character reports the error');
assert.equal(clears, 2);

vm.runInContext(await callback('screen:analysis', '\n    }).then'), context);
context.getCharacterId = () => 'vivian';
toasts.length = 0;
for (const handled of [true, false]) {
  context.handler({ payload: { character_id: 'vivian', result: { handled, saved_path: 'shot.png' } } });
  assert.equal(toasts.at(-1)[1], 'success');
}
context.handler({ payload: { character_id: 'vivian', error: 'capture failed' } });
assert.equal(toasts.at(-1)[0], 'capture failed');
assert.equal(toasts.at(-1)[1], 'error');
context.handler({ payload: { character_id: 'nana', error: 'foreign' } });
context.handler({ payload: { character_id: 'vivian', result: { handled: true } } });
assert.equal(toasts.length, 3, 'foreign results and analysis without a save emit no operation toast');
assert.equal(bubbles, 0, 'operation status never speaks through a pet bubble');
console.log('Operation toasts: cross-character errors, stream cleanup, screenshot saves and failures passed.');
