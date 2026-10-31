// Mobile work process timeline: tool reconciliation, path folding, noise merging.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('../src-tauri/src/remote/frontend/work-process.js', import.meta.url), 'utf8');
const sandbox = { window: {} };
vm.createContext(sandbox);
vm.runInContext(source, sandbox, { filename: 'work-process.js' });
const WorkProcess = sandbox.window.WorkProcess;
assert(WorkProcess, 'work-process.js must expose window.WorkProcess');

const use = (id, name, args = {}) => ({ role: 'tool_use', tool_call_id: id, tool_name: name, tool_arguments: args });
const batchUse = calls => ({ role: 'tool_use', content: '', tool_arguments: calls.map(([id, name, args]) => ({ id, name, arguments: args })) });
const done = (id, name, content = '', success = true) => ({ role: 'tool_result', tool_call_id: id, tool_name: name, content, tool_success: success });

const paired = WorkProcess.build([
  use('a', 'read_file', { path: 'src/a.ts' }),
  done('a', 'read_file', JSON.stringify({ total_lines: 12 })),
], 'G:\\vivian-rs', 'idle');
assert.equal(paired.calls.length, 1, 'tool_use + tool_result must merge into one call');
assert.equal(paired.items.filter(item => item.type === 'tool').length, 1);
assert.equal(paired.steps, 1);
assert.equal(paired.calls[0].path, 'src/a.ts');
assert.equal(paired.calls[0].argsText, 'src/a.ts');
assert.equal(paired.calls[0].info.meta, '12 行');
assert.deepEqual(JSON.parse(JSON.stringify(paired.summary.facts[0])), { kind: 'read', count: 1, files: 1 });

// Real agent messages: one tool_use carries the whole batch of calls.
const batch = WorkProcess.build([
  batchUse([
    ['x', 'grep_search', { pattern: 'needle' }],
    ['y', 'read_file', { path: 'src/b.ts' }],
  ]),
  done('x', 'grep_search', JSON.stringify({ matches: [{ file: 'src/c.ts' }, { file: 'src/c.ts' }], total: 3 })),
  done('y', 'read_file', JSON.stringify({ total_lines: 40 })),
], 'G:\\vivian-rs', 'idle');
assert.equal(batch.calls.length, 2, 'batch tool_use must register every call');
assert.equal(batch.calls[0].argsText, 'needle');
assert.equal(batch.calls[0].info.meta, '找到 3 处匹配');
assert.equal(batch.calls[0].info.files.length, 1, 'duplicate match files must be deduplicated');
assert.equal(batch.calls[1].info.meta, '40 行');

// Results whose intent fell outside the 50-message window still carry arguments.
const resultOnly = WorkProcess.build([
  done('z', 'read_file', JSON.stringify({ total_lines: 7 }), true),
], 'G:\\vivian-rs', 'idle');
assert.equal(resultOnly.calls.length, 1);
assert.equal(resultOnly.calls[0].kind, 'read');

assert.equal(WorkProcess.relativePath('G:\\vivian-rs\\src\\a.ts', 'G:\\vivian-rs'), 'src/a.ts');
assert.equal(WorkProcess.relativePath('C:\\elsewhere\\a.ts', 'G:\\vivian-rs'), 'C:\\elsewhere\\a.ts');
const longPath = WorkProcess.displayPath(
  'C:\\Users\\someone\\documents\\projects\\vivian\\src-tauri\\src\\remote\\frontend\\work-process.js',
  'G:\\vivian-rs',
);
assert(longPath.includes('…'), 'long absolute paths must be folded in the middle');
assert(longPath.length <= 46, 'folded paths must fit a phone-width row');

const emptyMessages = [0, 1, 2].flatMap(index => [
  use('s' + index, 'grep_search', { pattern: 'needle' }),
  done('s' + index, 'grep_search', JSON.stringify({ matches: [], total: 0 })),
]);
const empty = WorkProcess.build(emptyMessages, '', 'idle');
const emptyRows = empty.items.filter(item => item.type === 'tool');
assert.equal(emptyRows.length, 1, 'consecutive empty searches must collapse into one row');
assert.equal(emptyRows[0].emptyCount, 3);
assert.equal(emptyRows[0].weight, 3);
assert.equal(empty.calls.length, 3, 'summary counts must keep the real call count');
assert.equal(empty.emptyTotal, 3);
assert.equal(empty.emptyMerged, 1);

