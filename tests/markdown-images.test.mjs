import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { buildSync } from 'esbuild';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

globalThis.window = { __TAURI_INTERNALS__: { convertFileSrc: path => `asset://localhost/${encodeURIComponent(path)}` } };
const bundled = buildSync({
  stdin: { contents: `export * from './src/components/mind-inspector/pages/markdownImages'; export * from './src/components/mind-inspector/pages/markdownLiveHtml'; export * from './src/components/mind-inspector/pages/codeMarkdown';`, resolveDir: process.cwd(), loader: 'ts' },
  bundle: true, write: false, format: 'cjs', platform: 'node', external: ['react', 'lucide-react'],
});
const mod = { exports: {} };
new Function('require', 'module', 'exports', bundled.outputFiles[0].text)(createRequire(import.meta.url), mod, mod.exports);
const { parseMarkdownImage, markdownImageSrc, inlineHtml, docHtml, MarkdownText, MarkdownFileContext } = mod.exports;
for (const source of [
  '![RRSI 原论文 Figure 2](C:/Users/86139/Documents/outputs/RRSI-Figure2.png)',
  String.raw`![图示]\(C:/My Figures/figure(2).png\)`,
  '![图示](<C:/My Figures/figure(2).png> "Figure 2")',
  '![图示](images/figure.png)',
]) {
  const parsed = parseMarkdownImage(source);
  assert.equal(parsed.raw, source);
  const html = inlineHtml(source, undefined, 'C:/docs/report.md');
  assert.match(html, /<img class="codex-md-image"/);
  assert.match(html, /data-md-ui="1"/);
  const hiddenSource = html.match(/^<span class="md-mark">([\s\S]*?)<\/span>/)[1];
  assert.equal(hiddenSource.replace(/&quot;/g, '"').replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>'), source, 'editable preview preserves the source');
  const reactHtml = renderToStaticMarkup(React.createElement(MarkdownFileContext.Provider, { value: { documentPath: 'C:/docs/report.md' } }, React.createElement(MarkdownText, { text: source })));
  assert.match(reactHtml, /<img /, 'read-only rendering also displays images');
}
assert.equal(markdownImageSrc('images/a.png', 'C:/docs/report.md'), 'asset://localhost/C%3A%2Fdocs%2Fimages%2Fa.png');
assert.equal(markdownImageSrc('file:///C:/docs/a.png'), 'asset://localhost/C%3A%2Fdocs%2Fa.png');
assert.equal(markdownImageSrc('https://example.com/a.png'), 'https://example.com/a.png');
assert.equal(markdownImageSrc('javascript:alert(1)'), null);
assert.equal(markdownImageSrc('data:text/html;base64,abc'), null);
assert.equal(parseMarkdownImage('![unfinished](a.png'), null);
assert.doesNotMatch(inlineHtml('`![code](a.png)`', undefined, 'C:/docs/a.md'), /<img /);
assert.match(docHtml('- ![nested](images/a.png)', undefined, 'C:/docs/a.md'), /<img /);
const multiline = '**作者与机构：** Peng Xia 等。 &#x20;\n**发表时间与出处：** 2026 年。 &#x20;\n**一句话概括：** 约束自我改进。';
assert.equal((docHtml(multiline).match(/<br /g) ?? []).length, 2, 'preview preserves paragraph source line breaks');
assert.equal((renderToStaticMarkup(React.createElement(MarkdownText, { text: multiline })).match(/<br/g) ?? []).length, 2, 'read-only Markdown matches editable line breaks');
assert.doesNotMatch(renderToStaticMarkup(React.createElement(MarkdownText, { text: multiline })), /&amp;#x20;/, 'encoded spaces render as spaces');
assert.match(docHtml(multiline), /&amp;#x20;/, 'editable preview retains the original entity in source markers');
console.log('Markdown images: local/relative paths, both renderers, editable source preservation and unsafe URL rejection passed.');
