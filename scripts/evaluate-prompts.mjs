#!/usr/bin/env node
/**
 * Prompt-layer regression harness.
 *
 * PROMPT_EVALUATION.md defines acceptance scenarios for the prompt layer but
 * records them as "尚非已运行结果" — defined, never run. `evaluate-companion.mjs`
 * only runs offline contracts and reports `real_model_quality_measured: false`.
 * This script closes that gap in two stages:
 *
 *   offline (default, no model, CI-safe)
 *     - fixture integrity: unique ids, known groups, compilable regexes
 *     - tool-reference lint: every `"tool": "x"` in a prompt must be registered
 *     - prompt invariants declared by scenarios with `kind: "prompt_invariant"`
 *     - JSON examples in output_format.en.md must actually parse
 *
 *   live (--live, requires VIVIAN_EVAL_ENDPOINT / _MODEL / _API_KEY)
 *     - sends each scenario through the real prompt sources, N times each
 *       (--samples, default 3), and applies the scenario's negative/structural
 *       assertions to every reply
 *     - writes a revision-blind review sheet for the manual 1-5 comparison
 *       PROMPT_EVALUATION.md asks for, so a prompt edit can be judged rather
 *       than guessed at
 *     - with --baseline <dir>, runs the same scenarios against a frozen copy of
 *       the prompt sources and shuffles both revisions into one blind deck
 *       (create the copy before editing with --snapshot)
 *
 * Live mode composes messages from the real prompt files in the documented
 * order. It is a source-level approximation of the production assembly (which
 * lives in companion_prompt.rs / prompt_modules.rs), not a byte-identical
 * reproduction; the report states this explicitly.
 */
import { readFile, writeFile, mkdir, rm, cp } from 'node:fs/promises';
import { readdirSync, readFileSync, existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { evaluatePromptReply } from './prompt-reply-evaluation.mjs';
import {
  EVAL_CREDENTIAL_KEYS,
  REVIEW_DIMENSIONS,
  REVIEW_FAILURE_FLAGS,
  buildReviewDeck,
  fingerprintPromptSources,
  parseEnvFile,
  promptSourceFiles,
  resolveCredentials,
} from './prompt-eval-support.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const argv = process.argv.slice(2);
const live = argv.includes('--live');
const snapshot = argv.includes('--snapshot');
const argValue = (name) => {
  const index = argv.indexOf(name);
  const value = index >= 0 ? argv[index + 1] : undefined;
  return value && !value.startsWith('--') ? value : null;
};
const baselineDir = argValue('--baseline');
const filter = argValue('--filter');
const samplesPerScenario = Math.max(1, Number.parseInt(argValue('--samples') ?? '3', 10) || 3);
const results = [];
let failures = 0;
let configError = false;

const record = (name, passed, detail) => {
  results.push({ name, passed, ...(detail ? { detail } : {}) });
  if (!passed) failures += 1;
  console.log(`  ${passed ? 'ok  ' : 'FAIL'} ${name}${detail && !passed ? ` — ${detail}` : ''}`);
};

// ── prompt sources ────────────────────────────────────────────────────────────
const promptsDir = path.join(root, 'src-tauri/prompts');
const readPrompt = (rel) => readFileSync(path.join(promptsDir, rel), 'utf8');
const readPromptFrom = (dir, rel) => {
  try { return readFileSync(path.join(dir, rel), 'utf8'); } catch { return ''; }
};

// --snapshot freezes the current prompt sources so a later edit can be compared
// against them through --baseline. Run it before touching any prompt file.
if (snapshot) {
  const target = path.resolve(root, baselineDir ?? 'tmp/prompt-baseline');
  await rm(target, { recursive: true, force: true });
  await cp(promptsDir, target, { recursive: true });
  console.log(`[prompt-eval] snapshot → ${path.relative(root, target)} · fingerprint ${fingerprintPromptSources(promptSourceFiles(target))}`);
  console.log(`[prompt-eval] compare later with: node scripts/evaluate-prompts.mjs --live --baseline ${baselineDir ?? 'tmp/prompt-baseline'}`);
  process.exit(0);
}

function promptFiles(dir = promptsDir, acc = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) promptFiles(full, acc);
    else if (entry.name.endsWith('.md')) acc.push(full);
  }
  return acc;
}

