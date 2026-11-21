import assert from 'node:assert/strict';
import { build } from 'esbuild';

const bundle = await build({
  entryPoints: ['src/utils/windowLanguage.ts'], bundle: true, write: false, platform: 'node', format: 'esm',
  plugins: [{ name: 'window-language-fixtures', setup(builder) {
    builder.onResolve({ filter: /^@tauri-apps\/api\// }, args => ({ path: args.path, namespace: 'fixture' }));
    builder.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents: path.endsWith('/core')
      ? 'export const invoke=(...args)=>globalThis.__languageFixture.invoke(...args);'
      : 'export const listen=(...args)=>globalThis.__languageFixture.listen(...args);' }));
  } }],
});
const { synchronizeWindowLanguage } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
const flush = () => new Promise(resolve => setImmediate(resolve));

try {
  let callback, resolveConfig, cleanups = 0;
  const applied = [];
  globalThis.__languageFixture = {
    async listen(name, handler) {
      assert.equal(name, 'config:language-changed');
      callback = handler;
      return () => cleanups++;
    },
    invoke(name, args) {
      assert.equal(name, 'get_config');
      assert.deepEqual(args, { key: 'base.language' });
      return new Promise(resolve => { resolveConfig = resolve; });
    },
  };
  const changeLanguage = async language => { applied.push(language); };
  const sync = synchronizeWindowLanguage(changeLanguage);
  await flush();
  resolveConfig('ja');
  await sync.ready;
  assert.deepEqual(applied, ['ja'], 'a new webview reads the saved language');
  callback({ payload: { language: 'en' } });
  callback({ payload: { language: 'zh-CN' } });
  assert.deepEqual(applied, ['ja', 'en', 'zh-CN'], 'the same webview follows language changes');
  sync.dispose();
  callback({ payload: { language: 'en' } });
  assert.equal(applied.length, 3, 'disposed windows ignore events');
  assert.equal(cleanups, 1);

  const racing = synchronizeWindowLanguage(changeLanguage);
  await flush();
  callback({ payload: { language: 'en' } });
  resolveConfig('ja');
  await racing.ready;
  assert.equal(applied.at(-1), 'en', 'a stale config response cannot overwrite a newer language event');
  racing.dispose();

  const closing = synchronizeWindowLanguage(changeLanguage);
  await flush();
  closing.dispose();
  resolveConfig('ja');
  await closing.ready;
  assert.equal(applied.at(-1), 'en', 'config reads cannot update an unmounted window');

  let resolveListen;
  globalThis.__languageFixture.listen = () => new Promise(resolve => { resolveListen = resolve; });
  const pending = synchronizeWindowLanguage(changeLanguage);
  pending.dispose();
  resolveListen(() => cleanups++);
  await pending.ready;
  assert.equal(cleanups, 4, 'a late subscription is immediately released');
  console.log('Window language: saved settings, live switching, stale reads and subscription cleanup passed');
} finally {
  delete globalThis.__languageFixture;
}
