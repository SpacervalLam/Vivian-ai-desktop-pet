//! 技能校验：front-matter、命名、占位符与目录卫生检查。
//!
//! 三处共用同一套规则：`create_skill` 工具写入前自检、`use_skill` 的 validate 动作、
//! 设置页命令保存前校验。`error` 阻断写入，`warning` 只作提示。

use std::path::Path;

use serde::Serialize;

use super::{front_matter_block, is_valid_skill_name, parse_skill_doc};

/// 描述长度上限（超过只提示，不阻断）。
const MAX_DESCRIPTION_CHARS: usize = 200;
/// 未完成占位符标记。
const PLACEHOLDERS: &[&str] = &["TODO", "TBD", "FIXME", "PLACEHOLDER", "XXX"];
/// 允许出现的顶层 front-matter 键。
const KNOWN_KEYS: &[&str] = &["name", "description", "keywords"];

/// 单条校验问题。
#[derive(Debug, Clone, Serialize)]
pub struct SkillIssue {
    /// `error` 阻断写入；`warning` 仅提示。
    pub level: String,
    pub code: String,
    pub message: String,
}

/// 校验结果。
#[derive(Debug, Clone, Serialize)]
pub struct SkillValidation {
    pub ok: bool,
    pub issues: Vec<SkillIssue>,
}

fn error(code: &str, message: impl Into<String>) -> SkillIssue {
    SkillIssue {
        level: "error".to_string(),
        code: code.to_string(),
        message: message.into(),
    }
}

fn warning(code: &str, message: impl Into<String>) -> SkillIssue {
    SkillIssue {
        level: "warning".to_string(),
        code: code.to_string(),
        message: message.into(),
    }
}

/// front-matter 的零缩进顶层键。
fn top_level_keys(front_matter: &str) -> Vec<String> {
    front_matter
        .lines()
        .filter(|line| !line.starts_with(' ') && !line.starts_with('\t'))
        .filter_map(|line| line.split_once(':').map(|(key, _)| key.trim().to_string()))
        .filter(|key| !key.is_empty())
        .collect()
}

/// 规范命名：仅小写字母、数字、连字符、下划线。
fn is_canonical_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// 校验技能文本。`fallback_name` 在 front-matter 未声明 `name` 时用作兜底技能名。
pub fn validate_skill_text(fallback_name: &str, content: &str) -> SkillValidation {
    let mut issues: Vec<SkillIssue> = Vec::new();
    let parsed = parse_skill_doc(content, fallback_name);
    let keys = front_matter_block(content).map(top_level_keys).unwrap_or_default();

    if keys.is_empty() {
        issues.push(error(
            "missing_front_matter",
            "缺少 front-matter：正文前需要 `--- name: … / description: … ---` 头部",
        ));
    } else if !keys.iter().any(|key| key == "name") {
        issues.push(error("missing_name", "front-matter 缺少 name"));
    }

    if !is_valid_skill_name(&parsed.name) {
        issues.push(error(
            "invalid_name",
            format!(
                "技能名「{}」不合法：仅允许字母、数字、下划线、连字符或中文，且不超过 64 字符",
                parsed.name
            ),
        ));
    } else if !is_canonical_name(&parsed.name) {
        issues.push(warning(
            "non_canonical_name",
            format!("技能名「{}」含大写或非 ASCII 字符，建议统一为小写字母/数字/连字符", parsed.name),
        ));
    }

    if parsed.description.trim().is_empty() {
        issues.push(error("missing_description", "缺少 description（一句话说明技能用途与适用时机）"));
    } else if parsed.description.chars().count() > MAX_DESCRIPTION_CHARS {
        issues.push(warning(
            "long_description",
            format!("description 超过 {MAX_DESCRIPTION_CHARS} 字符，建议精简以便检索"),
        ));
    }

    if parsed.body.trim().is_empty() {
        issues.push(error("empty_body", "技能正文为空"));
    }

    for key in &keys {
        if !KNOWN_KEYS.contains(&key.as_str()) {
            issues.push(warning(
                "unknown_key",
                format!("front-matter 出现未知顶层键「{key}」（仅 name/description/keywords 生效）"),
            ));
        }
    }

    if let Some(hit) = PLACEHOLDERS.iter().find(|marker| parsed.body.contains(**marker)) {
        issues.push(warning(
            "placeholder",
            format!("正文仍含未完成占位符「{hit}」，交付前请替换或删除"),
        ));
    }

    let ok = !issues.iter().any(|issue| issue.level == "error");
    SkillValidation { ok, issues }
}

