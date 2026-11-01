import assert from 'node:assert/strict';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

// Temporal awareness contract: prompt-side gap markers live in
// message_context (prompt-orchestration/communication-context harnesses) and
// the two guards below decide whether wording or a duplicate utterance may
// reach the user. Both are extracted from production source verbatim so a
// behavioural regression turns this test red instead of silently passing.

const root = process.cwd();
const abs = (p) => path.join(root, p).replaceAll('\\', '/');

const proactive = (await readFile('src-tauri/src/proactive/mod.rs', 'utf8')).replaceAll('\r\n', '\n');
const duplicateStart = proactive.indexOf('const NEAR_DUPLICATE_SIMILARITY');
const duplicateEnd = proactive.indexOf('static RNG_COUNTER');
assert.ok(duplicateStart > 0 && duplicateEnd > duplicateStart, 'duplicate guard slice moved');
const duplicateGuard = proactive.slice(duplicateStart, duplicateEnd);

// Recall labels must come from the same helper as history and recaps. Slice the
// production function so a re-implementation (own day boundary, own wording)
// breaks this test instead of drifting quietly.
const memoryStep = (await readFile('src-tauri/src/pipeline/steps/memory.rs', 'utf8')).replaceAll('\r\n', '\n');
const labelStart = memoryStep.indexOf('fn is_authored_context');
const labelEnd = memoryStep.indexOf('// ============================================================================');
assert.ok(labelStart > 0 && labelEnd > labelStart, 'memory time label slice moved');
const memoryLabel = memoryStep.slice(labelStart, labelEnd);

