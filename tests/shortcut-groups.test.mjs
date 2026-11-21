import assert from 'node:assert/strict';
import {
  UNGROUPED, groupShortcuts, shortcutGroupNames, shortcutMatches, showsGroupHeaders,
} from '../src/utils/shortcutGroups.ts';

const item = (id, name, group, target = 'https://example.com') => ({ id, name, kind: 'website', target, group });

// —— 分组归并：首次出现顺序，未分组固定最后 ——
const items = [
  item('a', 'B站', '视频', 'https://bilibili.com'),
  item('b', 'GitHub', '开发', 'https://github.com'),
  item('c', '计算器', '', 'C:\\Windows\\System32\\calc.exe'),
  item('d', 'YouTube', '视频', 'https://youtube.com'),
  item('e', '本地文档', '  开发  ', 'D:\\docs'),
];
assert.deepEqual(groupShortcuts(items).map(s => s.name), ['视频', '开发', UNGROUPED], 'first-seen group order, ungrouped last');
assert.deepEqual(groupShortcuts(items).map(s => s.items.map(i => i.id)), [['a', 'd'], ['b', 'e'], ['c']], 'items keep their order inside a group');
// 首尾空白不应把同一组拆成两组
assert.equal(groupShortcuts(items).find(s => s.name === '开发').items.length, 2, 'group names are trimmed before merging');
assert.deepEqual(groupShortcuts(items).map(s => s.key), ['视频', '开发', '__ungrouped'], 'ungrouped gets a non-empty render key');
// 旧配置没有 group 字段
assert.deepEqual(groupShortcuts([{ id: 'x', name: 'Old', kind: 'website', target: 'https://x.dev' }]).map(s => s.name), [UNGROUPED], 'legacy shortcuts without a group fall into ungrouped');

// —— 搜索：名称 / 目标 / 分组都能命中，多词逐词全中 ——
assert.ok(shortcutMatches(items[0], 'b站'), 'name match is case-insensitive');
assert.ok(shortcutMatches(items[1], 'github.com'), 'target match');
assert.ok(shortcutMatches(items[1], '开发'), 'group match');
assert.ok(shortcutMatches(items[0], '视频 bili'), 'multi-term search requires every term');
assert.equal(shortcutMatches(items[0], '视频 github'), false, 'a missing term drops the row');
assert.ok(shortcutMatches(items[0], '   '), 'blank query matches everything');

const searched = groupShortcuts(items, '视频');
assert.deepEqual(searched.map(s => s.name), ['视频'], 'groups without a hit disappear');
assert.deepEqual(searched[0].items.map(i => i.id), ['a', 'd']);
assert.deepEqual(groupShortcuts(items, 'github').map(s => [s.name, s.items.length]), [['开发', 1]], 'search narrows inside a group');
assert.deepEqual(groupShortcuts(items, '不存在的关键词'), [], 'no hit yields no sections');

// —— 分组头只在真的用了分组时出现 ——
assert.equal(showsGroupHeaders(groupShortcuts([{ id: 'x', name: 'Old', kind: 'website', target: 'https://x.dev' }])), false, 'a single ungrouped section keeps the plain list');
assert.equal(showsGroupHeaders(groupShortcuts([item('a', 'B站', '视频')])), true, 'a named group always shows its header');
assert.equal(showsGroupHeaders(groupShortcuts(items)), true, 'several groups show headers');
assert.equal(showsGroupHeaders([]), false, 'an empty list has no headers');

// —— 表单分组候选：去重、首次出现顺序、忽略空组 ——
assert.deepEqual(shortcutGroupNames(items), ['视频', '开发'], 'suggestions are unique and ordered');
assert.deepEqual(shortcutGroupNames([item('a', 'x', '  ')]), [], 'blank groups are not suggestions');
assert.deepEqual(shortcutGroupNames([]), []);

console.log('Shortcut groups: merge order, ungrouped last, name/target/group search, header gating and suggestions passed');
