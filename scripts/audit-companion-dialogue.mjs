// Offline diagnostics: run production UI handlers and the production truncator.
// No model calls, personal histories or application settings are read.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir, mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import { transformSync } from 'esbuild';
import { evaluatePromptReply } from './prompt-reply-evaluation.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const outputArg = process.argv.indexOf('--output-dir');
const outDir = outputArg >= 0 ? path.resolve(process.argv[outputArg + 1])
  : await mkdtemp(path.join(tmpdir(), 'vivian-companion-dialogue-audit-'));
await mkdir(outDir, { recursive: true });
const read = name => readFile(path.join(root, name), 'utf8');
const source = (await read('src/components/ChatWindow.tsx')).replaceAll('\r\n', '\n');
function handler(name) {
  const start = source.indexOf(`unlisten${name} = await listen`);
  assert.ok(start >= 0, `${name} handler registration moved`);
  const body = source.indexOf('(event) => {', start);
  const end = source.indexOf('\n      });', body);
  assert.ok(body >= 0 && end > body, `${name} handler boundary moved`);
  return transformSync(`handler = ${source.slice(body, end)}\n};`, { loader: 'ts' }).code;
}
const chunkCode = handler('Chunk');
const doneCode = handler('Done');
const settlementCode = transformSync((await read('src/utils/chatStreamSettlement.ts')).replace('export function', 'function'), { loader: 'ts' }).code;
function uiProbe(chunks, finalText, channel = 'wechat') {
  let messages = [], previews = {}, id = 0;
  const ref = current => ({ current });
  const context = {
    companionParagraphId: (_stream, message) => message,
    groupStreamCharMapRef: ref(new Map(channel === 'wechat_group' ? [['probe', 'vivian']] : [])),
    groupStreamBuffersRef: ref(new Map()), groupStreamSegmentIdsRef: ref(new Map()),
    groupStreamActiveIdRef: ref(new Map()), privateCharIdRef: ref('vivian'),
    streamBuffersRef: ref(new Map([['probe', '']])), streamSegmentIdsRef: ref(new Map()),
    streamActiveIdRef: ref(new Map()), typingDelayTimerRef: ref(null), typingSafetyTimerRef: ref(null),
    hasStreamingRef: ref(true), pendingRefreshRef: ref(false), refreshTimerRef: ref(null),
    refreshHistoryRef: ref(() => {}), refreshLastPreviewsRef: ref(() => {}),
    hasVisibleChatText: text => Boolean(text.trim()), parseSticker: () => undefined,
    stripActions: text => text, nextId: () => String(++id), setPrivateTyping: () => {},
    setMessages: update => { messages = update(messages); },
    setGroupMessages: update => { messages = update(messages); }, setGroupStreaming: () => {},
    setLastPreviews: update => { previews = update(previews); },
    invoke: async () => {}, window: { setTimeout: () => 1, clearTimeout: () => {} },
  };
  vm.createContext(context);
  vm.runInContext(settlementCode, context);
  vm.runInContext(chunkCode, context);
  const event = text => ({ payload: { text, stream_id: 'probe', character_id: 'vivian', channel } });
  for (const chunk of chunks) context.handler(event(chunk));
  const beforeDone = messages.length;
  vm.runInContext(doneCode, context);
  context.handler(event(finalText));
  const shown = messages.map(message => message.content).join('\n');
  return { bubbles_before_done: beforeDone, shown_text: shown, final_text: finalText,
    shown_matches_final: shown === finalText, preview_text: previews.vivian?.content ?? '' };
}

// Extract the pure production function rather than reimplementing its behavior.
const validation = (await read('src-tauri/src/pipeline/steps/validation.rs')).replaceAll('\r\n', '\n');
const start = validation.indexOf('    fn truncate_at_boundary(');
const end = validation.indexOf('\n    }', start) + '\n    }'.length;
assert.ok(start >= 0 && end > start, 'production truncation function moved');
const min = validation.match(/const MIN_KEEP_CHARS: usize = (\d+);/);
const max = validation.match(/const MAX_RESPONSE_CHARS: usize = ([\d_]+);/);
assert.ok(min && max);
const rust = path.join(outDir, 'truncation.rs');
const executable = path.join(outDir, process.platform === 'win32' ? 'truncation.exe' : 'truncation');
await writeFile(rust, `const MIN_KEEP_CHARS:usize=${min[1]};\n${validation.slice(start, end)}
fn main(){let text="详细说明。".repeat(150);print!("{}",truncate_at_boundary(&text,${max[1]}));}\n`);
const compiled = spawnSync('rustc', ['--edition=2021', rust, '-o', executable], { encoding: 'utf8' });
assert.equal(compiled.status, 0, compiled.stdout + compiled.stderr);
const truncated = spawnSync(executable, [], { encoding: 'utf8' });
assert.equal(truncated.status, 0, truncated.stderr);

const libraries = {};
for (const character of ['vivian', 'nana']) {
  const entries = JSON.parse(await read(`src-tauri/prompts/characters/${character}/example_library.json`));
  const language = text => !text ? 'empty' : /[\u3040-\u30ff]/.test(text) ? 'ja_or_mixed'
    : /[\u4e00-\u9fff]/.test(text) ? 'zh_or_mixed' : 'en_or_other';
  const scripts = {};
  for (const entry of entries) scripts[language(entry.response.text)] = (scripts[language(entry.response.text)] ?? 0) + 1;
  libraries[character] = { entries: entries.length, response_script_heuristic: scripts,
    with_language_metadata: entries.filter(e => e.language || e.locale).length };
}
const fixture = JSON.parse(await read('tests/fixtures/prompt-scenarios.json'));
const naturalnessCases = fixture.scenarios.filter(s => s.review_criteria?.length);
const cannedText = '嗯，听着确实挺折腾的，我先听你说。';
const cannedChecks = naturalnessCases.map(s => ({ id: s.id,
  passes_automatic_checks: evaluatePromptReply(JSON.stringify({ text: cannedText, intent: 'reply' }), s.expect).violations.length === 0 }));
