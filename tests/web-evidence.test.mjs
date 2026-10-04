import assert from 'node:assert/strict';
import { buildSync } from 'esbuild';
const bundle = buildSync({ entryPoints: ['src/components/mind-inspector/pages/webEvidence.ts'], bundle: true, write: false, format: 'esm', platform: 'node' });
const { parseWebEvidence } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
const output = parseWebEvidence(JSON.stringify({ data: { queries: [
  { results: [{ url: 'https://example.com/a', title: 'A', snippet: 'evidence', engines: ['exa'] }] },
  { results: [{ url: 'https://example.com/a' }, { url: 'javascript:alert(1)' }], error: 'One provider timed out' }
] } }));
assert.equal(output.sources.length, 1);
assert.deepEqual(output.warnings, ['One provider timed out']);
assert.equal(output.sources[0].title, 'A');
const fetched = parseWebEvidence(JSON.stringify({ url: 'https://example.com/a', text: 'original', focused_summary: 'summary', artifact: { text_path: 'C:/evidence/a.md' }, next_offset: 6000 }));
assert.equal(fetched.summary, 'summary');
assert.equal(fetched.text, 'original');
assert.equal(fetched.nextOffset, 6000);
assert.equal(parseWebEvidence('{invalid'), null);
assert.equal(parseWebEvidence(JSON.stringify({ results: [{ url: 'https://user:secret@example.com' }] })), null);
assert.deepEqual(parseWebEvidence(JSON.stringify({ success: false, message: 'Fetch failed: timeout', data: { url: 'https://example.com' } })).warnings, ['Fetch failed: timeout']);
console.log('Web evidence: source deduplication, partial failure, unsafe URL rejection and saved document rendering passed.');