// ── registered tool names ─────────────────────────────────────────────────────
function registeredTools() {
  const names = new Set();
  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.name.endsWith('.rs')) {
        const src = readFileSync(full, 'utf8');
        const re = /fn name\(&self\) -> &str \{\s*\n\s*"([a-z][a-z_0-9]*)"/g;
        let match;
        while ((match = re.exec(src))) names.add(match[1]);
      }
    }
  };
  const dir = path.join(root, 'src-tauri/src/tools');
  if (existsSync(dir)) walk(dir);
  return names;
}

// ── fixture ───────────────────────────────────────────────────────────────────
const fixturePath = path.join(root, 'tests/fixtures/prompt-scenarios.json');
const fixture = JSON.parse(await readFile(fixturePath, 'utf8'));
const scenarios = fixture.scenarios ?? [];
const GROUPS = new Set(['work', 'companion', 'proactive', 'memory']);

console.log(`\n[prompt-eval] offline checks — ${scenarios.length} scenarios from ${path.relative(root, fixturePath)}`);

// ── 1. fixture integrity ──────────────────────────────────────────────────────
{
  const ids = scenarios.map((s) => s.id);
  const dupes = ids.filter((id, i) => ids.indexOf(id) !== i);
  record('fixture: ids are unique', dupes.length === 0, `duplicates: ${[...new Set(dupes)].join(', ')}`);

  const badGroup = scenarios.filter((s) => !GROUPS.has(s.group)).map((s) => `${s.id}:${s.group}`);
  record('fixture: every scenario has a known group', badGroup.length === 0, badGroup.join(', '));

  // A scenario is only meaningful if it can actually be judged.
  const unjudgeable = scenarios.filter((s) => {
    const e = s.expect ?? {};
    const hasAssertion = (e.reply_forbidden?.length ?? 0) > 0
      || (e.reply_required?.length ?? 0) > 0
      || typeof e.reply_min_chars === 'number'
      || e.proactive_must_decline === true
      || (s.prompt_must_match?.length ?? 0) > 0;
    return !hasAssertion;
  }).map((s) => s.id);
  record('fixture: every scenario has a machine-checkable assertion', unjudgeable.length === 0, unjudgeable.join(', '));

  const badRegex = [];
  for (const s of scenarios) {
    for (const key of ['reply_forbidden', 'reply_required']) {
      for (const pattern of s.expect?.[key] ?? []) {
        try { new RegExp(pattern, 'i'); } catch { badRegex.push(`${s.id}.${key}: ${pattern}`); }
      }
    }
  }
  record('fixture: all assertions compile as regexes', badRegex.length === 0, badRegex.join(' | '));

  const badBounds = scenarios.filter(s => {
    const e = s.expect ?? {};
    return ['reply_min_chars', 'reply_max_chars', 'reply_max_questions'].some(key =>
      key in e && (!Number.isInteger(e[key]) || e[key] < 0))
      || (e.reply_min_chars ?? 0) > (e.reply_max_chars ?? Infinity);
  }).map(s => s.id);
  record('fixture: speech bounds are consistent nonnegative integers', badBounds.length === 0, badBounds.join(', '));
  const badLanguages = scenarios.filter(s => s.language && !['zh', 'en', 'ja'].includes(s.language)).map(s => s.id);
  record('fixture: localized scenarios use supported languages', badLanguages.length === 0, badLanguages.join(', '));
}