/// 校验磁盘上的技能目录：`<dir>/SKILL.md` + 目录卫生。
pub fn validate_skill_dir(dir: &Path) -> SkillValidation {
    let dir_name = dir.file_name().and_then(|name| name.to_str()).unwrap_or("skill");
    let skill_md = dir.join("SKILL.md");
    if !skill_md.is_file() {
        return SkillValidation {
            ok: false,
            issues: vec![error("missing_skill_md", "技能目录缺少 SKILL.md")],
        };
    }
    let content = match std::fs::read_to_string(&skill_md) {
        Ok(content) => content,
        Err(e) => {
            return SkillValidation {
                ok: false,
                issues: vec![error("unreadable_skill_md", format!("读取 SKILL.md 失败: {e}"))],
            };
        }
    };

    let mut validation = validate_skill_text(dir_name, &content);
    let parsed_name = parse_skill_doc(&content, dir_name).name;
    if parsed_name != dir_name {
        validation.issues.push(warning(
            "dir_name_mismatch",
            format!("技能名「{parsed_name}」与目录名「{dir_name}」不一致"),
        ));
    }
    for auxiliary in ["README.md", "CHANGELOG.md"] {
        if dir.join(auxiliary).is_file() {
            validation.issues.push(warning(
                "auxiliary_doc",
                format!("技能目录含 {auxiliary}：技能不应附带变更日志或重复的速查文档"),
            ));
        }
    }
    validation.ok = !validation.issues.iter().any(|issue| issue.level == "error");
    validation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_skill_passes_and_placeholder_is_warned() {
        let ok = validate_skill_text("demo", "---\nname: demo\ndescription: 演示技能\n---\n\n按步骤做\n");
        assert!(ok.ok, "{:?}", ok.issues);
        assert!(!ok.issues.iter().any(|issue| issue.level == "error"));

        let placeholder = validate_skill_text(
            "demo",
            "---\nname: demo\ndescription: 演示技能\n---\n\nTODO: 待补\n",
        );
        assert!(placeholder.ok, "占位符只提示不阻断");
        assert!(placeholder.issues.iter().any(|i| i.code == "placeholder"));
    }

    #[test]
    fn missing_front_matter_and_name_are_errors() {
        let no_fm = validate_skill_text("demo", "只有正文\n");
        assert!(!no_fm.ok);
        assert!(no_fm.issues.iter().any(|i| i.code == "missing_front_matter"));

        let no_name = validate_skill_text("demo", "---\ndescription: 无名称\n---\n\n正文\n");
        assert!(!no_name.ok);
        assert!(no_name.issues.iter().any(|i| i.code == "missing_name"));
    }

    #[test]
    fn invalid_and_non_canonical_names_are_flagged() {
        let invalid = validate_skill_text("demo", "---\nname: bad name!\ndescription: d\n---\n\n正文\n");
        assert!(!invalid.ok);
        assert!(invalid.issues.iter().any(|i| i.code == "invalid_name"));

        let upper = validate_skill_text("demo", "---\nname: Demo\ndescription: d\n---\n\n正文\n");
        assert!(upper.ok);
        assert!(upper.issues.iter().any(|i| i.code == "non_canonical_name"));
    }

    #[test]
    fn directory_validation_checks_name_match_and_auxiliary_docs() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("demo");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: 演示\n---\n\n正文\n",
        )
        .unwrap();
        let ok = validate_skill_dir(&skill_dir);
        assert!(ok.ok, "{:?}", ok.issues);

        std::fs::write(skill_dir.join("README.md"), "冗余说明").unwrap();
        let with_aux = validate_skill_dir(&skill_dir);
        assert!(with_aux.ok);
        assert!(with_aux.issues.iter().any(|i| i.code == "auxiliary_doc"));
    }

    #[test]
    fn missing_skill_md_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let validation = validate_skill_dir(&empty);
        assert!(!validation.ok);
        assert!(validation.issues.iter().any(|i| i.code == "missing_skill_md"));
    }
}
