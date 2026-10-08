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
 *     - sends each scenario through the real prompt sources and applies the
 *       scenario's negative/structural assertions to the model's reply
 *
 * Live mode composes messages from the real prompt files in the documented
 * order. It is a source-level approximation of the production assembly (which
 * lives in companion_prompt.rs / prompt_modules.rs), not a byte-identical
 * reproduction; the report states this explicitly.
 */
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { readdirSync, readFileSync, existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const live = process.argv.includes('--live');
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

// ── 6. live model evaluation (opt-in) ─────────────────────────────────────────
const endpoint = process.env.VIVIAN_EVAL_ENDPOINT;
const model = process.env.VIVIAN_EVAL_MODEL;
const apiKey = process.env.VIVIAN_EVAL_API_KEY;
const liveReady = Boolean(live && endpoint && model && apiKey);

if (live && !liveReady) {
  console.error('\n[prompt-eval] --live requires VIVIAN_EVAL_ENDPOINT, VIVIAN_EVAL_MODEL and VIVIAN_EVAL_API_KEY.');
  configError = true;
}

const modelResults = [];

if (liveReady) {
  console.log(`\n[prompt-eval] live evaluation — ${model} @ ${endpoint}`);

  const framework = {
    dialogue: readPrompt('framework/companion_dialogue.en.md'),
    contract: readPrompt('framework/companion_contract.en.md'),
    focus: readPrompt('framework/conversation_focus.en.md'),
    postHistory: readPrompt('framework/companion_post_history.en.md'),
    outputFormat: readPrompt('framework/output_format.en.md'),
    work: readPrompt('work/execution.md'),
    proactive: readPrompt('framework/proactive_companionship.en.md'),
  };

  const systemFor = (scenario) => {
    if (scenario.group === 'proactive') {
      return [
        framework.contract,
        framework.proactive,
        'You may decline. To decline, reply with exactly DONT_NOTIFY.',
        scenario.kind === 'proactive' ? '' : '',
      ].filter(Boolean).join('\n\n');
    }
    if (scenario.group === 'work') {
      return [framework.work, framework.contract].join('\n\n');
    }
    return [framework.dialogue, framework.contract, framework.focus, framework.postHistory, framework.outputFormat].join('\n\n');
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

  for (const scenario of scenarios) {
    if (scenario.kind === 'prompt_invariant') continue;
    const body = {
      model,
      messages: [
        { role: 'system', content: systemFor(scenario) },
        ...(scenario.state?.dialogue ?? []),
        { role: 'user', content: userFor(scenario) },
      ],
      temperature: 0.7,
    };
    let reply = '';
    let error = null;
    try {
      const response = await fetch(`${endpoint.replace(/\/$/, '')}/chat/completions`, {
        method: 'POST',
        headers: { 'content-type': 'application/json', authorization: `Bearer ${apiKey}` },
        body: JSON.stringify(body),
      });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const payload = await response.json();
      reply = payload.choices?.[0]?.message?.content ?? '';
    } catch (cause) {
      error = String(cause.message ?? cause);
    }

    const violations = [];
    if (error) {
      violations.push(`request failed: ${error}`);
    } else {
      const expect = scenario.expect ?? {};
      const text = reply.trim();
      for (const pattern of expect.reply_forbidden ?? []) {
        if (new RegExp(pattern, 'i').test(text)) violations.push(`forbidden /${pattern}/`);
      }
      if (expect.reply_required?.length) {
        const satisfied = expect.reply_required.some((p) => new RegExp(p, 'i').test(text));
        if (!satisfied) violations.push(`none of required ${expect.reply_required.join(' | ')}`);
      }
      if (typeof expect.reply_min_chars === 'number' && text.length < expect.reply_min_chars) {
        violations.push(`reply shorter than ${expect.reply_min_chars} chars (${text.length})`);
      }
      if (expect.proactive_must_decline === true) {
        const declined = /DONT_NOTIFY/i.test(text) || /"text"\s*:\s*""/.test(text) || text === '';
        if (!declined) violations.push('did not decline on an unchanged repeated trigger');
      }
    }

    modelResults.push({ id: scenario.id, group: scenario.group, passed: violations.length === 0, violations, reply: reply.slice(0, 400) });
    record(`live: ${scenario.id}`, violations.length === 0, violations.join('; '));
  }
}

// ── report ────────────────────────────────────────────────────────────────────
const report = {
  mode: liveReady ? 'offline_and_live' : 'offline',
  model_calls: modelResults.length,
  real_model_quality_measured: liveReady && modelResults.length > 0,
  model: liveReady ? model : null,
  scenarios_total: scenarios.length,
  scenarios_evaluated_live: modelResults.length,
  assembly_note: liveReady
    ? 'Live messages are composed from the real prompt sources in documented order; this approximates, but does not byte-match, companion_prompt.rs / prompt_modules.rs.'
    : null,
  checks: results,
  model_results: modelResults,
};
await mkdir(path.join(root, 'tmp'), { recursive: true });
const output = path.join(root, 'tmp/prompt-eval-report.json');
await writeFile(output, `${JSON.stringify(report, null, 2)}\n`);

const passed = results.filter((r) => r.passed).length;
console.log(`\n[prompt-eval] ${passed}/${results.length} checks passed · mode=${report.mode} · real_model_quality_measured=${report.real_model_quality_measured}`);
console.log(`[prompt-eval] report: ${path.relative(root, output)}`);
if (!liveReady) {
  console.log('[prompt-eval] live model evaluation skipped — set VIVIAN_EVAL_ENDPOINT/_MODEL/_API_KEY and pass --live.');
}
process.exitCode = failures || configError ? 1 : 0;
