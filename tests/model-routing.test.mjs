import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { buildSync } from 'esbuild';

const catalog = JSON.parse(readFileSync('src-tauri/prompts/routing/task_catalog.json', 'utf8'));
const ids = new Set(catalog.map(task => task.id));
assert.equal(ids.size, catalog.length);
assert.equal(new Set(catalog.map(task => task.instruction)).size, catalog.length);
const bundle = buildSync({ entryPoints: ['src/components/settings/ModelSettings.tsx'], bundle: true, write: false, format: 'esm', platform: 'node', external: ['@tauri-apps/*'] });
// Avoid native imports: verify the shared source and evaluate the exact UI mapping.
const ui = readFileSync('src/components/settings/ModelSettings.tsx', 'utf8');
assert.ok(ui.includes("import taskCatalog from '../../../src-tauri/prompts/routing/task_catalog.json'"));
assert.ok(bundle.outputFiles[0].text.includes('query_rewrite'));
const translations = readFileSync('src/i18n/index.ts', 'utf8');
for (const task of catalog) {
  for (const field of ['labelKey', 'helpKey', 'groupKey']) {
    const key = task[field].replace('config.', '');
    assert.equal([...translations.matchAll(new RegExp(`\\b${key}:`, 'g'))].length, 3, `${task.id}: ${field} in all languages`);
  }
  if (task.fallback) assert.ok(ids.has(task.fallback));
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
assert.equal(catalog.find(task => task.id === 'pet_reaction').fallback, 'companion');
assert.equal(catalog.find(task => task.id === 'query_rewrite').fallback, 'memory');
assert.equal(catalog.find(task => task.id === 'context_compress').fallback, 'memory');
console.log(`Model routing: ${catalog.length} distinct contracts, multilingual settings, and ${requests} production call sites verified.`);
