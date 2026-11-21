// Pure helpers for the prompt-layer evaluation harness: credential resolution,
// prompt-source fingerprinting and blinded review decks. Kept in a separate
// module so they can be unit-tested without a model.
import { createHash } from 'node:crypto';
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';

export const EVAL_CREDENTIAL_KEYS = ['VIVIAN_EVAL_ENDPOINT', 'VIVIAN_EVAL_MODEL', 'VIVIAN_EVAL_API_KEY'];

// The five dimensions PROMPT_EVALUATION.md asks a human to rate, 1-5 each.
// Format, invented history and ignored boundaries stay separate failure flags.
export const REVIEW_DIMENSIONS = [
  { key: 'catches_the_specific_situation', label: '接住具体处境' },
  { key: 'follows_the_current_need', label: '遵循当前需要' },
  { key: 'character_difference', label: '角色差异' },
  { key: 'low_repetition_and_no_forced_continuation', label: '少重复、不强迫续聊' },
  { key: 'facts_and_memory_reliable', label: '事实与记忆可靠' },
];

export const REVIEW_FAILURE_FLAGS = [
  { key: 'format_violation', label: '格式违规' },
  { key: 'invented_experience_or_perception', label: '编造经历/感知' },
  { key: 'ignored_explicit_boundary', label: '无视明确边界' },
];

/** Parse a dotenv-style file. Comments, blanks and unmatched lines are skipped. */
export function parseEnvFile(text) {
  const values = {};
  for (const rawLine of String(text ?? '').split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith('#')) continue;
    const separator = line.indexOf('=');
    if (separator <= 0) continue;
    const key = line.slice(0, separator).trim();
    if (!key) continue;
    let value = line.slice(separator + 1).trim();
    if (value.length >= 2 && ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'")))) {
      value = value.slice(1, -1);
    }
    values[key] = value;
  }
  return values;
}

/**
 * Resolve live-eval credentials. Environment variables win; an optional
 * dotenv-style file (`.env.local`, git-ignored) fills the rest so a run does
 * not need three exports in every shell.
 */
export function resolveCredentials(env = {}, fileValues = {}) {
  const fromEnv = (key) => (env[key] ?? '').trim();
  const fromFile = (key) => (fileValues[key] ?? '').trim();
  const resolved = EVAL_CREDENTIAL_KEYS.map((key) => ({ key, value: fromEnv(key) || fromFile(key), source: fromEnv(key) ? 'env' : (fromFile(key) ? 'file' : null) }));
  const missing = resolved.filter((entry) => !entry.value).map((entry) => entry.key);
  const sources = new Set(resolved.map((entry) => entry.source));
  return {
    endpoint: resolved[0].value,
    model: resolved[1].value,
    apiKey: resolved[2].value,
    complete: missing.length === 0,
    missing,
    source: missing.length ? null : (sources.size === 1 ? [...sources][0] : 'mixed'),
  };
}

/** All markdown prompt sources under a root, as { rel, content } pairs. */
export function promptSourceFiles(rootDir) {
  const files = [];
  const walk = (dir) => {
    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.name.endsWith('.md')) {
        files.push({ rel: path.relative(rootDir, full).split(path.sep).join('/'), content: readFileSync(full, 'utf8') });
      }
    }
  };
  walk(rootDir);
  return files;
}

/** Stable short hash over the whole prompt tree, so two reports can be paired.
 *  It covers every markdown file under the root, not only the files this
 *  harness assembles, so a doc moved in or out also shifts the fingerprint. */
export function fingerprintPromptSources(files) {
  const hash = createHash('sha256');
  for (const file of [...files].sort((a, b) => (a.rel < b.rel ? -1 : a.rel > b.rel ? 1 : 0))) {
    hash.update(file.rel);
    hash.update('\u0000');
    hash.update(file.content);
    hash.update('\u0000');
  }
  return hash.digest('hex').slice(0, 16);
}

function seedFromString(text) {
  let seed = 2166136261;
  for (const char of String(text)) {
    seed ^= char.codePointAt(0);
    seed = Math.imul(seed, 16777619);
  }
  return seed >>> 0;
}

function mulberry32(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** A/B/C... label for a slot index. */
export function reviewLabel(slot) {
  return String.fromCharCode(65 + (slot % 26)) + (slot >= 26 ? String(Math.floor(slot / 26)) : '');
}

/**
 * Shuffle samples into a presentation order without exposing which revision
 * produced them. Seeded by the scenario id so re-runs of the same setup keep
 * the same order, while labels stay uninformative about revision.
 */
export function buildReviewDeck(scenarioId, items) {
  const rand = mulberry32(seedFromString(scenarioId));
  const order = items.map((_, index) => index);
  for (let i = order.length - 1; i > 0; i -= 1) {
    const j = Math.floor(rand() * (i + 1));
    [order[i], order[j]] = [order[j], order[i]];
  }
  return order.map((itemIndex, slot) => ({ label: reviewLabel(slot), item: items[itemIndex] }));
}