// ── 2. tool-reference lint ────────────────────────────────────────────────────
{
  const tools = registeredTools();
  record('tool registry: found registered tools', tools.size > 0, `${tools.size} tools`);

  const unknown = [];
  for (const file of promptFiles()) {
    const src = readFileSync(file, 'utf8');
    const rel = path.relative(root, file);
    const re = /"tool"\s*:\s*"([a-z][a-z_0-9]*)"/g;
    let match;
    while ((match = re.exec(src))) {
      if (!tools.has(match[1])) unknown.push(`${rel} → ${match[1]}`);
    }
  }
  record('tool lint: every "tool" reference in a prompt is registered', unknown.length === 0, unknown.join(' | '));

  // Prose references: a backticked identifier within 80 chars of the word "tool".
  // Prompt file names and JSON schema field names are not tool references.
  const promptBasenames = new Set(
    promptFiles().map((file) => path.basename(file).replace(/\.(en|ja|zh)\.md$/, '').replace(/\.md$/, '')),
  );
  const SCHEMA_FIELDS = new Set([
    'no_reply', 'short_reply', 'reply', 'text', 'intent', 'arguments', 'json', 'memory_used',
    'voice_message', 'sticker_id', 'response_mode', 'emotion_delta', 'monologue', 'notify',
    'expression', 'motion', 'control_actions', 'appraisal', 'emotion_update', 'event_summary',
    'behavior_drive', 'world_update', 'goal_updates', 'long_term_memory', 'source_quote',
    'explicit_feedback', 'reason', 'revises', 'tone', 'personality', 'scope', 'importance',
    'has_valuable_memory', 'operations', 'user_activity', 'confidence', 'reference',
  ]);
  const proseUnknown = [];
  for (const file of promptFiles()) {
    const src = readFileSync(file, 'utf8');
    const rel = path.relative(root, file);
    const re = /`([a-z][a-z_0-9]{3,})`/g;
    let match;
    while ((match = re.exec(src))) {
      const token = match[1];
      if (tools.has(token) || promptBasenames.has(token) || SCHEMA_FIELDS.has(token)) continue;
      const context = src.slice(Math.max(0, match.index - 80), match.index + 80);
      if (!/\btools?\b|工具/i.test(context)) continue;
      proseUnknown.push(`${rel} → ${token}`);
    }
  }
  record('tool lint: prose tool mentions resolve to registered tools', proseUnknown.length === 0, proseUnknown.join(' | '));
}

// ── 3. scenario-declared prompt invariants ────────────────────────────────────
{
  const invariantScenarios = scenarios.filter((s) => s.kind === 'prompt_invariant');
  for (const scenario of invariantScenarios) {
    for (const rule of scenario.prompt_must_match ?? []) {
      const target = path.join(root, rule.file);
      let source = '';
      try { source = readFileSync(target, 'utf8'); } catch { /* missing file is a failure */ }
      const ok = source.length > 0 && new RegExp(rule.pattern, 'i').test(source);
      record(`${scenario.id}: ${rule.file} matches /${rule.pattern}/`, ok);
    }
  }
}

// ── 4. output_format JSON examples must parse ─────────────────────────────────
{
  const src = readPrompt('framework/output_format.en.md');
  const blocks = [];
  for (const line of src.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed.startsWith('{') && !trimmed.startsWith('[')) continue;
    if (!trimmed.endsWith('}') && !trimmed.endsWith(']')) continue;
    // Section tags such as [OUTPUT_FIELDS] / [/OUTPUT_FIELDS] are not JSON.
    if (/^\[\/?[A-Z_]+]$/.test(trimmed)) continue;
    blocks.push(trimmed);
  }
  const broken = blocks.filter((block) => {
    try { JSON.parse(block); return false; } catch { return true; }
  });
  record('output_format: every JSON example parses', broken.length === 0 && blocks.length > 0,
    broken.length ? broken.join(' | ') : (blocks.length ? '' : 'no JSON examples found'));
}

// ── 5. silence-schema consistency ─────────────────────────────────────────────
{
  // A prompt that names the silence intent but never states the schema invites the
  // model to invent one. Every file mentioning no_reply must carry the JSON shape.
  const offenders = [];
  for (const file of promptFiles()) {
    const src = readFileSync(file, 'utf8');
    if (!/no_reply/.test(src)) continue;
    if (!/"intent"/.test(src)) offenders.push(path.relative(root, file));
  }
  record('silence: files mentioning no_reply also state the "intent" schema', offenders.length === 0, offenders.join(' | '));

  // The canonical silence payload must appear somewhere, verbatim.
  const canonical = promptFiles().some((file) => /"text"\s*:\s*""\s*,\s*"intent"\s*:\s*"no_reply"/.test(readFileSync(file, 'utf8')));
  record('silence: canonical {"text":"","intent":"no_reply"} payload is present', canonical);
}

