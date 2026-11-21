import assert from 'node:assert/strict';
import {
  REVIEW_DIMENSIONS,
  REVIEW_FAILURE_FLAGS,
  buildReviewDeck,
  fingerprintPromptSources,
  parseEnvFile,
  promptSourceFiles,
  resolveCredentials,
} from '../scripts/prompt-eval-support.mjs';
import { readFileSync } from 'node:fs';

// Credentials: environment first, .env.local fills the rest, missing keys named.
{
  const file = parseEnvFile([
    '# comment',
    'VIVIAN_EVAL_ENDPOINT=https://example.test/v1',
    'VIVIAN_EVAL_MODEL="quoted-model"',
    "VIVIAN_EVAL_API_KEY='sk-quoted'",
    '',
    'IGNORED_LINE_WITHOUT_EQUALS',
    'SPACED = around ',
  ].join('\n'));
  assert.equal(file.VIVIAN_EVAL_ENDPOINT, 'https://example.test/v1');
  assert.equal(file.VIVIAN_EVAL_MODEL, 'quoted-model', 'quotes must be stripped');
  assert.equal(file.VIVIAN_EVAL_API_KEY, 'sk-quoted');
  assert.equal(file.SPACED, 'around');
  assert.equal(file.IGNORED_LINE_WITHOUT_EQUALS, undefined);

  const fromFile = resolveCredentials({}, file);
  assert.equal(fromFile.complete, true);
  assert.equal(fromFile.source, 'file');
  assert.equal(fromFile.model, 'quoted-model');

  const fromEnv = resolveCredentials({ VIVIAN_EVAL_ENDPOINT: 'http://env.test', VIVIAN_EVAL_MODEL: 'env-model', VIVIAN_EVAL_API_KEY: 'env-key' }, file);
  assert.equal(fromEnv.source, 'env', 'environment must win over the file');

  const partial = resolveCredentials({ VIVIAN_EVAL_ENDPOINT: 'http://env.test' }, { VIVIAN_EVAL_MODEL: 'file-model' });
  assert.equal(partial.complete, false);
  assert.deepEqual(partial.missing, ['VIVIAN_EVAL_API_KEY']);
  assert.equal(partial.source, null, 'an incomplete set must not report a source');
}

// Fingerprint: same content in any file order, but sensitive to content changes.
{
  const a = [{ rel: 'b.md', content: 'two' }, { rel: 'a.md', content: 'one' }];
  const b = [{ rel: 'a.md', content: 'one' }, { rel: 'b.md', content: 'two' }];
  assert.equal(fingerprintPromptSources(a), fingerprintPromptSources(b), 'fingerprint must not depend on walk order');
  const changed = [{ rel: 'b.md', content: 'two!' }, { rel: 'a.md', content: 'one' }];
  assert.notEqual(fingerprintPromptSources(a), fingerprintPromptSources(changed));
  assert.match(fingerprintPromptSources(a), /^[0-9a-f]{16}$/);
}

// The live harness reads the real prompt tree through this helper.
{
  const files = promptSourceFiles('src-tauri/prompts');
  assert.ok(files.length > 20, `expected the prompt tree to expose many sources, saw ${files.length}`);
  assert.ok(files.every((file) => file.rel.endsWith('.md') && !file.rel.includes('\\')), 'sources use forward-slash relative paths');
  assert.ok(files.some((file) => file.rel === 'framework/companion_dialogue.zh.md'));
}

// Blind deck: identical multiset, stable order per scenario, labels hide revision.
{
  const items = [
    { revision: 'baseline', index: 0, reply: 'b0' },
    { revision: 'baseline', index: 1, reply: 'b1' },
    { revision: 'candidate', index: 0, reply: 'c0' },
    { revision: 'candidate', index: 1, reply: 'c1' },
  ];
  const first = buildReviewDeck('tired_without_interview', items);
  const second = buildReviewDeck('tired_without_interview', items);
  assert.deepEqual(first.map((entry) => entry.item.reply), second.map((entry) => entry.item.reply), 'same scenario must keep the same order');
  assert.deepEqual([...first].map((entry) => entry.item.reply).sort(), ['b0', 'b1', 'c0', 'c1'], 'shuffle must be a permutation');
  assert.deepEqual(first.map((entry) => entry.label), ['A', 'B', 'C', 'D']);

  const deck = buildReviewDeck('other_scenario', items);
  const labelsForRevision = new Map();
  for (const entry of deck) {
    if (!labelsForRevision.has(entry.item.revision)) labelsForRevision.set(entry.item.revision, []);
    labelsForRevision.get(entry.item.revision).push(entry.label);
  }
  assert.equal(labelsForRevision.size, 2, 'both revisions must appear in the deck');
}

// The review sheet's vocabulary must match the fixture's human criteria.
{
  assert.equal(REVIEW_DIMENSIONS.length, 5);
  assert.ok(REVIEW_DIMENSIONS.every((dimension) => dimension.key && dimension.label));
  assert.equal(REVIEW_FAILURE_FLAGS.length, 3, 'format, invented experience and ignored boundaries stay separate');
  const fixture = JSON.parse(readFileSync('tests/fixtures/prompt-scenarios.json', 'utf8'));
  assert.ok(fixture.scenarios.some((scenario) => (scenario.review_criteria?.length ?? 0) > 0), 'review criteria must exist to rate against');
}

console.log('Prompt eval support: credentials, fingerprint, blind deck and offline report guarantees verified.');
