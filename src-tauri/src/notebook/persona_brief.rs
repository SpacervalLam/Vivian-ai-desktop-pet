//! 笔记专用人设精简版 —— 把 `char_id` 的人设压成一小段可拼进工具描述的文本。
//!
//! 为什么需要它：笔记由智能体代写，若只说「用你的口吻写」，模型没有具体锚点，
//! 很容易退回客观报告腔。把人设里**与文风直接相关**的部分给出来，命中率才高。
//!
//! 为什么只取一部分：`PersonaConfig` 全量很大（外观、场景模式、决策权重、
//! 演化层…），拼进每次工具描述会显著推高 token，且大部分与「怎么写笔记」无关。
//! 这里只取 4 类：
//!
//! 1. **身份**（`identity.name` / `identity.tagline`）—— 署名与基本定位
//! 2. **口癖与句式**（`language_style`）—— 最直接决定笔迹
//! 3. **表达倾向**（`expression` 的数值 → 文字描述）—— 决定语气浓度
//! 4. **写作禁令**（`identity.taboos`）—— 客服腔、动作描述等硬红线
//!
//! 数值维度必须**转成文字**再给模型：直接给 `tsundere: 0.3` 毫无意义，
//! 模型无法把它映射到具体用词。转成「有点毒舌但不过分」才是可执行的指令。

use crate::persona::schemas::{
    default_persona_for, CharacterExpression, IdentityLayer, PersonaConfig,
};

/// 笔记人设精简版的自然语言上限。
///
/// 控制在 400 字符内：工具描述每轮都会进 prompt，这段要短到可以忽略。
const BRIEF_CHAR_BUDGET: usize = 400;

/// 读取指定角色的 `PersonaConfig`（供笔记人设提炼）。
///
/// 刻意**不用** `PersonaEngine::new`——那个构造函数会 `create_dir_all` 写磁盘，
/// 工具层拿个描述文本不该有文件副作用。直接读 `persona.json`，失败回退出厂默认。
pub fn load_persona_for_notes(char_id: &str) -> PersonaConfig {
    let path = crate::utils::path::get_character_data_dir(char_id).join("persona/persona.json");
    // 用户可能没保存过 persona.json（当前实测就是这种情况），此时用出厂默认
    let Ok(content) = std::fs::read_to_string(&path) else {
        return default_persona_for(char_id);
    };
    if content.trim().is_empty() {
        return default_persona_for(char_id);
    }
    match serde_json::from_str::<PersonaConfig>(&content) {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::warn!("[Notebook] 解析 persona.json 失败，回退默认人设: {e}");
            default_persona_for(char_id)
        }
    }
}

/// 表达倾向的一条结论（双语成对，判定一次产出两句）。
///
/// 刻意不做成「先出中文、再按中文匹配回英文」—— 那样靠字符串相等来判断语义，
/// 改一个字就静默失配。这里让判定函数直接产出两种语言。
struct TraitLine {
    zh: &'static str,
    en: &'static str,
}

/// 数值表达维度 → 可执行的写法指引（双语）。
///
/// 阈值分档而不是线性插值：模型对「0.55」无感，对「有点毒舌」有感。
/// 每档给一个**可执行**的写法提示，而不只是形容词。
fn expression_traits(expr: &CharacterExpression) -> Vec<TraitLine> {
    let mut out = Vec::new();
    // 毒舌：轻度吐槽，绝不刻薄
    if expr.sass >= 0.6 {
        out.push(TraitLine {
            zh: "可以适度吐槽，毒舌但绝不刻薄",
            en: "mild snark is fine, never mean",
        });
    } else if expr.sass < 0.3 {
        out.push(TraitLine {
            zh: "不用毒舌，语气平和",
            en: "keep the tone even, no snark",
        });
    }
    // 傲娇：把关心藏在强硬外表后
    if expr.tsundere >= 0.6 {
        out.push(TraitLine {
            zh: "关心藏在别扭的语气后面，不要直白说在乎",
            en: "care shows through a slightly awkward tone, never state it outright",
        });
    } else if expr.tsundere < 0.25 {
        out.push(TraitLine {
            zh: "不傲娇，想说什么就说什么",
            en: "straightforward, no tsundere",
        });
    }
    // 元气：活力感染力
    if expr.genki >= 0.7 {
        out.push(TraitLine {
            zh: "语气有活力、有热情",
            en: "lively and warm in tone",
        });
    } else if expr.genki < 0.4 {
        out.push(TraitLine {
            zh: "安静克制，不用感叹号堆情绪",
            en: "quiet and restrained, no exclamation marks",
        });
    }
    // 治愈：安慰时多温暖走心
    if expr.healing >= 0.7 {
        out.push(TraitLine {
            zh: "涉及对方难处时偏温暖走心",
            en: "warm and sincere when the user is struggling",
        });
    }
    // 好奇：对用户生活多感兴趣
    if expr.curiosity >= 0.7 {
        out.push(TraitLine {
            zh: "可以表达对自己观察到的现象的好奇",
            en: "feel free to show curiosity about what you notice",
        });
    }
    out
}

