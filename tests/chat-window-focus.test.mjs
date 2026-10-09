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
