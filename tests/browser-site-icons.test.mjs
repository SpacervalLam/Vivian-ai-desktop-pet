import assert from 'node:assert/strict';
import { build } from 'esbuild';

const calls = [];
let respond;
globalThis.__browserIconInvoke = (name, args) => {
  calls.push({ name, args });
  return respond(name, args);
};
const bundle = await build({
  entryPoints: ['src/components/mind-inspector/pages/browserSiteIcons.ts'], bundle: true,
  write: false, format: 'esm', platform: 'node',
  plugins: [{ name: 'native-icon-fixture', setup(build) {
    build.onResolve({ filter: /^@tauri-apps\/api\/core$/ }, () => ({ path: 'native', namespace: 'fixture' }));
    build.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: 'export const invoke=(...args)=>globalThis.__browserIconInvoke(...args);' }));
  } }],
});
const { browserSiteOrigin, browserSiteIcon } = await import('data:text/javascript;base64,' + Buffer.from(bundle.outputFiles[0].text).toString('base64'));
assert.equal(browserSiteOrigin('https://example.com/private/page?token=secret#fragment'), 'https://example.com');
assert.equal(browserSiteOrigin('http://localhost:3000/page'), 'http://localhost:3000');
for (const value of ['about:blank', 'file:///G:/page.html', 'javascript:alert(1)', 'https://user:password@example.com', 'invalid']) assert.equal(browserSiteOrigin(value), null);

const originalNow = Date.now;
let clock = 100;
Date.now = () => clock;
try {
  let resolve;
  respond = () => new Promise(done => { resolve = done; });
  const first = browserSiteIcon('https://example.com');
  assert.equal(browserSiteIcon('https://example.com'), first, 'parallel UI instances must share one pending request');
  assert.deepEqual(calls, [{ name: 'browser_site_icon', args: { url: 'https://example.com' } }]);
  resolve('data:image/png;base64,fixture');
  assert.equal(await first, 'data:image/png;base64,fixture');
  assert.equal(await browserSiteIcon('https://example.com'), 'data:image/png;base64,fixture');
  assert.equal(calls.length, 1);

  respond = async () => { throw Error('offline'); };
  assert.equal(await browserSiteIcon('https://offline.example'), null);
  assert.equal(await browserSiteIcon('https://offline.example'), null);
  assert.equal(calls.length, 2, 'failed sites must not trigger a retry storm');
  clock += 30_001;
  respond = async () => 'data:image/x-icon;base64,recovered';
  assert.equal(await browserSiteIcon('https://offline.example'), 'data:image/x-icon;base64,recovered');
  assert.equal(calls.length, 3, 'temporary failures must be retryable');
  respond = async () => 'https://third-party.example/icon.png';
  assert.equal(await browserSiteIcon('https://invalid.example'), null, 'only backend image data is displayed');
} finally { Date.now = originalNow; delete globalThis.__browserIconInvoke; }
console.log('Browser icons: origin-only requests, pending/success deduplication, failure recovery and image data validation passed.');
