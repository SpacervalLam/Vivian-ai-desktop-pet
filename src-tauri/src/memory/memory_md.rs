//! 角色长期记忆笔记（memory.md）
//!
//! 每个角色在应用数据目录下维护一份 markdown 长期记忆笔记：
//! `%APPDATA%\Vivian\characters\<char_id>\memory\memory.md`（按角色隔离）。
//!
//! 与结构化记忆库（`unified_memory.json` + 向量检索）互补：
//! - 结构化记忆库负责「按查询检索记忆条目」；
//! - 这里负责「每轮全量注入上下文的相处约定」，只收四类内容：
//!   相处约定 / 角色许下的承诺 / 相处中的教训 / 只有彼此懂的梗。
//!   用户的事实与偏好不在这里（由 user_facts / user_model 承载）。
//!
//! 读写链路（与注入预算协同，保证注入侧永不截断）：
//! - **注入**：每轮由 PromptBuildingStep 全量读取（`read_memory_md`）；
//! - **沉淀**：反思步（reflection）在每轮回复后产出 `memory_note` 字段，
//!   经 `append_memory_md` 按日期分节追加——对话中零工具调用，不打断沉浸感；
//! - **预算硬不变量**：追加后若超过 [`MEMORY_MD_MAX_CHARS`]，从最旧的日期分节起
//!   整节驱逐，使文件长度始终 ≤ 上限——注入侧因此永远读到全文，不存在截断损失；
//! - **整理**：睡眠巩固窗口在行数超阈值时用 LLM rewrite 合并去重（机械整理，
//!   不占对话路径）；`memory_md` 工具仅作用户明说查看/整理时的手动入口。

/// 注入 prompt 的最大字符数（写侧驱逐保证文件恒 ≤ 此值，注入侧正常永不触发截断；
/// 保留读侧尾部截断仅作手工编辑文件等旁路场景的安全网）。
pub const MEMORY_MD_MAX_CHARS: usize = 2000;

/// 睡眠整理触发阈值（行数超过则巩固窗口跑一次 rewrite 合并去重）。
pub const MEMORY_MD_TIDY_LINES: usize = 60;

/// memory.md 首次创建时写入的文件头（说明用途与维护方式）。
pub const MEMORY_MD_HEADER: &str = "# 长期记忆笔记\n\n\
    > 本文件随对话自动沉淀（只记相处约定、承诺、教训和梗），睡眠期自动整理合并。\n\
    > 每次对话全量注入上下文，是与主人相处的长期约定。\n";

/// 角色长期记忆笔记文件路径：`<角色数据目录>/memory/memory.md`。
pub fn memory_md_path(char_id: &str) -> std::path::PathBuf {
    crate::utils::path::get_character_data_dir(char_id)
        .join("memory")
        .join("memory.md")
}

/// 读取 memory.md 全文（不做截断，重写/工具读写用）。空文件视为 None。
pub fn read_memory_md_raw(char_id: &str) -> Option<String> {
    if char_id.trim().is_empty() {
        return None;
    }
    let text = std::fs::read_to_string(memory_md_path(char_id)).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 读取 memory.md（注入用）。超长保留尾部——最新沉淀的条目。
///
/// 写侧驱逐保证正常运行时文件恒 ≤ [`MEMORY_MD_MAX_CHARS`]，此处的截断仅覆盖
/// 用户手工编辑文件等旁路场景，属安全网而非常规路径。
pub fn read_memory_md(char_id: &str) -> Option<String> {
    let full = read_memory_md_raw(char_id)?;
    let chars: Vec<char> = full.chars().collect();
    if chars.len() <= MEMORY_MD_MAX_CHARS {
        return Some(full);
    }
    let tail: String = chars[chars.len() - MEMORY_MD_MAX_CHARS..].iter().collect();
    Some(format!("（较早内容已截断）\n{tail}"))
}

/// 是否需要睡眠整理（行数超阈值——碎片化的分节积累到该合并了）。
pub fn needs_tidy(char_id: &str) -> bool {
    match read_memory_md_raw(char_id) {
        Some(text) => text.lines().count() > MEMORY_MD_TIDY_LINES,
        None => false,
    }
}

/// 全量重写 memory.md（文件头 + 正文）。
///
/// 重写结果超过 [`MEMORY_MD_MAX_CHARS`] 时**拒绝**而非静默截断——
/// 全文重写的正文结构由调用方组织，截断会破坏其语义；调用方应精简后重试。
/// 组装重写全文（文件头 + 正文）并校验预算。
///
/// 重写结果超过 [`MEMORY_MD_MAX_CHARS`] 时**拒绝**而非静默截断——
/// 全文重写的正文结构由调用方组织，截断会破坏其语义；调用方应精简后重试。
fn compose_memory_md(body: &str) -> Result<String, String> {
    let mut content = MEMORY_MD_HEADER.to_string();
    let body = body.trim();
    if !body.is_empty() {
        content.push('\n');
        content.push_str(body);
        content.push('\n');
    }
    let count = content.chars().count();
    if count > MEMORY_MD_MAX_CHARS {
        return Err(format!(
            "重写后的笔记为 {count} 字符，超过上限 {MEMORY_MD_MAX_CHARS}，请合并去重后精简重试"
        ));
    }
    Ok(content)
}

pub fn write_memory_md(char_id: &str, body: &str) -> Result<(), String> {
    if char_id.trim().is_empty() {
        return Err("缺少角色 ID".into());
    }
    let content = compose_memory_md(body)?;
    let path = memory_md_path(char_id);
    if let Some(parent) = path.parent() {
        crate::utils::path::ensure_dir(parent).map_err(|e| format!("创建记忆目录失败：{e}"))?;
    }
    crate::utils::fs::write_atomic(&path, &content)
        .map_err(|e| format!("写入 memory.md 失败：{e}"))
}

/// 追加一段内容到 memory.md（不存在则带说明头创建，按日期分节）。
///
/// 追加后若超过 [`MEMORY_MD_MAX_CHARS`]，从**最旧的日期分节**起整节驱逐
/// （分节按追加顺序天然时间升序），直到回到上限内——最旧分节里的内容
/// 也是睡眠整理时最可能被合并/淘汰的，机械驱逐与整理语义一致。
pub fn append_memory_md(char_id: &str, body: &str) -> Result<(), String> {
    if char_id.trim().is_empty() {
        return Err("缺少角色 ID".into());
    }
    let body = body.trim();
    if body.is_empty() {
        return Ok(());
    }
    let path = memory_md_path(char_id);
    if let Some(parent) = path.parent() {
        crate::utils::path::ensure_dir(parent).map_err(|e| format!("创建记忆目录失败：{e}"))?;
    }
    let mut content = std::fs::read_to_string(&path).unwrap_or_default();
    if content.trim().is_empty() {
        content = MEMORY_MD_HEADER.to_string();
    }
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&format!(
        "\n## {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M")
    ));
    content.push_str(body);
    content.push('\n');

    let (evicted, content) = enforce_char_budget(content);
    if evicted > 0 {
        tracing::info!(
            "[memory_md] 追加后超上限，已驱逐最旧的 {evicted} 个日期分节（睡眠整理时会合并剩余内容）"
        );
    }
    crate::utils::fs::write_atomic(&path, &content)
        .map_err(|e| format!("写入 memory.md 失败：{e}"))
}