/// 写作相关的禁令（taboos）。
///
/// 只挑**与笔迹直接相关**的：客服腔、动作描述。名字锁、场景类的对写笔记无约束力。
/// 按 `prompt_instruction` 的关键词匹配而非按 description（description 是英文标签，
/// 如 "No customer-service speech"，对不上就别硬匹配）。
fn writing_taboos(identity: &IdentityLayer) -> Vec<TraitLine> {
    let mut out = Vec::new();
    for rule in &identity.taboos {
        if !rule.enabled {
            continue;
        }
        let instr = rule.prompt_instruction.to_lowercase();
        if instr.contains("customer service") || instr.contains("assistant") {
            out.push(TraitLine {
                zh: "不要客服腔、不要「有什么可以帮您」这类话",
                en: "no customer-service speak",
            });
        } else if instr.contains("action descriptions") || instr.contains("actions like") {
            out.push(TraitLine {
                zh: "只写文字，不要写（递茶）、*微笑* 这类动作描写",
                en: "no action descriptions like (hands over tea)",
            });
        }
    }
    out.truncate(2);
    out
}

/// 生成笔记专用人设精简版（中文，用于简中工具描述）。
pub fn brief_zh(char_id: &str) -> String {
    let cfg = load_persona_for_notes(char_id);
    brief_from_config(&cfg, Lang::Zh)
}

/// 生成笔记专用人设精简版（英文，用于英文工具描述）。
pub fn brief_en(char_id: &str) -> String {
    let cfg = load_persona_for_notes(char_id);
    brief_from_config(&cfg, Lang::En)
}

/// 列出已知的角色 ID。
///
/// 工具描述是**全局单例**（`CreateNotebookTool::new()` 不带角色上下文），
/// 而人设按 `char_id` 变化。静态描述无法内嵌单个角色的人设，
/// 所以工具描述里把两个角色的文风**并列**给出，由模型按当前角色认领。
/// 这份清单要保持权威，不能在别处硬编码角色名字符串。
pub fn known_char_ids() -> Vec<&'static str> {
    vec!["vivian", "nana"]
}

/// 全部已知角色的笔记人设精简版（中文），用 ` / ` 分隔。
pub fn all_brief_zh() -> String {
    all_brief(Lang::Zh)
}

/// 全部已知角色的笔记人设精简版（英文），用 ` / ` 分隔。
pub fn all_brief_en() -> String {
    all_brief(Lang::En)
}

fn all_brief(lang: Lang) -> String {
    known_char_ids()
        .iter()
        .map(|id| brief_from_config(&load_persona_for_notes(id), lang))
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" / ")
}

/// 内部实现：按语言把 PersonaConfig 压成一段文本。
fn brief_from_config(cfg: &PersonaConfig, lang: Lang) -> String {
    let name = cfg.identity.name.trim();
    if name.is_empty() {
        return String::new();
    }

    let mut parts: Vec<String> = Vec::new();

    match lang {
        Lang::Zh => {
            // tagline 在出厂配置里是英文（"A weeb netizen who lives online…"）。
            // 混进中文句子只会变成噪音——真正决定文风的是下面的表达倾向与禁令，
            // 所以中文版只报名字，不硬译 tagline。
            parts.push(format!("你是{name}。"));
        }
        Lang::En => {
            let tagline = first_sentence_en(&cfg.identity.tagline, 80);
            if tagline.is_empty() {
                parts.push(format!("You are {name}."));
            } else {
                parts.push(format!("You are {name} — {tagline}"));
            }
        }
    }

    // 口癖：最多 2 个，多了会让笔记变成口头禅堆砌
    let phrases: Vec<&str> = cfg
        .language_style
        .catchphrases
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .take(2)
        .collect();
    if !phrases.is_empty() {
        match lang {
            Lang::Zh => parts.push(format!("常用语气词：{}", phrases.join("、"))),
            Lang::En => parts.push(format!("Frequent interjections: {}", phrases.join(", "))),
        }
    }

    let sep = match lang {
        Lang::Zh => "；",
        Lang::En => "; ",
    };

    let traits = expression_traits(&cfg.expression);
    if !traits.is_empty() {
        let text = traits
            .iter()
            .map(|t| match lang {
                Lang::Zh => t.zh,
                Lang::En => t.en,
            })
            .collect::<Vec<_>>()
            .join(sep);
        parts.push(text);
    }

    let taboos = writing_taboos(&cfg.identity);
    if !taboos.is_empty() {
        let text = taboos
            .iter()
            .map(|t| match lang {
                Lang::Zh => t.zh,
                Lang::En => t.en,
            })
            .collect::<Vec<_>>()
            .join(sep);
        parts.push(text);
    }

    // 分隔符要跟语言走：中文用「；」，英文用 "; "。
    // 混用会让英文描述里出现中文分号，模型读起来是乱码般的噪音。
    let mut text = parts.join(sep);
    text.truncate(BRIEF_CHAR_BUDGET);
    text
}

