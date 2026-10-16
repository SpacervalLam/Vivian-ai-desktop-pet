import assert from 'node:assert/strict';
import { buildSync } from 'esbuild';

const bundle = buildSync({
  entryPoints: ['src/utils/openWebsite.ts'], bundle: true, write: false, format: 'esm', platform: 'node',
  external: ['@tauri-apps/api/core', '@tauri-apps/plugin-shell'],
});
// Replace the two transport imports, keeping the real routing implementation.
const source = bundle.outputFiles[0].text
  .replace(/import \{ invoke \} from "@tauri-apps\/api\/core";/, 'const invoke = (...args) => globalThis.websiteTransport.invoke(...args);')
  .replace(/import \{ open \} from "@tauri-apps\/plugin-shell";/, 'const open = (...args) => globalThis.websiteTransport.open(...args);');
const { openWebsite } = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
const calls = [];
let embeddedFailure = false;
let shellFailure = false;
globalThis.websiteTransport = {
  async invoke(...args) { calls.push(['embedded', ...args]); if (embeddedFailure) throw new Error('WebView2 unavailable'); },
  async open(url) { calls.push(['system', url]); if (shellFailure) throw new Error('System browser unavailable'); },
};
await openWebsite(' HTTPS://example.com/a?q=1#part ');
assert.deepEqual(calls.splice(0), [['embedded', 'open_embedded_browser', { url: 'HTTPS://example.com/a?q=1#part' }]]);
embeddedFailure = true;
await openWebsite('https://example.com/');
assert.deepEqual(calls.splice(0), [['embedded', 'open_embedded_browser', { url: 'https://example.com/' }], ['system', 'https://example.com/']]);
await openWebsite('mailto:hello@example.com');
assert.deepEqual(calls.splice(0), [['system', 'mailto:hello@example.com']]);
shellFailure = true;
await assert.rejects(() => openWebsite('https://example.com/'), /System browser unavailable/);
delete globalThis.websiteTransport;
console.log('Website opening: embedded first, system fallback only on failure, existing non-web handlers and final errors preserved.');
