import assert from 'node:assert/strict';
import { buildSync } from 'esbuild';

const result = buildSync({ entryPoints: ['src/components/mind-inspector/pages/workbenchLayout.ts'], bundle: true, write: false, format: 'esm', platform: 'node' });
const { workbenchLayout, reconcileToolMessages, groupWorkMessages } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);

let cases = 0;
for (const width of [320, 480, 640, 768, 960, 1180, 1440, 1920]) {
  for (const left of [180, 268, 460]) for (const right of [260, 360, 720]) {
    for (const leftClosed of [false, true]) for (const rightClosed of [false, true]) for (const focus of [false, true]) {
      const layout = workbenchLayout(width, left, right, leftClosed, rightClosed, focus);
      assert.ok(layout.centralWidth >= Math.min(540, width - 60), JSON.stringify({ width, left, right, layout }));
      if (layout.rightDrawer) assert.ok(layout.rightWidth <= width - 20);
      if (focus) assert.ok(layout.leftCollapsed && layout.rightCollapsed && !layout.rightDrawer);
      cases++;
    }
  }
}
assert.equal(workbenchLayout(960, 268, 360, false, false, false).rightDrawer, true);
assert.equal(workbenchLayout(1440, 268, 360, false, false, false).rightDrawer, false);
const intent = { role: 'tool_use', tool_call_id: 'a', tool_name: 'read_file', tool_arguments: { path: 'app.ts' } };
const outcome = { role: 'tool_result', tool_call_id: 'a', tool_name: 'read_file', content: 'source', tool_success: true };
assert.deepEqual(reconcileToolMessages([intent, outcome]), [{ ...intent, ...outcome }]);
assert.equal(reconcileToolMessages([outcome, intent])[0].role, 'tool_result', 'late intent must not erase a result');
assert.equal(reconcileToolMessages([intent, { ...intent, tool_call_id: 'b' }, outcome]).length, 2);
assert.equal(reconcileToolMessages([{ role: 'tool_use', tool_arguments: [] }, outcome]).length, 1, 'aggregate intent is not a visible step');
assert.equal(reconcileToolMessages([{ ...intent, tool_call_id: null }, { ...intent, tool_call_id: null }]).length, 2, 'anonymous calls remain distinct');
assert.equal(reconcileToolMessages([intent, { ...outcome, tool_name: null }])[0].tool_name, 'read_file', 'legacy results inherit the intent name');
assert.deepEqual(reconcileToolMessages([intent, { ...intent, tool_call_id: 'b' }, outcome, { ...outcome, tool_call_id: 'b' }]).map(row => row.tool_call_id), ['a', 'b'], 'parallel completion preserves invocation order');
console.log(`Workbench: ${cases} width/panel combinations and tool reconciliation checks passed.`);

// Process prose must survive tool completion and session reload without becoming a final reply.
const thought = { role: 'thinking', content: '检查工作目录与错误原因' };
const commentary = { role: 'commentary', content: '先读取配置，然后修复构建脚本。' };
const user = { role: 'user', content: '修复项目' };
const live = groupWorkMessages([user, thought, commentary, intent], true);
assert.equal(live.length, 2);
assert.deepEqual(live[1].msgs, [thought, commentary, intent]);
assert.equal(live[1].settled, false);
const final = { role: 'assistant', content: '修复完成' };
for (const running of [true, false]) {
  const history = [user, thought, intent, outcome, { role: 'notice', content: '历史消息已达 63 条，已自动把 23 条早期历史压缩为摘要。' }, commentary, { role: 'tool_use', content: '调用工具：read_file' }, final];
  const restored = groupWorkMessages(JSON.parse(JSON.stringify(history)), running);
  assert.equal(restored.length, 3);
  assert.equal(restored[1].settled, true);
  assert.deepEqual(restored[1].msgs, [thought, { ...intent, ...outcome }, commentary]);
  assert.deepEqual(restored[2].msg, final);
}
assert.equal(groupWorkMessages([thought, commentary, intent, outcome], false)[0].settled, true, 'summary phase collapses the entire process before final text');
assert.equal(groupWorkMessages([thought], false)[0].settled, true, 'text-only processes also collapse');
console.log('Work process groups include prose and hide host diagnostics across reloads.');
