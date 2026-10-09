import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { transform } from 'esbuild';
import vm from 'node:vm';

const office = (await readFile('src/components/mind-inspector/pages/CodeAgentPageNew.tsx', 'utf8')).replace(/\r\n/g, '\n');
const start = office.indexOf('  const refreshSessions = useCallback(');
const end = office.indexOf('  const loadActiveModelId', start);
const code = (await transform(office.slice(start, end).replace('const refreshSessions', 'var refreshSessions').replace('const loadOlderSessions', 'var loadOlderSessions'), { loader: 'ts', format: 'cjs' })).code;
const data = Array.from({ length: 75 }, (_, i) => ({ session_id: `session-${i}`, updated_at: 75 - i }));
let visible, more, requestFailure = false;
const context = {
  useCallback: callback => callback, sessionWindow: { current: 30 }, sessionRefresh: { current: 0 },
  activeIdRef: { current: null },
  loadingOlderSessions: false, setLoadingOlderSessions: () => {},
  setSessions: value => { visible = typeof value === 'function' ? value(visible ?? []) : value; }, setHasOlderSessions: value => { more = value; },
  invoke: async (name, { offset, limit }) => { assert.equal(name, 'coding_list_sessions'); if (requestFailure) throw new Error('offline'); return data.slice(offset, offset + limit); },
};
vm.createContext(context); vm.runInContext(code, context);
await context.refreshSessions();
assert.equal(visible.length, 30); assert.equal(more, true);
await context.loadOlderSessions();
assert.equal(visible.length, 60); assert.equal(more, true);
await context.loadOlderSessions();
assert.equal(visible.length, 75); assert.equal(more, false);
assert.equal(new Set(visible.map(s => s.session_id)).size, 75);
assert.equal(visible[74].session_id, 'session-74');
requestFailure = true; await context.refreshSessions();
assert.equal(visible.length, 75, 'a failed refresh must not erase already loaded history');
assert.ok(office.includes('{ sessionId: requestedSession }'), 'deep links can fetch a session outside the loaded page');
console.log('Recent page, older history, exhaustion, deduplication and failed refresh preservation passed.');

const linkStart = office.indexOf('  useEffect(() => {\n    if (!requestedSession) return;');
const linkEnd = office.indexOf('  /** 在指定目录新建会话', linkStart);
assert.ok(linkStart >= 0 && linkEnd > linkStart);
const linkCode = (await transform(office.slice(linkStart, linkEnd), { loader: 'tsx', format: 'cjs' })).code;
let selected, acknowledged = 0;
const linkContext = {
  requestedSession: 'session-74', refreshSessions: async () => data.slice(0, 30),
  useEffect: callback => callback(), switchSession: session => { selected = session; },
  invoke: async (name, args) => { assert.equal(name, 'coding_list_sessions'); assert.equal(args.sessionId, 'session-74'); return [data[74]]; },
  navigation: { clearPageParams: () => acknowledged++ }, setPageError: error => assert.fail(error),
  setSessions: updater => updater(data.slice(0, 30)),
};
vm.createContext(linkContext); vm.runInContext(linkCode, linkContext);
await new Promise(resolve => setImmediate(resolve));
assert.equal(selected.session_id, 'session-74'); assert.equal(acknowledged, 1);

const deleteStart = office.indexOf('  const deleteWorkspace = useCallback(');
const deleteEnd = office.indexOf('  // 工作区管理菜单', deleteStart);
const deleteCode = (await transform(office.slice(deleteStart, deleteEnd).replace('const deleteWorkspace', 'var deleteWorkspace'), { loader: 'tsx', format: 'cjs' })).code;
const deleted = [];
const deleteContext = {
  useCallback: callback => callback, refreshSessions: async () => [], switchSession: () => {},
  activeIdRef: { current: null }, setActiveId: () => {}, setMessages: () => {},
  setManualOrder: updater => updater(data.map(s => s.session_id)), notifyError: error => assert.fail(error),
  invoke: async (name, args) => {
    if (name === 'coding_list_sessions') { assert.equal(args.workspace, 'project'); return data; }
    assert.equal(name, 'coding_delete_session'); deleted.push(args.sessionId);
  },
};
vm.createContext(deleteContext); vm.runInContext(deleteCode, deleteContext);
await deleteContext.deleteWorkspace('project');
assert.equal(deleted.length, 75, 'workspace deletion includes sessions outside the loaded page');
console.log('Deep links and workspace-wide deletion also include unloaded old sessions.');