/// 字符预算执行器：超限时从最旧的 `## ` 分节起整节驱逐。
///
/// 返回 (驱逐分节数, 收敛后的内容)。极端兜底（文件头 + 单个分节仍超限，
/// 仅当单次追加本身巨长时出现）保留最新分节的尾部并加截断标记。
fn enforce_char_budget(content: String) -> (usize, String) {
    if content.chars().count() <= MEMORY_MD_MAX_CHARS {
        return (0, content);
    }
    const MARKER: &str = "\n## ";
    let head_end = content.find(MARKER).unwrap_or(content.len());
    let (head, rest) = content.split_at(head_end);
    let mut sections: Vec<String> = rest
        .split(MARKER)
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("{MARKER}{s}"))
        .collect();

    let mut evicted = 0;
    loop {
        let joined = format!("{head}{}", sections.concat());
        if joined.chars().count() <= MEMORY_MD_MAX_CHARS || sections.len() <= 1 {
            break;
        }
        sections.remove(0);
        evicted += 1;
    }
    let mut result = format!("{head}{}", sections.concat());
    if result.chars().count() > MEMORY_MD_MAX_CHARS {
        // 单节仍超限：保尾部（最新内容），标记截断
        let chars: Vec<char> = result.chars().collect();
        let keep = MEMORY_MD_MAX_CHARS.saturating_sub(20);
        let tail: String = chars[chars.len() - keep..].iter().collect();
        result = format!("{head}\n## （更早内容因超长被截断）\n{tail}");
        if evicted == 0 {
            evicted = 1;
        }
    }
    (evicted, result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforce_budget_evicts_oldest_sections() {
        let mut content = String::from("# 长期记忆笔记\n");
        for i in 0..5 {
            content.push_str(&format!("\n## 2026-09-0{i} 10:00\n- 条目 {i} "));
            content.push_str(&"很长的内容".repeat(200));
        }
        let (evicted, out) = enforce_char_budget(content);
        assert!(evicted > 0);
        assert!(out.chars().count() <= MEMORY_MD_MAX_CHARS);
        // 最旧分节被驱逐，最新分节保留
        assert!(!out.contains("条目 0"));
        assert!(out.contains("条目 4"));
        // 文件头保留
        assert!(out.starts_with("# 长期记忆笔记"));
    }

    #[test]
    fn enforce_budget_noop_within_limit() {
        let content = String::from("# 长期记忆笔记\n\n## 2026-09-01\n- 短条目\n");
        let (evicted, out) = enforce_char_budget(content.clone());
        assert_eq!(evicted, 0);
        assert_eq!(out, content);
    }

    #[test]
    fn write_compose_rejects_over_budget() {
        let body = "长".repeat(MEMORY_MD_MAX_CHARS);
        let err = compose_memory_md(&body).unwrap_err();
        assert!(err.contains("超过上限"));
        assert!(compose_memory_md("## 约定\n- 周五一起看番").is_ok());
    }
}