// ── manual review sheet ───────────────────────────────────────────────────────
// PROMPT_EVALUATION.md asks for a shuffled 1-5 comparison of before/after
// samples. Regex assertions cannot measure naturalness, so the sheet keeps the
// replies in presentation order and leaves the revision out of the page.
function renderReviewSheet(liveScenarios, items) {
  const lines = [
    '# 陪伴提示词盲评工作表',
    '',
    '样本顺序已打乱，标签与提示词版本无关。先逐条评分，再看 tmp/prompt-eval-report.json 的 review_key 还原对应关系。',
    '',
    `评分维度（每项 1-5）：${REVIEW_DIMENSIONS.map((dimension) => dimension.label).join(' / ')}`,
    '',
    `单列失败标记（不计入均分）：${REVIEW_FAILURE_FLAGS.map((flag) => flag.label).join(' / ')}`,
    '',
  ];
  const byScenario = new Map();
  for (const item of items) {
    if (!byScenario.has(item.scenario_id)) byScenario.set(item.scenario_id, []);
    byScenario.get(item.scenario_id).push(item);
  }
  for (const scenario of liveScenarios) {
    const deck = byScenario.get(scenario.id) ?? [];
    if (!deck.length) continue;
    lines.push(`## ${scenario.id}${scenario.title ? ` · ${scenario.title}` : ''}`, '', `输入：${scenario.input}`, '');
    for (const criteria of scenario.review_criteria ?? []) lines.push(`- 复核：${criteria}`);
    if (scenario.review_criteria?.length) lines.push('');
    for (const item of deck) {
      const reply = item.reply?.trim() ? item.reply.trim() : `（无回复${item.failed_request ? `：${item.failed_request}` : ''}）`;
      lines.push(`**${item.label}.** ${reply}`, '');
    }
    lines.push(`| 样本 | ${REVIEW_DIMENSIONS.map((dimension) => dimension.label).join(' | ')} | 失败标记 |`);
    lines.push(`| --- |${REVIEW_DIMENSIONS.map(() => ' --- |').join('')} --- |`);
    for (const item of deck) lines.push(`| ${item.label} |${REVIEW_DIMENSIONS.map(() => ' |').join('')} | |`);
    lines.push('');
  }
  return `${lines.join('\n')}\n`;
}

// ── 6. live model evaluation (opt-in) ─────────────────────────────────────────
const envFile = path.join(root, '.env.local');
const envFileValues = existsSync(envFile) ? parseEnvFile(readFileSync(envFile, 'utf8')) : {};
const credentials = resolveCredentials(process.env, envFileValues);
const liveReady = Boolean(live && credentials.complete);

const revisions = [{ id: 'candidate', dir: promptsDir }];
if (baselineDir) {
  const dir = path.resolve(root, baselineDir);
  if (!existsSync(dir)) {
    console.error(`\n[prompt-eval] --baseline directory not found: ${baselineDir}`);
    configError = true;
  } else if (dir !== promptsDir) {
    revisions.unshift({ id: 'baseline', dir });
  }
}
for (const revision of revisions) {
  revision.fingerprint = fingerprintPromptSources(promptSourceFiles(revision.dir));
}

if (live && !liveReady) {
  console.error(`\n[prompt-eval] --live needs all of: ${EVAL_CREDENTIAL_KEYS.join(', ')}`);
  console.error(`[prompt-eval] still missing: ${credentials.missing.join(', ')}`);
  console.error(`[prompt-eval] set them in the shell, or write them to ${path.relative(root, envFile)} (git-ignored):`);
  for (const key of EVAL_CREDENTIAL_KEYS) console.error(`[prompt-eval]   ${key}=…`);
  configError = true;
}

const modelResults = [];
const reviewItems = [];
const reviewKey = {};
let reviewSheet = null;

async function callModel(messages) {
  try {
    const response = await fetch(`${credentials.endpoint.replace(/\/$/, '')}/chat/completions`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: `Bearer ${credentials.apiKey}` },
      body: JSON.stringify({ model: credentials.model, messages, temperature: 0.7 }),
    });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const payload = await response.json();
    return { reply: payload.choices?.[0]?.message?.content ?? '', error: null };
  } catch (cause) {
    return { reply: '', error: String(cause.message ?? cause) };
  }
}

let liveScenarios = [];
if (liveReady && !configError) {
  liveScenarios = scenarios.filter((scenario) => scenario.kind !== 'prompt_invariant'
    && (!filter || scenario.id.includes(filter) || scenario.group === filter));
  if (!liveScenarios.length) {
    console.error(`\n[prompt-eval] --filter "${filter}" matched no scenario.`);
    configError = true;
  }
}

