import assert from 'node:assert/strict';
import { buildSync } from 'esbuild';
import { findProviderPresetByEndpoint, presetMatches } from '../src/components/settings/providerPresets.ts';

const openai = findProviderPresetByEndpoint('https://API.OPENAI.COM/v1/');
assert.equal(openai?.id, 'openai');
assert.equal(findProviderPresetByEndpoint('https://api.openai.com/v1/responses?stream=true')?.id, 'openai');
assert.equal(findProviderPresetByEndpoint('https://api.deepseek.com/v1')?.id, 'deepseek');
assert.equal(findProviderPresetByEndpoint('https://api.minimaxi.com/anthropic/v1/messages')?.id, 'minimax');
for (const endpoint of ['', 'invalid', 'https://example.com/v1',
  'https://api.openai.com.evil.test/v1', 'https://api.openai.com/v10',
  'http://api.openai.com/v1', 'https://api.openai.com:8080/v1']) {
  assert.equal(findProviderPresetByEndpoint(endpoint), undefined, endpoint);
}
assert.equal(presetMatches(openai, 'chat_completions', 'https://api.openai.com/v1'), true);
assert.equal(presetMatches(openai, 'anthropic', ''), false);

// Shared data must stay usable without pulling in settings UI, React or native IPC.
const bundle = buildSync({ entryPoints: ['src/components/settings/providerPresets.ts'],
  bundle: true, write: false, metafile: true, format: 'esm', platform: 'node' });
assert.deepEqual(Object.keys(bundle.metafile.inputs), ['src/components/settings/providerPresets.ts']);

const appBundle = buildSync({ entryPoints: ['src/App.tsx'], bundle: true, write: false,
  metafile: true, format: 'esm', platform: 'browser', packages: 'external',
  loader: { '.css': 'empty' } });
for (const file of Object.keys(appBundle.metafile.inputs)) {
  assert.ok(!/\/(ConfigWindow|ModelSettings)\.tsx$/.test(file),
    `Pet entry must not eagerly load settings UI: ${file}`);
}
console.log('Provider presets: endpoint boundaries, protocol matching and UI-independent imports passed.');