const phased = WorkProcess.build([
  use('p1', 'grep_search', { pattern: 'a' }),
  done('p1', 'grep_search', JSON.stringify({ matches: [], total: 0 })),
  use('p2', 'read_file', { path: 'src/a.ts' }),
  done('p2', 'read_file', JSON.stringify({ total_lines: 3 })),
  use('p3', 'edit_file', { path: 'src/a.ts' }),
  done('p3', 'edit_file', JSON.stringify({ replacements: 2 })),
  use('p4', 'run_command', { command: 'cargo test' }),
  done('p4', 'run_command', JSON.stringify({ exit_code: 0 })),
], 'G:\\vivian-rs', 'idle');
const phases = phased.items.filter(item => item.type === 'phase');
assert.deepEqual(JSON.parse(JSON.stringify(phases.map(item => item.phase))), ['explore', 'edit', 'verify']);
assert.equal(phases[0].count, 2, 'invisible merged rows still count toward the phase');
assert.equal(phases[0].files, 1);
assert.equal(phases[1].files, 1);
assert.equal(phases[2].count, 1);

const failing = WorkProcess.build([
  use('f', 'run_command', { command: 'cargo test' }),
  done('f', 'run_command', JSON.stringify({ exit_code: 2 })),
], '', 'idle');
assert.equal(failing.counts.failed, 1);
assert.equal(failing.calls[0].info.state, 'failed');
assert.equal(failing.calls[0].info.meta, '退出码 2');
assert.equal(WorkProcess.summaryText(failing.summary), '执行 1 条命令');

const edits = WorkProcess.build([
  use('e1', 'edit_file', { path: 'src/x.ts' }), done('e1', 'edit_file', JSON.stringify({ replacements: 1 })),
  use('e2', 'write_file', { path: 'src/y.ts' }), done('e2', 'write_file', JSON.stringify({ total_lines: 9 })),
  use('e3', 'edit_file', { path: 'src/x.ts' }), done('e3', 'edit_file', JSON.stringify({ replacements: 1 })),
  use('e4', 'run_command', { command: 'cargo test' }), done('e4', 'run_command', JSON.stringify({ exit_code: 0 })),
], '', 'idle');
assert.equal(WorkProcess.summaryText(edits.summary), '编辑 2 个文件 · 执行 1 条命令');

const legacy = WorkProcess.build([
  { role: 'tool', tool_name: 'read_file', tool_arguments: { path: 'src/legacy.ts' }, content: '<script>bad()</script>', tool_success: true },
], 'G:\\vivian-rs', 'idle');
assert.equal(legacy.calls.length, 1, 'legacy role:tool messages stay supported');
assert.equal(legacy.calls[0].path, 'src/legacy.ts');

const thinking = WorkProcess.build([
  { role: 'thinking', content: '先看看目录结构。' },
  { role: 'commentary', content: '我准备读取入口文件。' },
  use('t1', 'read_file', { path: 'src/main.ts' }),
], '', 'running');
assert.equal(thinking.counts.thinking, 1);
assert.equal(thinking.running, true);
assert(thinking.items.some(item => item.type === 'think'));
assert(thinking.items.some(item => item.type === 'say'));
assert.equal(thinking.items.find(item => item.type === 'tool').call.info.state, 'pending');

assert.equal(WorkProcess.activityKind('glob'), 'search');
assert.equal(WorkProcess.activityKind('send_image'), 'image');
assert.equal(WorkProcess.workPhaseOf('glob'), 'explore');
assert.equal(WorkProcess.workPhaseOf('edit_file'), 'edit');
assert.equal(WorkProcess.workPhaseOf('run_command'), 'verify');
assert.equal(WorkProcess.workPhaseOf('send_image'), 'other');
assert.equal(WorkProcess.toolLabel('glob'), '查找文件');
assert.equal(WorkProcess.phaseLabel('verify'), '验证');

console.log('Work process timeline: merge, batch calls, path folding, empty-search collapse, phases and failures passed.');
