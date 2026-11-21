import assert from 'node:assert/strict';
import { buildSync } from 'esbuild';

const { outputFiles } = buildSync({ entryPoints: ['src/components/mind-inspector/pages/terminalText.ts'], bundle: true, write: false, format: 'esm', platform: 'node' });
const { terminalScreenText, terminalQuote } = await import(`data:text/javascript;base64,${Buffer.from(outputFiles[0].text).toString('base64')}`);
const buffer = rows => ({ length: rows.length, getLine: i => rows[i] && ({
  isWrapped: !!rows[i].wrapped, translateToString: trim => trim ? rows[i].text.trimEnd() : rows[i].text,
}) });
assert.equal(terminalScreenText(buffer([
  { text: 'PS> build' }, { text: '编译🙂' }, { text: '完成', wrapped: true }, { text: '' }, { text: 'PS>' }, { text: '' },
])), 'PS> build\n编译🙂完成\n\nPS>');
assert.equal(terminalScreenText(buffer([])), '');
assert.equal(terminalScreenText(buffer([{ text: 'hello ' }, { text: 'world', wrapped: true }])), 'hello world', 'spaces at wrap boundaries remain significant');
const long = terminalScreenText(buffer(Array.from({ length: 5000 }, (_, i) => ({ text: `${i} ${'x'.repeat(96)}` }))));
assert.ok(long.length < 40_200, 'large scrollback is bounded');
assert.ok(long.endsWith(`4999 ${'x'.repeat(96)}`), 'newest output remains available');
const quote = terminalQuote('终端', 'G:\\项目', 'term-123', 'error\r\n```\r\n> detail\r\n');
assert.equal(quote, '\n\n终端 · G:\\项目 · term-123\n> error\n> ```\n> > detail\n> \n\n');
console.log('Terminal sharing: wrapped rows, Unicode, blank lines, bounded scrollback and quoted source passed.');
