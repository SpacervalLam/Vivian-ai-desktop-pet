import assert from 'node:assert/strict';
import { buildSync } from 'esbuild';

const result = buildSync({
  entryPoints: ['src/components/mind-inspector/pages/workActivitySummary.ts'],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { collectWorkActivity, formatWorkActivity, workPhaseOf } = await import(
  `data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`
);

const use = (id, name, args = {}) => ({ role: 'tool_use', tool_call_id: id, tool_name: name, tool_arguments: args });
const done = (id, name, success = true) => ({ role: 'tool_result', tool_call_id: id, tool_name: name, tool_success: success });

const read = collectWorkActivity([use('a', 'read_file', { path: 'src/a.ts' }), done('a', 'read_file')]);
assert.equal(read.total, 1, 'one call must not be counted twice after its result arrives');
assert.deepEqual(read.facts, [{ kind: 'read', count: 1, files: 1 }]);
assert.equal(read.failed, 0);

const lateIntent = collectWorkActivity([done('b', 'edit_file'), use('b', 'edit_file', { path: 'src/b.ts' })]);
assert.equal(lateIntent.total, 1, 'late intent must not replace the result record');
assert.equal(lateIntent.facts[0]?.kind, 'edit');

const edits = collectWorkActivity([
  use('c', 'edit_file', { path: 'src/x.ts' }), done('c', 'edit_file'),
  use('d', 'write_file', { path: 'src/y.ts' }), done('d', 'write_file'),
  use('e', 'edit_file', { path: 'src/x.ts' }), done('e', 'edit_file'),
]);
assert.deepEqual(edits.facts, [{ kind: 'edit', count: 3, files: 2 }], 'unique paths drive the file count');

const mixed = collectWorkActivity([
  use('f', 'grep_search', { pattern: 'needle' }), done('f', 'grep_search'),
  use('g', 'list_dir', { directory: 'src' }), done('g', 'list_dir'),
  use('h', 'read_file', { path: 'src/a.ts' }), done('h', 'read_file', false),
  use('i', 'run_command', { command: 'cargo test' }), done('i', 'run_command'),
]);
assert.equal(mixed.total, 4);
assert.equal(mixed.failed, 1);
assert.deepEqual(mixed.facts, [
  { kind: 'read', count: 2, files: 1 },
  { kind: 'search', count: 1, files: 0 },
  { kind: 'run', count: 1, files: 0 },
], 'facts keep the edit → read → search → run order; directories do not count as files');

const globSearch = collectWorkActivity([
  use('g', 'glob', { pattern: '*.rs' }),
  { role: 'tool_result', tool_call_id: 'g', tool_name: 'glob', tool_success: true, content: JSON.stringify({ count: 0, files: [] }) },
]);
assert.deepEqual(globSearch.facts, [{ kind: 'search', count: 1, files: 0 }], 'glob is a search operation');

const anonymous = collectWorkActivity([
  use(null, 'read_file', { path: 'src/a.ts' }), done(null, 'read_file'),
]);
assert.equal(anonymous.total, 2, 'anonymous calls stay distinct, matching message reconciliation');

const t = (key, options = {}) => `${key}:${options.n}`;
assert.deepEqual(formatWorkActivity(edits, t), ['workbench.activity_edit_files:2']);
assert.deepEqual(formatWorkActivity(read, t), ['workbench.activity_read_files:1']);
assert.deepEqual(formatWorkActivity(mixed, t), [
  'workbench.activity_read_files:1',
  'workbench.activity_search_ops:1',
  'workbench.activity_run_ops:1',
]);

assert.equal(workPhaseOf('grep_search'), 'explore');
assert.equal(workPhaseOf('glob'), 'explore');
assert.equal(workPhaseOf('read_file'), 'explore');
assert.equal(workPhaseOf('web_fetch'), 'explore');
assert.equal(workPhaseOf('edit_file'), 'edit');
assert.equal(workPhaseOf('write_file'), 'edit');
assert.equal(workPhaseOf('run_command'), 'verify');
assert.equal(workPhaseOf('send_image'), 'other');

console.log('Work activity summary: dedupe, file counts, failure counts, labels and phases passed.');
