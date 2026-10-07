import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { buildSync } from 'esbuild';

const bundle = buildSync({ entryPoints: ['src/components/usage/usageData.ts'], bundle: true, format: 'esm', platform: 'node', write: false });
const { sumUsage, tokenTotal, dimensionRows, selectedDays, LEGACY_KEY, routeLabelKeys } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
const catalog = JSON.parse(readFileSync('src-tauri/prompts/routing/task_catalog.json', 'utf8'));
assert.deepEqual(routeLabelKeys, Object.fromEntries(catalog.map(({ id, labelKey }) => [id, labelKey])), 'Every task route uses the shared translated label, including newly added routes');
const primary = { input: 10, output: 2, hit: 100, cache_creation: 5, requests: 1, model: 'primary' };
const fallback = { input: 20, output: 3, hit: 40, cache_creation: 6, requests: 1, model: 'fallback' };
const chat = { ...sumUsage([primary, fallback]), route: 'chat', models: [primary, fallback] };
const purpose = { ...sumUsage([primary, fallback]), task: 'proactive_message' };
const legacy = { input: 80, output: 7, hit: 0, cache_creation: 0, requests: 3 };
const report = {
  days: [
    { date: '2026-09-29', ...legacy, models: [{ ...legacy, model: 'primary' }], tasks: [], routes: [] },
    { date: '2026-09-30', ...sumUsage([primary, fallback]), models: [primary, fallback], tasks: [purpose], routes: [chat] },
  ],
  models: [{ ...sumUsage([legacy, primary]), model: 'primary' }, fallback],
  tasks: [purpose], routes: [chat],
};
assert.equal(tokenTotal(primary), 117, 'Cache reads and cache writes both contribute to token totals');
for (const dimension of ['models', 'routes', 'tasks']) {
  for (const metric of ['tokens', 'requests']) {
    const rows = dimensionRows(report, dimension, metric);
    assert.deepEqual(sumUsage(rows), sumUsage(report.days), `${dimension}: every stored token and call stays in the report`);
  }
}
const historical = dimensionRows(report, 'routes', 'tokens').find((row) => row.key === LEGACY_KEY);
assert.deepEqual(sumUsage([historical]), legacy, 'Historical usage is not inferred from current routing settings');
const modelDays = selectedDays(report, { dimension: 'models', key: 'primary' });
assert.deepEqual(sumUsage(modelDays), sumUsage([legacy, primary]));
const pairDays = selectedDays(report, { dimension: 'route-model', key: 'chat', model: 'fallback' });
assert.equal(pairDays[0].requests, 0, 'No fake route-model attribution for old records');
assert.deepEqual(sumUsage(pairDays), sumUsage([fallback]));
assert.deepEqual(sumUsage(selectedDays(report, { dimension: 'routes', key: LEGACY_KEY })), legacy);
assert.deepEqual(sumUsage(selectedDays(report, { dimension: 'tasks', key: 'proactive_message' })), sumUsage([primary, fallback]));
assert.deepEqual(dimensionRows({ days: [], models: [], tasks: [], routes: [] }, 'routes', 'tokens'), []);
assert.equal(dimensionRows({ ...report, routes: undefined }, 'routes', 'tokens')[0].key, LEGACY_KEY);
assert.equal(sumUsage([{ input: 3, output: 2, hit: 0, requests: 1 }]).cache_creation, 0, 'Old records without cache creation remain compatible');
console.log('Usage dashboard: cache totals, dimension conservation, daily drill-down and historical attribution passed.');
