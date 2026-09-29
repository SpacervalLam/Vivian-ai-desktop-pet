import assert from 'node:assert/strict';
import { routeAssistantMessage } from '../src/utils/chatMessageRouting.ts';

for (const view of ['home', 'group', 'details']) {
  const route = routeAssistantMessage(view, 'vivian', 'nana', 'wechat');
  assert.equal(route.append, null);
  assert.equal(route.unread, 'nana');
  assert.equal(route.preview, 'nana');
}
const other = routeAssistantMessage('private', 'vivian', 'nana', 'wechat');
assert.equal(other.append, null);
assert.equal(other.unread, 'nana');
assert.equal(other.preview, 'nana');
const own = routeAssistantMessage('private', 'nana', 'nana', 'wechat');
assert.equal(own.append, 'private');
assert.equal(own.unread, undefined);
assert.deepEqual(routeAssistantMessage('group', 'nana', 'nana', 'wechat_group'), { append: 'group' });
assert.deepEqual(routeAssistantMessage('private', 'nana', 'nana', 'wechat_group'), { append: null, unread: 'group' });
assert.deepEqual(routeAssistantMessage('private', 'nana', 'nana', 'proactive'), { append: null });
assert.deepEqual(routeAssistantMessage('group', 'nana', 'nana', 'direct'), { append: null });
console.log('ChatWindow private/group/unread routing: passed');
