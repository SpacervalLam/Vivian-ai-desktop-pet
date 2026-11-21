import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const source = await readFile('src/components/ChatWindow.tsx', 'utf8');
const body = source.match(/const handleVisible = \(\) => \{([\s\S]*?)\n    \};/)?.[1];
assert.ok(body, 'ChatWindow visibility handler exists');
const events = [];
const closing = { current: false };
const viewing = { current: 'nana' };
const element = {
  getAnimations: () => [{ cancel: () => events.push('cancel-exit') }],
  animate: () => { throw new Error('Focus must not animate the root'); },
};
const focus = new Function('viewingRef', 'markConversationRead', 'refreshLastPreviews', 'isClosingRef', 'setIsClosing', 'rootRef', body)
  .bind(null, viewing, id => events.push(`read:${id}`), () => events.push('refresh'), closing, value => events.push(`closing:${value}`), { current: element });
focus();
assert.deepEqual(events, ['read:nana', 'refresh'], 'normal focus only updates read state and previews');
events.length = 0;
closing.current = true;
focus();
assert.equal(closing.current, false);
assert.deepEqual(events, ['read:nana', 'refresh', 'closing:false', 'cancel-exit'], 'reopening cancels the closing animation without bouncing');
events.length = 0;
focus();
assert.deepEqual(events, ['read:nana', 'refresh'], 'repeated focus does not replay presentation');
events.length = 0;
viewing.current = null;
focus();
assert.deepEqual(events, ['refresh'], 'home view refreshes without marking another conversation read');
console.log('ChatWindow focus: stable presentation, close/reopen recovery and read refresh passed');

const closeBody = source.match(/const closeWindow = useCallback\(async \(\) => \{([\s\S]*?)\n  \}, \[\]\);/)?.[1];
assert.ok(closeBody, 'ChatWindow delegates closure to the native animation');
const closingState = { current: false }, changes = [], nativeCalls = [];
let completeNativeExit;
const nativeExit = new Promise(resolve => { completeNativeExit = resolve; });
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
const close = new AsyncFunction('isClosingRef', 'setIsClosing', 'invoke', closeBody).bind(
  null, closingState, value => changes.push(value),
  async name => { nativeCalls.push(name); await nativeExit; },
);
const pendingClose = close();
assert.equal(closingState.current, true);
await close();
assert.deepEqual(nativeCalls, ['close_chat_window'], 'repeated back presses do not restart native exit');
assert.deepEqual(changes, [true], 'closing remains active until native exit completes');
completeNativeExit();
await pendingClose;
assert.equal(closingState.current, false);
assert.deepEqual(changes, [true, false]);
console.log('ChatWindow back: awaits native mirrored exit and suppresses repeated close requests passed');