if (liveReady && !configError && liveScenarios.length) {
  console.log(`\n[prompt-eval] live evaluation — ${credentials.model} @ ${credentials.endpoint} · credentials from ${credentials.source}`);
  console.log(`[prompt-eval] ${liveScenarios.length} scenarios × ${revisions.length} revisions × ${samplesPerScenario} samples`
    + ` = ${liveScenarios.length * revisions.length * samplesPerScenario} calls`);
  console.log(`[prompt-eval] prompt sources — ${revisions.map((revision) => `${revision.id}:${revision.fingerprint}`).join(' · ')}`);

  const systemFor = (dir, scenario) => {
    if (scenario.group === 'proactive') {
      return [
        readPromptFrom(dir, 'framework/companion_contract.en.md'),
        readPromptFrom(dir, 'framework/proactive_companionship.en.md'),
        'You may decline. To decline, reply with exactly DONT_NOTIFY.',
      ].filter(Boolean).join('\n\n');
    }
    if (scenario.group === 'work') {
      return [readPromptFrom(dir, 'work/execution.md'), readPromptFrom(dir, 'framework/companion_contract.en.md'),
        readPromptFrom(dir, 'framework/output_format.en.md')].join('\n\n');
    }
    const language = scenario.language ?? 'en';
    return [readPromptFrom(dir, `framework/companion_dialogue.${language}.md`),
      readPromptFrom(dir, 'framework/companion_contract.en.md'),
      readPromptFrom(dir, `framework/conversation_focus.${language}.md`),
      readPromptFrom(dir, `framework/companion_post_history.${language}.md`),
      readPromptFrom(dir, 'framework/output_format.en.md')].join('\n\n');
  };

  const userFor = (scenario) => {
    const lines = [scenario.input];
    const state = scenario.state ?? {};
    if (state.memory) lines.unshift(`[Memory context] ${state.memory}`);
    if (state.history?.length) lines.unshift(`[Earlier short replies] ${state.history.join(' / ')}`);
    if (state.tool_results?.length) lines.push(`[Tool results] ${JSON.stringify(state.tool_results)}`);
    if (state.result) lines.push(`[Background result] ${JSON.stringify(state.result)}`);
    if (state.objective) lines.push(`[Objective] ${state.objective}`);
    if (state.user_correction) lines.push(`[User correction] ${state.user_correction}`);
    if (state.weather) lines.push(`[Timestamped weather context] ${JSON.stringify(state.weather)}`);
    if (state.location) lines.push(`[Configured location] ${JSON.stringify(state.location)}`);
    if (state.failed_checks?.length) lines.push(`[Failed checks] ${state.failed_checks.join('; ')}`);
    if (state.compacted) lines.push('[Note] Earlier context was compacted.');
    if (state.unchanged_status) lines.push('[Note] The background status is unchanged from the previous trigger.');
    if (state.screen_data === null && 'screen_data' in state) lines.push('[Note] No screen data is available.');
    if (state.weather_data === null && !state.weather) lines.push('[Note] No weather data is available.');
    return lines.join('\n');
  };

  for (const scenario of liveScenarios) {
    const samples = [];
    for (const revision of revisions) {
      const messages = [
        { role: 'system', content: systemFor(revision.dir, scenario) },
        ...(scenario.state?.dialogue ?? []),
        { role: 'user', content: userFor(scenario) },
      ];
      for (let index = 0; index < samplesPerScenario; index += 1) {
        const { reply, error } = await callModel(messages);
        samples.push({
          revision: revision.id,
          index,
          reply,
          error,
          violations: error ? [`request failed: ${error}`] : evaluatePromptReply(reply, scenario.expect).violations,
        });
      }
    }

    const perRevision = revisions.map((revision) => {
      const own = samples.filter((sample) => sample.revision === revision.id);
      return {
        revision: revision.id,
        samples: own.length,
        assertion_pass_rate: own.length ? own.filter((sample) => !sample.violations.length).length / own.length : 0,
        replies: own.map(({ index, reply, error, violations }) => ({ index, reply, violations, ...(error ? { error } : {}) })),
      };
    });
    record(`live: ${scenario.id}`, perRevision.every((entry) => entry.assertion_pass_rate === 1),
      perRevision.map((entry) => `${entry.revision} ${entry.replies.filter((reply) => !reply.violations.length).length}/${entry.samples}`).join(' · '));
    modelResults.push({
      scenario_id: scenario.id,
      group: scenario.group,
      title: scenario.title ?? null,
      review_criteria: scenario.review_criteria ?? [],
      human_review_required: Boolean(scenario.review_criteria?.length),
      revisions: perRevision,
    });

    for (const { label, item } of buildReviewDeck(scenario.id, samples)) {
      const itemId = `${scenario.id}::${label}`;
      reviewKey[itemId] = { scenario_id: scenario.id, revision: item.revision, sample_index: item.index };
      reviewItems.push({
        item_id: itemId,
        scenario_id: scenario.id,
        group: scenario.group,
        title: scenario.title ?? null,
        input: scenario.input,
        language: scenario.language ?? 'en',
        review_criteria: scenario.review_criteria ?? [],
        dimensions: REVIEW_DIMENSIONS.map((dimension) => dimension.key),
        failure_flags: REVIEW_FAILURE_FLAGS.map((flag) => flag.key),
        reply: item.reply,
        failed_request: item.error ?? null,
        label,
      });
    }
  }

  const reviewJson = path.join(root, 'tmp/prompt-eval-review.json');
  const reviewMarkdown = path.join(root, 'tmp/prompt-eval-review.md');
  await writeFile(reviewJson, `${JSON.stringify({ dimensions: REVIEW_DIMENSIONS, failure_flags: REVIEW_FAILURE_FLAGS, items: reviewItems }, null, 2)}\n`);
  await writeFile(reviewMarkdown, renderReviewSheet(liveScenarios, reviewItems));
  reviewSheet = { json: path.relative(root, reviewJson), markdown: path.relative(root, reviewMarkdown), items: reviewItems.length };
}

