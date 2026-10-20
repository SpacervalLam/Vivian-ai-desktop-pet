// 验证语气规则三语分发：从生产 prompt_modules.rs 切片提取 normalize_lang +
// tone_rule + 10 个包装函数（include_str! 路径重写为绝对路径），在 mini-crate 里
// 跑真实分发断言。漂移守卫：切片锚点找不到 → 立刻失败。
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const dir = path.join(root, 'tmp', 'tone-rule-tests');
const src = path.join(root, 'src-tauri', 'src', 'pipeline', 'prompt_modules.rs');
const framework = path.join(root, 'src-tauri', 'prompts', 'framework');
await mkdir(path.join(dir, 'src'), { recursive: true });

const text = (await readFile(src, 'utf8')).replace(/\r\n/g, '\n');

// 切片 1：normalize_lang
const nlStart = text.indexOf('pub fn normalize_lang');
const nlEnd = text.indexOf('\n}\n', nlStart) + 3;
if (nlStart < 0 || nlEnd < 3) throw new Error('normalize_lang slice not found');
const normalize = text.slice(nlStart, nlEnd);

// 切片 2：tone_rule + 全部包装函数（到 inner_monologue_rules 结束）
const trStart = text.indexOf('pub fn tone_rule');
const trEndMarker = 'pub fn inner_monologue_rules(lang: &str) -> &\'static str {\n    tone_rule(lang, "inner_monologue")\n}\n';
const trEnd = text.indexOf(trEndMarker, trStart) + trEndMarker.length;
if (trStart < 0 || trEnd < trEndMarker.length) throw new Error('tone_rule slice not found');
let dispatchers = text.slice(trStart, trEnd);

// include_str! 相对路径 → 绝对路径（mini-crate 的 CARGO_MANIFEST_DIR 不同）
const rewritten = dispatchers.replaceAll('../../prompts/framework/', framework.replaceAll('\\', '/') + '/');
if (!rewritten.includes(framework.replaceAll('\\', '/'))) throw new Error('include path rewrite failed');

await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-tone-rule-tests"
version="0.1.0"
edition="2021"
[dependencies]
`);

await writeFile(path.join(dir, 'src', 'lib.rs'), `#![allow(dead_code)]
${normalize}
${rewritten}

#[cfg(test)]
mod dispatch {
    use super::*;
    const BASES: &[&str] = &["chat_style", "human_feel", "conversation_rhythm",
        "companion_dialogue", "companion_post_history", "conversation_focus",
        "first_contact", "proactive_companionship", "initiative", "inner_monologue"];
    // 每个 base 的逐语锚点（initiative 用标题而非方括号标签）
    const TAGS: &[(&str, &str, &str, &str)] = &[
        ("chat_style", "[CHAT_STYLE_RULES]", "[CHAT_STYLE_RULES]", "[CHAT_STYLE_RULES]"),
        ("human_feel", "[HUMAN_FEEL_RULES]", "[HUMAN_FEEL_RULES]", "[HUMAN_FEEL_RULES]"),
        ("conversation_rhythm", "[RHYTHM_RULES]", "[RHYTHM_RULES]", "[RHYTHM_RULES]"),
        ("companion_dialogue", "[COMPANION DIALOGUE]", "[COMPANION DIALOGUE]", "[COMPANION DIALOGUE]"),
        ("companion_post_history", "[NEXT TURN]", "[NEXT TURN]", "[NEXT TURN]"),
        ("conversation_focus", "[CURRENT COMPANION TURN]", "[CURRENT COMPANION TURN]", "[CURRENT COMPANION TURN]"),
        ("first_contact", "[FIRST CONTACT GREETING]", "[FIRST CONTACT GREETING]", "[FIRST CONTACT GREETING]"),
        ("proactive_companionship", "[PROACTIVE COMPANIONSHIP]", "[PROACTIVE COMPANIONSHIP]", "[PROACTIVE COMPANIONSHIP]"),
        ("initiative", "Helpful initiative", "主动性与善后", "積極性"),
        ("inner_monologue", "[PRIVATE THOUGHT RECORD]", "[PRIVATE THOUGHT RECORD]", "[PRIVATE THOUGHT RECORD]"),
    ];

    #[test]
    fn normalize_lang_maps_and_defaults_to_zh() {
        assert_eq!(normalize_lang("zh-CN"), "zh");
        assert_eq!(normalize_lang("ja-JP"), "ja");
        assert_eq!(normalize_lang("en-US"), "en");
        assert_eq!(normalize_lang("fr"), "zh");
    }

    #[test]
    fn every_base_resolves_in_every_language_with_symmetric_tags() {
        for (base, en, zh, ja) in TAGS {
            for (lang, anchor) in [("en", *en), ("zh", *zh), ("ja", *ja)] {
                let rule = tone_rule(lang, base);
                assert!(!rule.is_empty(), "{base}/{lang} resolved empty");
                assert!(rule.contains(anchor), "{base}/{lang} missing anchor {anchor}");
            }
        }
    }

    #[test]
    fn dispatch_returns_language_specific_content_not_english_fallback() {
        assert!(tone_rule("zh", "chat_style").contains("聊天风格"));
        assert!(tone_rule("ja", "chat_style").contains("チャットスタイル"));
        assert!(tone_rule("en", "chat_style").contains("Chat Style"));
        assert!(tone_rule("zh", "human_feel").contains("说话的方式"));
        assert!(tone_rule("ja", "human_feel").contains("話し方"));
        assert!(tone_rule("zh", "conversation_rhythm").contains("对话节奏"));
        assert!(tone_rule("ja", "conversation_rhythm").contains("会話のリズム"));
        // 工具标识符不翻译
        assert!(tone_rule("zh", "companion_dialogue").contains("get_weather_forecast"));
        assert!(tone_rule("ja", "companion_dialogue").contains("get_weather_forecast"));
        assert!(tone_rule("zh", "initiative").contains("delegate_to_work_agent"));
        // 未知 base 返回空串而不是英文兜底（调用方应只传已知 base）
        assert_eq!(tone_rule("zh", "nope"), "");
    }

    #[test]
    fn wrappers_match_dispatch() {
        assert_eq!(chat_style_framework("zh"), tone_rule("zh", "chat_style"));
        assert_eq!(human_feel_rules("ja"), tone_rule("ja", "human_feel"));
        assert_eq!(conversation_rhythm("en"), tone_rule("en", "conversation_rhythm"));
        assert_eq!(companion_dialogue_rules("zh"), tone_rule("zh", "companion_dialogue"));
        assert_eq!(companion_post_history_rules("ja"), tone_rule("ja", "companion_post_history"));
        assert_eq!(conversation_focus_rules("zh"), tone_rule("zh", "conversation_focus"));
        assert_eq!(first_contact_rules("ja"), tone_rule("ja", "first_contact"));
        assert_eq!(proactive_companionship_rules("zh"), tone_rule("zh", "proactive_companionship"));
        assert_eq!(initiative_rules("ja"), tone_rule("ja", "initiative"));
        assert_eq!(inner_monologue_rules("zh"), tone_rule("zh", "inner_monologue"));
    }
}
`);

const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1', '--', '--test-threads=1'], {
  cwd: root, stdio: 'inherit',
  env: { ...process.env, CARGO_INCREMENTAL: '0', CARGO_PROFILE_DEV_DEBUG: '0' },
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
