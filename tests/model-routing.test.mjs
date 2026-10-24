import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { buildSync } from 'esbuild';

const rawCatalog = JSON.parse(readFileSync('src-tauri/prompts/routing/task_catalog.json', 'utf8'));
const tiers = rawCatalog.tiers;
const catalog = rawCatalog.tasks;
const tasks = new Map(catalog.map((task) => [task.id, task]));
const tierIds = new Set(tiers.map((tier) => tier.id));
const ids = new Set(tasks.keys());

assert.equal(ids.size, catalog.length, 'task ids are unique');
assert.equal(tierIds.size, tiers.length, 'tier ids are unique');
assert.equal(new Set(catalog.map((task) => task.instruction)).size, catalog.length, 'task contracts are distinct');
assert.equal(new Set(tiers.map((tier) => tier.instruction)).size, tiers.length, 'tier descriptions are distinct');

const bundle = buildSync({ entryPoints: ['src/components/settings/ModelSettings.tsx'], bundle: true, write: false, format: 'esm', platform: 'node', external: ['@tauri-apps/*'] });
// Avoid native imports: verify the shared source and evaluate the exact UI mapping.
const ui = readFileSync('src/components/settings/ModelSettings.tsx', 'utf8');
assert.ok(ui.includes("import taskCatalog from '../../../src-tauri/prompts/routing/task_catalog.json'"));
assert.ok(bundle.outputFiles[0].text.includes('query_rewrite'));

// 每一个会给用户看的键都必须在三种语言里各出现一次。
const translations = readFileSync('src/i18n/index.ts', 'utf8');
const expectInAllLanguages = (key, label) => {
  const bare = key.replace('config.', '');
  assert.equal([...translations.matchAll(new RegExp(`\\b${bare}:`, 'g'))].length, 3, `${label}: in all languages`);
};
for (const tier of tiers) {
  expectInAllLanguages(tier.labelKey, tier.id);
  expectInAllLanguages(tier.helpKey, tier.id);
}
for (const task of catalog) {
  expectInAllLanguages(task.labelKey, task.id);
  expectInAllLanguages(task.helpKey, task.id);
  if (task.kind !== 'image') expectInAllLanguages(task.groupKey, task.id);
}

// 档位是模型选择的粒度，任务契约不能脱离档位存在。
for (const task of catalog) {
  if (task.kind === 'image') {
    assert.equal(task.tier, null, `${task.id}: non-LLM tasks must not claim a tier`);
    continue;
  }
  assert.ok(tierIds.has(task.tier), `${task.id}: unknown tier ${task.tier}`);
  if (task.spoken) assert.equal(task.tier, 'dialogue', `${task.id}: only the dialogue tier may speak`);
  if (task.fallback) assert.ok(ids.has(task.fallback), `${task.id}: unknown fallback ${task.fallback}`);
}
for (const tier of tiers) {
  assert.ok(catalog.some((task) => task.tier === tier.id), `${tier.id}: tier ships no task`);
}

// 工作智能体这条产品线必须独占档位与并发组。
const workTierMembers = catalog.filter((task) => task.tier === 'work').map((task) => task.id);
assert.deepEqual(workTierMembers.sort(), ['work_agent', 'work_memory']);
for (const task of catalog) {
  if (task.concurrency === 'work') assert.ok(workTierMembers.includes(task.id), `${task.id}: work concurrency outside the work tier`);
}

let requests = 0;
function inspect(dir) {
  for (const item of readdirSync(dir, { withFileTypes: true })) {
    const file = path.join(dir, item.name);
    if (item.isDirectory()) { inspect(file); continue; }
    if (!file.endsWith('.rs')) continue;
    const source = readFileSync(file, 'utf8').split('#[cfg(test)]')[0];
    for (const match of source.matchAll(/LLMRequest::new\(\s*"([^"]+)"/g)) {
      requests++;
      assert.ok(ids.has(match[1]), `${file}: unknown hardcoded task ${match[1]}`);
    }
  }
}
inspect('src-tauri/src');
assert.ok(requests > 30);

assert.equal(tasks.get('pet_reaction').fallback, 'companion');
assert.equal(tasks.get('query_rewrite').fallback, 'memory');
assert.equal(tasks.get('context_compress').fallback, 'memory');
const evolution = tasks.get('persona_evolution');
assert.equal(evolution.fallback, 'reasoning');
assert.equal(evolution.spoken, false);
assert.equal(evolution.concurrency, 'auxiliary');
assert.equal(evolution.tier, 'cognition');
// 事实核对与工作记忆各自独立立项，不再借用 memory 契约。
assert.equal(tasks.get('faithfulness').fallback, 'simple_judge');
assert.equal(tasks.get('faithfulness').tier, 'judge');
assert.equal(tasks.get('work_memory').fallback, 'work_agent');
assert.equal(tasks.get('work_memory').tier, 'work');
// 已删除的路由不允许复活。
for (const gone of ['chat', 'tool_execution', 'diary_growth']) {
  assert.equal(tasks.has(gone), false, `${gone} should be gone`);
}

// Optional coordination may inherit a model, but must keep its own local request budget.
for (const id of ['environment_judge', 'bystander_judge']) {
  const task = tasks.get(id);
  assert.equal(task.concurrency, 'environment');
  assert.equal(task.fallback, 'simple_judge');
  for (const foreground of ['companion', 'reasoning', 'emotion_analysis', 'intent_judge']) {
    assert.notEqual(task.concurrency, tasks.get(foreground).concurrency);
  }
}
console.log(`Model routing: ${tiers.length} tiers, ${catalog.length} task contracts, multilingual settings, and ${requests} production call sites verified.`);