// ── report ────────────────────────────────────────────────────────────────────
const modelCalls = modelResults.reduce((total, scenario) =>
  total + scenario.revisions.reduce((sum, entry) => sum + entry.replies.length, 0), 0);
const candidate = revisions.find((revision) => revision.id === 'candidate');
const baseline = revisions.find((revision) => revision.id === 'baseline');

const report = {
  mode: modelCalls ? 'offline_and_live' : 'offline',
  model_calls: modelCalls,
  real_model_quality_measured: modelCalls > 0,
  model: modelCalls ? credentials.model : null,
  credentials_source: credentials.source,
  samples_per_scenario: modelCalls ? samplesPerScenario : 0,
  scenarios_total: scenarios.length,
  scenarios_evaluated_live: modelResults.length,
  filter: filter ?? null,
  prompt_fingerprint: candidate?.fingerprint ?? null,
  baseline_prompt_fingerprint: baseline?.fingerprint ?? null,
  revisions: revisions.map((revision) => ({ id: revision.id, path: path.relative(root, revision.dir), fingerprint: revision.fingerprint })),
  assembly_note: modelCalls
    ? 'Live messages are composed from the real prompt sources in documented order; this approximates, but does not byte-match, companion_prompt.rs / prompt_modules.rs.'
    : null,
  comparison_note: modelCalls
    ? (baseline
      ? 'Replies from both revisions are shuffled into one blind deck. Rate tmp/prompt-eval-review.md item by item, then resolve labels through review_key in this report.'
      : 'Review items carry no revision label; pass --baseline to compare two prompt revisions in one blind deck.')
    : null,
  review_sheet: reviewSheet,
  review_key: reviewKey,
  checks: results,
  model_results: modelResults,
};
await mkdir(path.join(root, 'tmp'), { recursive: true });
const output = path.join(root, 'tmp/prompt-eval-report.json');
await writeFile(output, `${JSON.stringify(report, null, 2)}\n`);

const passed = results.filter((r) => r.passed).length;
console.log(`\n[prompt-eval] ${passed}/${results.length} checks passed · mode=${report.mode} · real_model_quality_measured=${report.real_model_quality_measured}`);
console.log(`[prompt-eval] report: ${path.relative(root, output)}`);
if (reviewSheet) console.log(`[prompt-eval] review sheet: ${reviewSheet.markdown} · ${reviewSheet.items} items`);
if (!liveReady) {
  console.log('[prompt-eval] live model evaluation skipped — set VIVIAN_EVAL_ENDPOINT/_MODEL/_API_KEY (or .env.local) and pass --live.');
}
process.exitCode = failures || configError ? 1 : 0;