const dir = await mkdtemp(path.join(tmpdir(), 'vivian-temporal-'));
try {
  const input = path.join(dir, 'main.rs');
  await writeFile(input, `
use crate::types::response::ChatMessage;
use chrono::{DateTime, Local};
#[path=${JSON.stringify(abs('src-tauri/src/utils/prompt_time.rs'))}] pub mod prompt_time;
mod utils { pub use crate::prompt_time; }
#[path=${JSON.stringify(abs('src-tauri/src/pipeline/temporal_consistency.rs'))}] pub mod temporal_consistency;
mod pipeline {
    pub mod prompt_modules {
        pub fn normalize_lang(lang: &str) -> &str {
            let l = lang.trim().to_ascii_lowercase();
            if l.starts_with("en") { "en" } else if l.starts_with("ja") { "ja" } else { "zh" }
        }
    }
}
mod i18n { pub fn get_language() -> String { "zh".into() } }
mod memory {
    pub mod time_stamped {
        use chrono::{DateTime, Local};
        pub struct TimeStampedSummary { pub content: String, pub start_time: DateTime<Local>, pub end_time: DateTime<Local> }
        #[derive(Default)]
        pub struct TimeStampedMemory { summaries: Vec<TimeStampedSummary> }
        impl TimeStampedMemory {
            pub fn get_summaries(&self) -> &[TimeStampedSummary] { &self.summaries }
            #[cfg(test)]
            pub(crate) fn with_test_summary(content: &str, start_time: DateTime<Local>, end_time: DateTime<Local>) -> Self {
                Self { summaries: vec![TimeStampedSummary { content: content.to_string(), start_time, end_time }] }
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct MemoryItem { pub id: String, pub timestamp: f64, pub metadata: serde_json::Value }
}
#[path=${JSON.stringify(abs('src-tauri/src/memory/session_compressor.rs'))}] pub mod session_compressor;
use serde_json::Value;
${memoryLabel}
mod types {
    pub mod response {
        #[derive(Clone, Debug, Default)]
        pub struct ChatMessage {
            pub role: String,
            pub content: String,
            pub timestamp: Option<chrono::DateTime<chrono::Local>>,
        }
        impl ChatMessage {
            pub fn system(s: impl Into<String>) -> Self { Self { role: "system".into(), content: s.into(), timestamp: None } }
            pub fn user(s: impl Into<String>) -> Self { Self { role: "user".into(), content: s.into(), timestamp: None } }
            pub fn assistant(s: impl Into<String>) -> Self { Self { role: "assistant".into(), content: s.into(), timestamp: None } }
        }
    }
}
${duplicateGuard}

#[cfg(test)]
mod guard_tests {
    use super::*;

    #[test]
    fn near_duplicate_ignores_punctuation_and_case_noise() {
        assert!(near_duplicate(
            "你好呀，Alen！我是 Nana，很高兴认识你呢。",
            "你好呀 Alen，我是 Nana，很高兴认识你。"
        ));
        assert!(near_duplicate("I just said hello.", "I JUST SAID HELLO"));
    }

    #[test]
    fn near_duplicate_keeps_distinct_short_replies() {
        assert!(!near_duplicate("嗯", "嗯嗯"));
        assert!(!near_duplicate("好的", "晚安"));
    }

    #[test]
    fn near_duplicate_does_not_suppress_a_different_subject() {
        assert!(!near_duplicate(
            "这个动漫收藏夹的数据我整理好了。",
            "今天的天气看起来要下雨了。"
        ));
        assert!(!near_duplicate("", "你好呀 Alen"));
    }

    #[test]
    fn recall_labels_share_local_boundary_and_language() {
        use chrono::Local;
        let now = Local::now();
        let mut item = crate::memory::MemoryItem {
            id: "m1".into(),
            timestamp: (now - chrono::Duration::hours(3)).timestamp() as f64,
            metadata: serde_json::json!({"source": "direct", "speaker": "user"}),
        };
        let zh = memory_time_label(&item, now, "zh");
        assert!(zh.contains("小时前"), "{zh}");
        let en = memory_time_label(&item, now, "en");
        assert!(en.contains("h ago"), "{en}");
        let ja = memory_time_label(&item, now, "ja");
        assert!(ja.contains("時間前"), "{ja}");
        // Yesterday must keep a date anchor instead of only counting hours.
        item.timestamp = (now - chrono::Duration::days(1)).timestamp() as f64;
        let old = memory_time_label(&item, now, "zh");
        assert!(old.contains("昨天") || old.contains("天前"), "{old}");
    }

    #[test]
    fn authored_background_never_reports_a_relative_age() {
        use chrono::Local;
        let now = Local::now();
        let plain = crate::memory::MemoryItem {
            id: "seed_1".into(),
            timestamp: (now - chrono::Duration::days(30)).timestamp() as f64,
            metadata: serde_json::json!({"source": "system_seed"}),
        };
        assert_eq!(memory_time_label(&plain, now, "zh"), "预置背景");
        let backstory = crate::memory::MemoryItem {
            id: "seed_2".into(),
            timestamp: (now - chrono::Duration::days(30)).timestamp() as f64,
            metadata: serde_json::json!({"source": "system_seed", "fictional_backstory": true}),
        };
        let label = memory_time_label(&backstory, now, "zh");
        assert!(label.starts_with("前史设定时间："), "{label}");
    }
}
`);
  const manifest = path.join(dir, 'Cargo.toml');
  await writeFile(manifest, '[package]\nname="temporal-awareness-fixture"\nversion="0.0.0"\nedition="2021"\n[dependencies]\nchrono="0.4"\nserde_json="1"\n[[bin]]\nname="temporal-awareness-fixture"\npath="main.rs"\n');
  const run = spawnSync('cargo', ['test', '--manifest-path', manifest], {
    encoding: 'utf8',
    env: { ...process.env, CARGO_PROFILE_DEV_DEBUG: '0', CARGO_BUILD_JOBS: '2' },
  });
  assert.equal(run.status, 0, run.stdout + run.stderr);
  const output = run.stdout.trim();
  // Each contract must actually run: a module that stops compiling its own
  // tests would otherwise still report a green summary.
  for (const name of [
    'session_compressor::tests::recap_range_keeps_old_segment_separate_from_current_turn',
    'temporal_consistency::tests::old_conversation_cannot_be_called_just_now',
    'temporal_consistency::tests::fresh_conversation_and_quotes_are_preserved',
    'guard_tests::near_duplicate_ignores_punctuation_and_case_noise',
    'guard_tests::near_duplicate_does_not_suppress_a_different_subject',
    'guard_tests::recall_labels_share_local_boundary_and_language',
    'guard_tests::authored_background_never_reports_a_relative_age',
    'prompt_time::tests::raw_timestamps_share_the_same_boundary_and_language',
  ]) {
    assert.ok(new RegExp(`${name} \\.\\.\\. ok`).test(output), `${name} did not run:\n${output}`);
  }
  console.log(output);
} finally {
  await rm(dir, { recursive: true, force: true });
}
