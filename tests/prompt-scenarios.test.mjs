import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';

// The acceptance scenarios in src-tauri/prompts/PROMPT_EVALUATION.md were defined
// but never run. scripts/evaluate-prompts.mjs makes them executable; this test
// keeps the offline half of that harness enforced in CI.
const run = spawnSync(process.execPath, ['scripts/evaluate-prompts.mjs'], { encoding: 'utf8' });
assert.equal(run.status, 0, `prompt harness must pass offline:\n${run.stdout}\n${run.stderr}`);

const reportPath = 'tmp/prompt-eval-report.json';
assert.ok(existsSync(reportPath), 'prompt harness must write its report');
const report = JSON.parse(readFileSync(reportPath, 'utf8'));

assert.equal(report.mode, 'offline', 'default run must stay offline so CI needs no credentials');
assert.equal(report.model_calls, 0, 'offline run must not call a model');
assert.equal(report.real_model_quality_measured, false, 'offline run must not claim to have measured model quality');

const scenarios = JSON.parse(readFileSync('tests/fixtures/prompt-scenarios.json', 'utf8')).scenarios;
assert.equal(report.scenarios_total, scenarios.length, 'report must cover every scenario');
assert.ok(scenarios.length >= 19, 'keep the full acceptance table from PROMPT_EVALUATION.md');

// Every scenario must be judgeable, and each group from the acceptance table must survive.
const groups = new Set(scenarios.map((scenario) => scenario.group));
for (const group of ['work', 'companion', 'proactive', 'memory']) {
  assert.ok(groups.has(group), `acceptance table must keep covering ${group}`);
}
for (const scenario of scenarios) {
  const expect = scenario.expect ?? {};
  const judgeable = (expect.reply_forbidden?.length ?? 0) > 0
    || (expect.reply_required?.length ?? 0) > 0
    || typeof expect.reply_min_chars === 'number'
    || expect.proactive_must_decline === true
    || (scenario.prompt_must_match?.length ?? 0) > 0;
  assert.ok(judgeable, `${scenario.id} must stay machine-checkable`);
  for (const key of ['reply_forbidden', 'reply_required']) {
    for (const pattern of expect[key] ?? []) {
      assert.doesNotThrow(() => new RegExp(pattern, 'i'), `${scenario.id}.${key} must compile: ${pattern}`);
    }
  }
}

const failed = report.checks.filter((check) => !check.passed);
assert.equal(failed.length, 0, `offline checks failed: ${failed.map((check) => check.name).join(', ')}`);

console.log(`Prompt scenarios: ${scenarios.length} acceptance cases, ${report.checks.length} offline checks, report shape and offline-only guarantees verified.`);
