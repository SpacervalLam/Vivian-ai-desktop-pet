import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { transform } from 'esbuild';
import vm from 'node:vm';

const source = await readFile('src/utils/workSessionWindow.ts', 'utf8');
const code = (await transform(source.replace(/^import .*;$/gm, ''), { loader: 'ts', format: 'cjs' })).code;
async function run(existing, delayed = 1, fails = false) {
  const created = [], navigation = [], raised = [];
  let listener, cleaned = 0;
  class Window {
    static async getByLabel(label) { assert.equal(label, 'memory'); return existing ? { label } : null; }
    constructor(label, options) { created.push({ label, options }); }
    async once(name, callback) { if (name === (fails ? 'tauri://error' : 'tauri://created')) queueMicrotask(() => callback({ payload: 'creation failed' })); }
  }
  const context = {
    module: { exports: {} }, exports: {}, WebviewWindow: Window, crypto: { randomUUID: () => 'navigation-id' },
    window: { screen: { width: 1920, height: 1080 }, setTimeout: callback => { queueMicrotask(callback); return 1; } },
    raiseWindow: async (window, label) => raised.push(label),
    listen: async (event, callback) => { assert.equal(event, 'memory:navigation-accepted'); listener = callback; return () => cleaned++; },
    emitTo: async (label, event, payload) => {
      assert.equal(label, 'memory'); assert.equal(event, 'memory:navigate'); navigation.push(payload);
      if (navigation.length >= delayed) listener({ payload: { requestId: payload.requestId } });
    },
  };
  vm.createContext(context); vm.runInContext(code, context);
  const open = context.module.exports.openWorkSession;
  await assert.rejects(open(''), /Missing work session/);
  if (fails) await assert.rejects(open('task/with space'), /creation failed/);
  else await open('task/with space');
  return { created, navigation, raised, cleaned };
}
let state = await run(false);
assert.equal(state.created.length, 1);
const url = new URL(state.created[0].options.url, 'http://local');
assert.equal(url.searchParams.get('nav'), 'code');
assert.equal(url.searchParams.get('work_session'), 'task/with space');
state = await run(true, 3);
assert.equal(state.created.length, 0);
assert.deepEqual(state.raised, ['memory']);
assert.equal(state.navigation.length, 3, 'retry navigation until the prewarmed shell accepts it');
assert.ok(state.navigation.every(payload => payload.workSessionId === 'task/with space'));
assert.equal(state.cleaned, 1);
await run(false, 1, true);

const fingerprintCode = (await transform(await readFile('src/utils/toastDedup.ts', 'utf8'), { loader: 'ts', format: 'cjs' })).code;
const ctx = { module: { exports: {} }, exports: {} }; vm.createContext(ctx); vm.runInContext(fingerprintCode, ctx);
const fingerprint = ctx.module.exports.toastFingerprint;
assert.notEqual(fingerprint('任务已经开始', 'info', 'task-a'), fingerprint('任务已经开始', 'info', 'task-b'));
assert.equal(fingerprint('同一通知', 'info'), fingerprint('同一通知', 'info'));
const toast = await readFile('src/components/Toast.tsx', 'utf8');
assert.ok(toast.includes('action?.cancelLabel'));
assert.ok(toast.includes('onClick={onClose}'), 'Cancel dismisses the prompt instead of canceling the running job');
const office = await readFile('src/components/mind-inspector/pages/CodeAgentPageNew.tsx', 'utf8');
assert.ok(office.includes('session.session_id === requestedSession'));
console.log('Delegated work: exact session navigation, mounting retries, creation errors, independent toasts and dismiss-only cancellation passed.');

// Exercise the actual office creation callback: manual tasks belong to the active pet.
const creationStart = office.indexOf('  const createSessionInWorkspace = useCallback(');
const creationEnd = office.indexOf('  /** 弹出目录选择框', creationStart);
assert.ok(creationStart >= 0 && creationEnd > creationStart);
const creationCode = (await transform(office.slice(creationStart, creationEnd).replace('const createSessionInWorkspace', 'var createSessionInWorkspace'), { loader: 'ts', format: 'cjs' })).code;
const creationRequests = [], chosenSessions = [];
const creationContext = {
  useCallback: callback => callback, creating: false, setCreating: () => {},
  invoke: async (command, args) => {
    creationRequests.push({ command, args });
    if (command === 'get_active_character') return { character_id: 'nana' };
    assert.equal(command, 'coding_new_session');
    return { session_id: 'manual-nana-task', char_id: args.charId };
  },
  refreshSessions: async () => [], switchSession: session => chosenSessions.push(session),
  setInput: () => {}, inputRef: { current: null }, t: key => key,
  notifyError: message => assert.fail(message),
};
vm.createContext(creationContext); vm.runInContext(creationCode, creationContext);
const manualSession = await creationContext.createSessionInWorkspace('G:/work');
assert.equal(manualSession.char_id, 'nana');
assert.equal(creationRequests[1].args.charId, 'nana');
assert.equal(creationRequests[1].args.workingDirectory, 'G:/work');
assert.equal(chosenSessions[0].session_id, 'manual-nana-task');
console.log('Manual office creation uses the active companion for callback ownership.');
