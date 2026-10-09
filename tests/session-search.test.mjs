import assert from 'node:assert/strict';
import { buildSync } from 'esbuild';

const build = buildSync({ entryPoints: ['src/components/mind-inspector/pages/searchIndex.ts'], bundle: true, write: false, format: 'esm', platform: 'node' });
const { indexSessions, searchSessions } = await import(`data:text/javascript;base64,${Buffer.from(build.outputFiles[0].text).toString('base64')}`);
const session = (id, title, messages, updated_at = 1) => ({ session_id: id, title, working_directory: 'G:/Project', messages, updated_at, status: 'idle' });
const data = indexSessions([
  session('title', '工作页优化', [], 1),
  session('body', '修复布局', [{ role: 'assistant', content: '前置说明。'.repeat(50) + '工作页的输入区需要优化。' }], 3),
  session('tool', '其他任务', [{ role: 'tool_result', content: '工作页' }], 4),
  session('english', 'Review UI', [{ role: 'user', content: 'Please fix the composer' }], 2),
]);
assert.deepEqual(searchSessions(data, '工作页').map(result => result.session.session_id), ['title', 'body'], 'title hits rank first; tool output is excluded');
assert.ok(searchSessions(data, '工作页')[1].snippet.includes('工作页'), 'snippet must contain the hit even late in a long message');
assert.deepEqual(searchSessions(data, '  PROJECT COMPOSER ').map(result => result.session.session_id), ['english'], 'case insensitive multi-term search can span workspace and messages');
assert.equal(searchSessions(data, 'missing').length, 0);
assert.deepEqual(searchSessions(data, '').map(result => result.session.session_id), ['tool', 'body', 'english', 'title']);
assert.equal(searchSessions(indexSessions(Array.from({ length: 60 }, (_, i) => session(String(i), '任务', [], i))), '').length, 50);
assert.equal(searchSessions(indexSessions([]), '').length, 0);
console.log('Session search: title/message/workspace matching, ranking, snippets and result limits passed.');

const archived = { ...session('archived', '旧任务', [], 0), search_excerpt: '早期关键约定：禁止自动发布' };
assert.equal(searchSessions(indexSessions([archived]), '禁止 发布')[0].session.session_id, 'archived');
assert.equal(archived.messages.length, 0, 'search snippets do not need full message histories');