const hydrateStart = office.indexOf('  const hydrateSession = useCallback(');
const hydrateEnd = office.indexOf('  const refreshSessions =', hydrateStart);
const switchStart = office.indexOf('  const switchSession = useCallback(');
const switchEnd = office.indexOf('  useEffect(() => {', switchStart);
const selectionCode = (await transform((office.slice(hydrateStart, hydrateEnd) + office.slice(switchStart, switchEnd))
  .replace('const hydrateSession', 'var hydrateSession').replace('const switchSession', 'var switchSession'), { loader: 'ts', format: 'cjs' })).code;
let selectedId = 'current', selectedMessages = ['current history'], rows = [];
const pending = new Map(), failures = [];
const selectionContext = {
  useCallback: callback => callback, sessionSelection: { current: 0 },
  invoke: async (_, { sessionId }) => new Promise((resolve, reject) => pending.set(sessionId, { resolve, reject })),
  setSessions: updater => { rows = updater(rows); }, setActiveId: id => { selectedId = id; },
  setMessages: messages => { selectedMessages = messages; }, notifyError: error => failures.push(error),
};
for (const name of ['setRunning', 'setThinking', 'setStreamingText', 'setThinkingText', 'setStats', 'setQueue', 'setPreviewTabs', 'setActivePreview', 'setPermission', 'setReasoningLevel', 'setModelName']) selectionContext[name] = () => {};
vm.createContext(selectionContext); vm.runInContext(selectionCode, selectionContext);
const old = selectionContext.switchSession({ session_id: 'old', history_loaded: false, messages: [] });
assert.equal(selectedId, 'current');
assert.equal(selectedMessages[0], 'current history', 'unloaded history must not clear the current conversation');
const newer = selectionContext.switchSession({ session_id: 'newer', history_loaded: false, messages: [] });
pending.get('newer').resolve([{ session_id: 'newer', history_loaded: true, messages: [{ content: 'complete history' }] }]);
await newer;
pending.get('old').resolve([{ session_id: 'old', history_loaded: true, messages: [{ content: 'late history' }] }]);
await old;
assert.equal(selectedId, 'newer', 'a late history response cannot replace a more recent selection');
assert.equal(selectedMessages[0].content, 'complete history');
const failed = selectionContext.switchSession({ session_id: 'failed', history_loaded: false, messages: [] });
pending.get('failed').reject(new Error('unreadable archive')); await failed;
assert.equal(selectedId, 'newer'); assert.equal(failures.length, 1);
const reopened = selectionContext.switchSession(rows[0]);
pending.get('newer').resolve([{ session_id: 'newer', history_loaded: true, messages: [{ content: 'updated history' }] }]);
await reopened;
assert.equal(selectedMessages[0].content, 'updated history', 'reopening refreshes a cached conversation after new messages');
console.log('Lazy detail hydration, selection races, failed reads and refreshed cached histories passed.');

requestFailure = false;
context.sessionWindow.current = 30;
context.activeIdRef.current = 'session-74';
visible = [{ ...data[74], history_loaded: true, messages: [{ content: 'archive history' }] }];
await context.refreshSessions();
assert.equal(visible.length, 31);
assert.equal(visible.find(s => s.session_id === 'session-74').messages[0].content, 'archive history', 'refresh keeps an active session outside the recent page');
let unwantedSwitches = 0;
deleteContext.activeIdRef.current = 'other-workspace-archive';
deleteContext.switchSession = () => { unwantedSwitches++; };
await deleteContext.deleteWorkspace('project');
assert.equal(unwantedSwitches, 0, 'deleting another workspace does not replace an active archived session outside the page');
console.log('Active archived sessions survive recent-page refresh and unrelated workspace deletion.');
