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

const insightsBundle = buildSync({ entryPoints: ['src/components/usage/usageInsightsData.ts'], bundle: true, format: 'esm', platform: 'node', write: false });
const { reportForDays, usageInsights, inputComposition, heatmapCells } = await import(`data:text/javascript;base64,${Buffer.from(insightsBundle.outputFiles[0].text).toString('base64')}`);
const filtered = reportForDays(report, [report.days[1]]);
for (const dimension of ['models', 'routes', 'tasks']) assert.deepEqual(sumUsage(dimensionRows(filtered, dimension, 'tokens')), sumUsage([primary, fallback]), 'Date filters update all dimensions together');
assert.deepEqual(sumUsage(selectedDays(filtered, { dimension: 'route-model', key: 'chat', model: 'fallback' })), sumUsage([fallback]));
assert.equal(filtered.routes[0].models.length, 2, 'Actual fallback models survive date filtering');
const oldOnly = reportForDays(report, [report.days[0]]);
assert.equal(dimensionRows(oldOnly, 'routes', 'tokens')[0].key, LEGACY_KEY);
assert.equal(report.days.length, 2, 'Filtering never mutates the source report');
assert.equal(reportForDays(report, []).models.length, 0);
const cache = inputComposition(primary);
assert.equal(cache.total, 115);
assert.equal(cache.hitRate, 100 / 115, 'Cache creation belongs in the input denominator; output does not');
assert.equal(inputComposition({}).hitRate, null, 'Missing input is not shown as a measured zero cache rate');
const stats = usageInsights(report.days, 'tokens');
assert.equal(stats.activeDays, 2);
assert.equal(stats.perCall, tokenTotal(sumUsage(report.days)) / 5);
assert.equal(stats.dailyAverage, tokenTotal(sumUsage(report.days)) / 2);
assert.equal(usageInsights([], 'tokens').peak, null);
assert.equal(usageInsights(report.days, 'requests').peak.date, '2026-09-29');
const calendar = heatmapCells(report.days, 'tokens');
assert.equal(calendar.padding, 1, 'ISO date weekday is aligned with a Monday-first calendar');
assert.equal(calendar.cells.length, 2);
assert.ok(calendar.cells.every(cell => cell.level >= 1 && cell.level <= 4));
assert.equal(heatmapCells([{ date: '2026-10-16', input: 0, output: 0, requests: 0 }], 'tokens').cells[0].level, 0);
assert.equal(heatmapCells(Array.from({ length: 70 }, (_, i) => ({ date: `2026-09-${String(i % 28 + 1).padStart(2, '0')}`, requests: i, ...{ input: i, output: 0, hit: 0, cache_creation: 0 } })), 'tokens').cells.length, 56);
console.log('Usage insights: date-filter conservation, cache denominators, averages, peaks and eight-week calendar passed.');
