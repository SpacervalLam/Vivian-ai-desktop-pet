//! 登录态被动信号采集 — 经浏览器桥执行固定平台请求
//!
//! 模型：不导航、不劫持标签页。仅当受控标签页**正好停在**目标平台域名时，
//! 由内嵌浏览器执行固定历史 API 请求（凭据保留在浏览器），
//! 读取观看历史等画像信号（登录态被动采集路径）。
//!
//! 采集到的历史标题经 LLM 提炼为兴趣域，写回兴趣画像（登录态画像初始化）。


use crate::discovery::profile::InterestProfile;

/// 读取 B站观看历史：内嵌浏览器验证当前标签域名后执行固定请求，不运行动态 JS。
pub async fn collect_bilibili_history(limit: usize) -> Vec<String> {
    let Some(bridge) = crate::browser_bridge::tools::global_bridge() else { return Vec::new(); };
    if !bridge.is_connected() { return Vec::new(); }
    let Ok(text) = bridge.request_tool(
        "browser_bilibili_history",
        &serde_json::json!({ "limit": limit.clamp(1, 100) }),
        std::time::Duration::from_secs(15), Some("discovery-passive"),
    ).await else { return Vec::new(); };
    parse_history_titles(&text)
}

fn parse_history_titles(text: &str) -> Vec<String> {
    // Extension results use the same untrusted wrapper as page extraction.
    let text = text.trim();
    let text = if text.starts_with("<UNTRUSTED_PAGE_CONTENT") {
        match (text.find('>'), text.rfind("</UNTRUSTED_PAGE_CONTENT>")) {
            (Some(start), Some(end)) if start < end => text[start + 1..end].trim(),
            _ => return Vec::new(),
        }
    } else { text };
    let Some(parsed) = crate::discovery::parse_json_tolerant(text) else { return Vec::new(); };
    parsed
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    e.get("title")
                        .and_then(|t| t.as_str())
                        .map(|t| t.trim().to_string())
                        .filter(|t| !t.is_empty())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 历史标题 → LLM 兴趣域提炼 → 写回画像（登录态画像初始化）
///
/// 返回新增/强化的兴趣域列表；无桥连接 / 不在 B 站 / LLM 失败时静默返回空。
pub async fn refresh_interests_from_history(char_id: &str) -> Vec<String> {
    let titles = collect_bilibili_history(50).await;
    if titles.is_empty() {
        return Vec::new();
    }

    let system = "你从用户的 B 站观看历史标题中提炼兴趣领域。\
只输出真正能从标题推断出的兴趣域（如「机械键盘」「量子物理科普」「城建模拟」），\
不要输出「视频」「娱乐」这类空泛词。输出严格 JSON，不要附带解释。";

    let title_lines: Vec<String> = titles
        .iter()
        .take(50)
        .map(|t| format!("- {}", t.chars().take(40).collect::<String>()))
        .collect();
    let user = format!(
        "## 观看历史标题（最近 {} 条）\n{}\n\n\
从标题中提炼 3-8 个兴趣域，按出现频度排序。\n\n\
严格输出 JSON：{{\"domains\":[\"...\",\"...\"]}}",
        titles.len(),
        title_lines.join("\n"),
    );

    let Some(content) = crate::discovery::llm_complete(system, &user, Some(0.4)).await else {
        return Vec::new();
    };
    let Some(value) = crate::discovery::parse_json_tolerant(&content) else {
        return Vec::new();
    };
    let domains: Vec<String> = value
        .get("domains")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|d| d.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty() && s.chars().count() <= 30)
                .take(8)
                .collect()
        })
        .unwrap_or_default();
    if domains.is_empty() {
        return Vec::new();
    }

    let mut profile = InterestProfile::load(char_id);
    for (idx, domain) in domains.iter().enumerate() {
        // 频度排序衰减：越靠前权重越高
        let weight = (0.9 - idx as f64 * 0.06).max(0.5);
        profile.upsert_interest(domain, weight, "history");
    }
    profile.save(char_id);
    tracing::info!(
        "[discovery:{}] 登录态历史采集：从 {} 条观看记录提炼 {} 个兴趣域: {:?}",
        char_id,
        titles.len(),
        domains.len(),
        &domains
    );
    domains
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_history_titles() {
        let json = r#"[{"title":"机械键盘入门指南","author":"up"},{"title":"","author":"x"}]"#;
        let titles = parse_history_titles(json);
        assert_eq!(titles, vec!["机械键盘入门指南".to_string()]);
    }

    #[test]
    fn test_parse_history_titles_double_wrapped() {
        // eval 的 safeSerialize：字符串值会再 JSON.stringify 一次
        let inner = r#"[{"title":"量子物理科普"}]"#.to_string();
        let wrapped = serde_json::to_string(&inner).unwrap();
        let titles = parse_history_titles(&wrapped);
        assert_eq!(titles, vec!["量子物理科普".to_string()]);
    }

    #[test]
    fn test_parse_history_titles_untrusted_page_wrapper() {
        let text = "<UNTRUSTED_PAGE_CONTENT nonce=\"test\">\n[{\"title\":\"视频标题\"}]\n</UNTRUSTED_PAGE_CONTENT>";
        assert_eq!(parse_history_titles(text), vec!["视频标题"]);
    }
}
