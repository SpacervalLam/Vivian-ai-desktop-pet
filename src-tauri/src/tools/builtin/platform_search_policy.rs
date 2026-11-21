//! 平台检索的准入策略（零依赖，便于隔离验证）。
//!
//! 与执行分离：这里只回答三件事——「允许哪些平台」「这次要跑哪几个」「参数是否合法」。
//! 网络与适配器留在 `platform_search_tool`，策略变更不必起整个 crate 才能验。
//!
//! 免登录平台是白名单；小红书 / 抖音 / 知乎没有匿名检索接口，站内搜索只能走
//! 登录态浏览器通道（`browser_task_tab` 的 platform + query），所以在这里被显式拒绝，
//! 并把正确的路指出来——否则模型会以为「不支持 = 查不了」而放弃。

/// 允许在匿名检索里使用的平台。
pub const PLATFORMS: &[&str] = &["bilibili", "bangumi", "weibo", "v2ex", "reddit", "x", "xiaohongshu", "douyin", "zhihu"];

/// 默认检索这几个：中文语境下命中率与可引用性最高。
pub const DEFAULT_PLATFORMS: &[&str] = &["bilibili", "weibo", "bangumi"];

/// 每平台默认条数与上限。
pub const DEFAULT_LIMIT: usize = 4;
pub const MAX_LIMIT: usize = 10;
/// 查询词长度上限。
pub const MAX_QUERY_CHARS: usize = 200;

/// 解析本次要跑的平台列表。空 = 用默认集合。
pub fn requested(platforms: &[String]) -> Result<Vec<String>, String> {
    if platforms.is_empty() {
        return Ok(DEFAULT_PLATFORMS.iter().map(|p| p.to_string()).collect());
    }
    let mut out = Vec::new();
    for platform in platforms {
        let platform = platform.trim();
        if platform.is_empty() {
            continue;
        }
        if !PLATFORMS.contains(&platform) {
            return Err(format!(
                "不支持平台 {platform}：免登录平台只有 {}",
                PLATFORMS.join(" / ")
            ));
        }
        if !out.iter().any(|existing| existing == platform) {
            out.push(platform.to_string());
        }
    }
    if out.is_empty() {
        return Err("platforms 不能全为空字符串".into());
    }
    Ok(out)
}

/// 每平台条数：缺省取默认值，超界夹到上限。
pub fn limit(requested: Option<u64>) -> usize {
    requested
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT)
}

/// 完整参数校验，返回本次要跑的平台列表。
pub fn validate(query: &str, limit_arg: Option<u64>, platforms: &[String]) -> Result<Vec<String>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("query 必须为非空字符串".into());
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(format!("query 过长（最多 {MAX_QUERY_CHARS} 字）"));
    }
    if limit_arg.is_some_and(|n| n < 1 || n as usize > MAX_LIMIT) {
        return Err(format!("limit 必须为 1–{MAX_LIMIT} 的整数"));
    }
    requested(platforms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_are_the_chinese_high_yield_platforms() {
        assert_eq!(validate("芯片", None, &[]).unwrap(), list(DEFAULT_PLATFORMS));
        assert_eq!(limit(None), DEFAULT_LIMIT);
    }

    #[test]
    fn limits_are_clamped_and_zero_is_rejected() {
        assert_eq!(limit(Some(99)), MAX_LIMIT);
        assert_eq!(limit(Some(2)), 2);
        assert!(validate("x", Some(0), &[]).is_err());
        assert!(validate("x", Some(MAX_LIMIT as u64 + 1), &[]).is_err());
    }

    #[test]
    fn empty_and_overlong_queries_are_rejected() {
        assert!(validate("   ", None, &[]).is_err());
        assert!(validate(&"字".repeat(MAX_QUERY_CHARS + 1), None, &[]).is_err());
        assert!(validate(&"字".repeat(MAX_QUERY_CHARS), None, &[]).is_ok());
    }

    #[test]
    fn login_only_platforms_are_routed_to_browser_adapters() {
        for platform in ["xiaohongshu", "douyin", "zhihu"] {
            assert_eq!(validate("芯片", None, &list(&[platform])).unwrap(), list(&[platform]));
        }
    }

    #[test]
    fn unknown_and_duplicate_platforms_are_handled() {
        assert!(validate("x", None, &list(&["myspace"])).is_err());
        assert!(validate("x", None, &list(&["", "  "])).is_err());
        assert_eq!(validate("x", None, &list(&["bilibili", "bilibili"])).unwrap(), list(&["bilibili"]));
        assert_eq!(validate("x", None, &list(&["v2ex", "reddit"])).unwrap(), list(&["v2ex", "reddit"]));
    }
}