const report = {
  mode: 'offline_production_function_probes', model_calls: 0,
  real_model_quality_measured: false, real_latency_measured: false,
  note: 'UI refs and display helpers are fixtures; production event handler bodies and truncation function execute unchanged.',
  short_reply: uiProbe(['嗯，先歇会儿。'], '嗯，先歇会儿。'),
  changed_final: uiProbe(['原始台词。'], '最终修订后的台词。'),
  corrected_paragraphs: uiProbe(['旧的第一段。\n旧的第二段。'], '修正后的唯一一段。'),
  silence_after_chunks: uiProbe(['不应保留。\n还有半句'], ''),
  final_without_chunks: uiProbe([], '只在最终事件出现的台词。'),
  group_corrected: uiProbe(['旧的第一段。\n旧的第二段。'], '群聊最终回复。', 'wechat_group'),
  group_silence: uiProbe(['不应保留。\n'], '', 'wechat_group'),
  group_final_only: uiProbe([], '群聊最终事件。', 'wechat_group'),
  long_reply: { original_chars: [...'详细说明。'.repeat(150)].length,
    truncated_chars: [...truncated.stdout].length, ...uiProbe(['详细说明。'.repeat(150)], truncated.stdout) },
  example_libraries: libraries,
  canned_reply_control: { text: cannedText, cases: cannedChecks.length,
    automatically_passed: cannedChecks.filter(s => s.passes_automatic_checks).length, results: cannedChecks,
    note: 'A deliberately context-insensitive reply; automated passes do not mean it satisfies the separate human review criteria.' },
};
if (process.argv.includes('--retrieval')) {
  // Reuse the lightweight production-module harness instead of booting Tauri.
  const harness = path.join(root, 'tmp', 'example-retriever-tests');
  await readFile(path.join(harness, 'src', 'lib.rs')).catch(() => {
    throw new Error('Run node tests/example-retriever-offline.test.mjs first to prepare the production retriever harness.');
  });
  const bin = path.join(harness, 'src', 'bin');
  await mkdir(bin, { recursive: true });
  await writeFile(path.join(bin, 'naturalness_probe.rs'), `
use vivian_example_retriever_tests::{example_retriever::ExampleRetriever, memory::embedding::default_embedding};
fn main() {
 let cases = [
  ("direct_vent", "我只想吐槽，别给我建议。"),
  ("quoted_vent_translation", "把“别给我建议”翻译成英文。"),
  ("code_not_feeling", "别分析我，帮我分析这段代码。"),
  ("fatigue_zh", "今天累死了，不想讲经过。"),
  ("fatigue_en", "I'm exhausted. I don't want to explain what happened. Just stay with me for a bit."),
  ("fatigue_ja", "今日は疲れた。何があったか説明したくない。質問しないで。"),
  ("correction_zh", "不是难过，就是烦。"),
  ("fresh_topic", "换个话题，猫为什么像液体？"),
 ];
 let mut out = Vec::new();
 for character in ["vivian", "nana"] {
  let r = ExampleRetriever::new(character, default_embedding());
  for (case, input) in &cases {
   let lang = if case.ends_with("_en") { "en" } else if case.ends_with("_ja") { "ja" } else { "zh" };
   let content = r.retrieve(input, &[], None, &[], lang).unwrap_or_default();
   let ids: Vec<_> = content.lines().filter_map(|line| line.strip_prefix("Example ").filter(|s| !s.starts_with("response:"))).collect();
   out.push(serde_json::json!({"character":character,"case":case,"input":input,"ids":ids,"chars":content.chars().count()}));
  }
 }
 println!("{}", serde_json::to_string(&out).unwrap());
}
`);
  const probe = spawnSync('cargo', ['run', '--offline', '--quiet', '--manifest-path',
    path.join(harness, 'Cargo.toml'), '--bin', 'naturalness_probe'], { encoding: 'utf8',
      env: { ...process.env, VIVIAN_TEST_DATA_DIR: path.join(outDir, 'retrieval-data') } });
  assert.equal(probe.status, 0, probe.stderr);
  report.retrieval = JSON.parse(probe.stdout);
}
const output = path.join(outDir, 'report.json');
for (const [name, result] of Object.entries(report)) {
  if (result && typeof result.shown_matches_final === 'boolean') {
    assert.equal(result.shown_matches_final, true, `${name}: shown text must equal the authoritative final reply`);
  }
}
await writeFile(output, JSON.stringify(report, null, 2) + '\n');
for (const [name, result] of Object.entries(report)) {
  if (result && typeof result === 'object') {
    const compact = name === 'example_libraries' || name === 'retrieval' || name === 'canned_reply_control' ? result : {
      bubbles_before_done: result.bubbles_before_done, shown_matches_final: result.shown_matches_final,
      shown_chars: [...result.shown_text].length, final_chars: [...result.final_text].length,
    };
    console.log(`${name}: ${JSON.stringify(compact)}`);
  }
}
console.log(`Report: ${output}`);
