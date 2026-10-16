import assert from 'node:assert/strict';
import { test } from 'node:test';
import { build } from 'esbuild';

const bundle = await build({ entryPoints: ['src/components/connections/mcpConfig.ts'], bundle: true, write: false, platform: 'node', format: 'esm' });
const { parseMcpArguments, parseMcpMap } = await import('data:text/javascript;base64,' + Buffer.from(bundle.outputFiles[0].text).toString('base64'));

test('quoted Windows paths, empty values and JSON argument arrays are lossless', () => {
  assert.deepEqual(parseMcpArguments('-y server "G:\\My Files" ""'), ['-y', 'server', 'G:\\My Files', '']);
  assert.deepEqual(parseMcpArguments("'C:\\My Files\\script.js' --flag"), ['C:\\My Files\\script.js', '--flag']);
  const args = ['C:\\My Files\\', 'a"b', '', 'x\ny'];
  assert.deepEqual(parseMcpArguments(JSON.stringify(args)), args);
  assert.deepEqual(parseMcpArguments(''), []);
  assert.throws(() => parseMcpArguments('"unfinished'), /quote/);
  assert.throws(() => parseMcpArguments('[1]'), /strings/);
});

test('environment and HTTP headers preserve secrets, equals signs and empty strings', () => {
  assert.deepEqual(parseMcpMap('TOKEN=a=b==\r\nEMPTY=\r\n# comment\r\nAuthorization=Bearer example'), { TOKEN: 'a=b==', EMPTY: '', Authorization: 'Bearer example' });
  assert.deepEqual(parseMcpMap('{"PATH":"G:\\\\My Files","TOKEN":""}'), { PATH: 'G:\\My Files', TOKEN: '' });
  assert.deepEqual(parseMcpMap(''), {});
  assert.throws(() => parseMcpMap('{"TOKEN":1}'), /string/);
  assert.throws(() => parseMcpMap('bad line'), /KEY=VALUE/);
});