/// 语言分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    Zh,
    En,
}

/// 取英文文本的第一句（句号）。/// 取英文文本的第一句（句号）。
fn first_sentence_en(text: &str, max: usize) -> String {
    let t = text.trim();
    if t.is_empty() {
        return String::new();
    }
    let cut = t
        .char_indices()
        .find(|(i, c)| (*c == '.' || *c == ',') && *i > 10)
        .map(|(i, _)| &t[..i])
        .unwrap_or(t);
    truncate_chars(cut, max)
}

/// 按字符数截断（不用 byte，避免切坏多字节字符）。
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brief_names_the_character() {
        let brief = brief_zh("vivian");
        assert!(brief.contains("Vivian"), "应含角色名: {brief}");
    }

    #[test]
    fn brief_fits_the_budget() {
        // 人设精简版每轮都进 prompt，超预算就失去意义
        for id in ["vivian", "nana"] {
            for lang in [Lang::Zh, Lang::En] {
                let brief = brief_from_config(&load_persona_for_notes(id), lang);
                assert!(
                    brief.chars().count() <= BRIEF_CHAR_BUDGET,
                    "{} {:?} 超出预算: {} 字符",
                    id,
                    lang,
                    brief.chars().count()
                );
                assert!(!brief.trim().is_empty(), "{} 不该为空", id);
            }
        }
    }

    #[test]
    fn numeric_traits_become_actionable_words() {
        // 关键：数值不能直接给模型，必须转成可执行的写法
        let expr = CharacterExpression {
            tsundere: 0.7,
            clingy: 0.5,
            genki: 0.8,
            sass: 0.7,
            healing: 0.5,
            curiosity: 0.5,
            ritual: 0.5,
            habit_awareness: 0.5,
        };
        let traits = expression_traits(&expr);
        let joined = traits.iter().map(|t| t.zh).collect::<Vec<_>>().join("；");
        assert!(joined.contains("毒舌") || joined.contains("别扭"), "{joined}");
        assert!(joined.contains("活力"), "{joined}");
        // 数值本身不该出现在输出里
        assert!(!joined.contains("0.7") && !joined.contains("0.8"), "{joined}");
    }

    #[test]
    fn low_traits_produce_the_opposite_instruction() {
        // 反面对照：低数值必须给相反的引导，不能沿用高数值的描述
        let expr = CharacterExpression {
            tsundere: 0.1,
            clingy: 0.5,
            genki: 0.2,
            sass: 0.1,
            healing: 0.5,
            curiosity: 0.5,
            ritual: 0.5,
            habit_awareness: 0.5,
        };
        let joined = expression_traits(&expr).iter().map(|t| t.zh).collect::<Vec<_>>().join("；");
        assert!(joined.contains("不傲娇"), "{joined}");
        assert!(joined.contains("不用毒舌"), "{joined}");
        assert!(joined.contains("安静克制"), "{joined}");
    }

    #[test]
    fn customer_service_taboos_are_included() {
        let cfg = load_persona_for_notes("vivian");
        let taboos = writing_taboos(&cfg.identity);
        let joined = taboos.iter().map(|t| t.zh).collect::<Vec<_>>().join("；");
        assert!(joined.contains("客服腔"), "应含客服腔禁令: {joined}");
    }

    #[test]
    fn missing_persona_file_falls_back_to_defaults() {
        // 不存在的角色 → default_persona_for 仍返回出厂配置，不 panic
        let cfg = load_persona_for_notes("不存在的角色");
        let brief = brief_from_config(&cfg, Lang::Zh);
        assert!(!brief.is_empty(), "回退出厂人设后仍应产出文本");
    }

    #[test]
    fn english_brief_is_produced() {
        let brief = brief_en("vivian");
        assert!(brief.contains("Vivian"), "{brief}");
        assert!(
            !brief.contains("；"),
            "英文版不该残留中文分号: {brief}"
        );
    }
}
