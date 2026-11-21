import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import ts from 'typescript';

// Execute the real App effects with controllable IPC and window readiness.
const source = await readFile(new URL('../src/App.tsx', import.meta.url), 'utf8');
const start = source.indexOf('  // 监听工具执行确认请求');
const end = source.indexOf('  // 主动对话流式 chunk 监听', start);
assert.ok(start >= 0 && end > start);
const code = ts.transpileModule(source.slice(start, end), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
}).outputText;
const flush = () => new Promise(resolve => setImmediate(resolve));
const request = { request_id: 1, char_id: 'vivian', tool: 'wallpaper_set' };

async function setup(ready) {
  const handlers = new Map(), sent = [], cleanups = [];
  const pendingConfirmRef = { current: [] }, completedConfirmRef = { current: new Set() };
  const toastReadyRef = { current: ready };
  let snapshot = [request], tick, release;
  const ensureToastWindow = () => ready ? Promise.resolve() : new Promise(resolve => { release = resolve; });
  new Function('useEffect', 'listen', 'safeUnlisten', 'getCharacterId', 'emit', 'invoke',
    'ensureToastWindow', 'pendingConfirmRef', 'completedConfirmRef', 'toastReadyRef', 'window', code)(
    effect => cleanups.push(effect()),
    async (name, fn) => { handlers.set(name, fn); return () => handlers.delete(name); },
    fn => fn?.(), () => 'vivian', async (name, payload) => sent.push({ name, payload }),
    async () => snapshot, ensureToastWindow, pendingConfirmRef, completedConfirmRef, toastReadyRef,
    { setInterval(fn) { tick = fn; return 1; }, clearInterval() {} },
  );
  await flush();
  return { handlers, sent, pendingConfirmRef, toastReadyRef,
    recover: async () => { tick(); await flush(); },
    finish: () => handlers.get('toast:confirm_done')({ payload: { request_id: 1 } }),
    release: async () => { toastReadyRef.current = true; release(); await flush(); },
    snapshot(value) { snapshot = value; },
    cleanup() { cleanups.forEach(fn => fn()); },
  };
}

const delivered = await setup(true);
assert.equal(delivered.sent.length, 1);
assert.deepEqual(delivered.pendingConfirmRef.current, [], 'delivered recovery must not remain queued for the next toast window');
delivered.finish();
await delivered.recover(); // Simulate an old registry snapshot arriving after confirmation.
assert.equal(delivered.sent.length, 1, 'completed request is not recovered again');
delivered.handlers.get('tool:confirmation_request')({ payload: request });
assert.equal(delivered.sent.length, 1, 'late live events cannot replay a completed request');
delivered.snapshot([{ ...request, request_id: 2 }]);
await delivered.recover();
assert.equal(delivered.sent.at(-1).payload.request_id, 2, 'a new invocation still requires its own confirmation');
delivered.cleanup();

const racing = await setup(false);
assert.equal(racing.pendingConfirmRef.current.length, 1);
racing.finish();
assert.equal(racing.pendingConfirmRef.current.length, 0, 'completion removes queued replay');
await racing.release();
assert.equal(racing.sent.length, 0, 'completion during window creation suppresses the captured snapshot');
racing.cleanup();
assert.equal(racing.handlers.size, 0, 'confirmation listeners are cleaned up');
console.log('Tool confirmation recovery: delivered queues, completed IDs, late events and window creation race passed.');
