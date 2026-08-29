//! Web 搜索能力缝隙（capability seam）的统一词汇。
//!
//! 参考 deepseek-harness 的 dsh-web 设计：
//! - 请求 / 结果 / 错误是缝隙拥有的稳定词汇，工具层与 provider 层都只面向它
//! - 可选字段诚实化：provider 返回不了的字段就是 `None`，适配器不编造
//!   （强迫适配器发明标题/摘要会让缝隙说谎）
//! - 错误携带机器可路由的错误码与 provider 归属，工具层把它结构化地传给模型

use serde::{Deserialize, Serialize};

// ============================================================================
// WebSearchRequest — 请求
// ============================================================================

/// 一次搜索请求。
///
/// 每个请求携带一条 query；`max_results` 是消费方（工具层 / 流水线 /
/// 后台任务）设定的上界，由**缝隙在返回时强制截断**，不信任 provider 自觉
/// ——provider 侧的条数参数只是省钱优化，不是正确性保证。
///
/// `engines` 是请求级引擎指定（LLM 工具调用时自主选择）：`None` 用配置的
/// 供应商链（全部已启用引擎并发）；`Some` 指定一个或多个引擎，与用户已
/// 启用的 providers 取交集后执行。
#[derive(Debug, Clone)]
pub struct WebSearchRequest {
    /// 搜索关键词
    pub query: String,
    /// 返回来源数上限；`None` = 不设限
    pub max_results: Option<usize>,
    /// 请求级引擎指定（如 ["deepseek"] 或 ["bing", "tavily"]）；`None` = 用配置链
    pub engines: Option<Vec<String>>,
}

impl WebSearchRequest {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            max_results: None,
            engines: None,
        }
    }

    pub fn with_max_results(mut self, max_results: usize) -> Self {
        self.max_results = Some(max_results);
        self
    }

    /// 指定本次请求使用的引擎（一个或多个；与用户已启用 providers 取交集）
    pub fn with_engines(mut self, engines: Vec<String>) -> Self {
        self.engines = Some(engines);
        self
    }
}

// ============================================================================
// WebSearchSource / WebSearchResult — 结果
// ============================================================================

/// 单条可引用搜索来源。
///
/// 除 `url` 外全部可选：不是每个 provider 都返回标题 / 摘要 / 发布时间，
/// 强迫适配器编造会让缝隙说谎。`source_tier` / `confidence` 是 vivian-rs
/// 特有的来源权威分级标注（由 URL 域名规则派生，与 provider 无关，
/// 见 `providers::util::annotate_source`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchSource {
    /// 来源 URL（必有）
    pub url: String,
    /// 标题（provider 未返回则为 `None`）
    pub title: Option<String>,
    /// 摘要（provider 未返回则为 `None`）
    pub snippet: Option<String>,
    /// 发布 / 收录时间（provider 提供的原始字符串，如 ISO-8601）
    pub published_at: Option<String>,
    /// 来源权威分级（P0 官方 / P1 权威 / P2 专业社区 / P3 一般参考）
    #[serde(default)]
    pub source_tier: String,
    /// 信心标注（CONFIRMED / MAJORITY / DISPUTED / SINGLE-SOURCE / UNKNOWN）
    #[serde(default)]
    pub confidence: String,
}

impl WebSearchSource {
    /// 新建一条来源（tier / confidence 留空，由 `annotate_source` 补齐）
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            title: None,
            snippet: None,
            published_at: None,
            source_tier: String::new(),
            confidence: String::new(),
        }
    }

    /// 展示标题：标题缺失或为空时回退到 URL 本身
    pub fn display_title(&self) -> &str {
        self.title
            .as_deref()
            .filter(|t| !t.is_empty())
            .unwrap_or(&self.url)
    }

    /// 摘要文本：缺失时回退为空串（供 format! 直接使用）
    pub fn snippet_text(&self) -> &str {
        self.snippet.as_deref().unwrap_or("")
    }
}

/// 一次搜索的归一化结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchResult {
    /// provider 生成的答案文本（答案型引擎如 Perplexity 类返回；普通引擎为 `None`）
    pub content: Option<String>,
    /// 可引用来源列表（已由缝隙截断到请求的 `max_results`）
    pub sources: Vec<WebSearchSource>,
    /// 缝隙为满足 `max_results` 裁掉了来源时置 `true`
    pub truncated: bool,
}

impl WebSearchResult {
    pub fn empty() -> Self {
        Self {
            content: None,
            sources: Vec::new(),
            truncated: false,
        }
    }

    /// 是否携带可用内容（答案文本或来源列表）
    pub fn has_content(&self) -> bool {
        self.content
            .as_deref()
            .map(|c| !c.is_empty())
            .unwrap_or(false)
            || !self.sources.is_empty()
    }
}

// ============================================================================
// WebError — 结构化错误
// ============================================================================

/// 机器可路由的错误码（对齐 deepseek-harness 的 WebError code 语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebErrorCode {
    /// 配置的 provider id 未注册
    ProviderConfiguredMissing,
    /// 配置的 provider 已注册但不可用（缺 api_key / base_url 等）
    ProviderConfiguredUnavailable,
    /// 无可用 provider（注册表为空且无兜底）
    ProviderUnavailable,
    /// provider 执行失败（网络 / HTTP 状态 / 响应解析）
    ProviderError,
    /// 注册 id 冲突
    DuplicateProvider,
}

impl WebErrorCode {
    /// 稳定字符串形式（用于日志与模型可见的工具错误元数据）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ProviderConfiguredMissing => "WEB_PROVIDER_CONFIGURED_MISSING",
            Self::ProviderConfiguredUnavailable => "WEB_PROVIDER_CONFIGURED_UNAVAILABLE",
            Self::ProviderUnavailable => "WEB_PROVIDER_UNAVAILABLE",
            Self::ProviderError => "WEB_PROVIDER_ERROR",
            Self::DuplicateProvider => "WEB_DUPLICATE_PROVIDER",
        }
    }
}

/// 带错误码与 provider 归属的 Web 搜索错误。
///
/// 区别于旧行为（引擎失败吞成空结果）：结构化错误让工具层能向模型
/// 诚实报告「搜索失败的原因」而非「没有结果」，让流水线 / 后台任务
/// 能区分「无匹配」与「不可用」。
#[derive(Debug, Clone)]
pub struct WebError {
    /// 机器可路由错误码
    pub code: WebErrorCode,
    /// 人类可读消息
    pub message: String,
    /// 失败归属的 provider id（聚合错误为 `None`）
    pub provider: Option<String>,
}

impl WebError {
    pub fn new(code: WebErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            provider: None,
        }
    }

    /// 某个 provider 执行失败
    pub fn provider_error(provider: &str, message: impl Into<String>) -> Self {
        Self {
            code: WebErrorCode::ProviderError,
            message: message.into(),
            provider: Some(provider.to_string()),
        }
    }

    /// 配置的 id 未注册
    pub fn configured_missing(provider: &str) -> Self {
        Self {
            code: WebErrorCode::ProviderConfiguredMissing,
            message: format!("配置的搜索 provider「{provider}」未注册"),
            provider: Some(provider.to_string()),
        }
    }

    /// 已注册但本配置下不可用
    pub fn configured_unavailable(provider: &str) -> Self {
        Self {
            code: WebErrorCode::ProviderConfiguredUnavailable,
            message: format!("搜索 provider「{provider}」已注册但不可用（缺少必要配置）"),
            provider: Some(provider.to_string()),
        }
    }
}

impl std::fmt::Display for WebError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.provider {
            Some(p) => write!(f, "[{}] {}: {}", self.code.as_str(), p, self.message),
            None => write!(f, "[{}]: {}", self.code.as_str(), self.message),
        }
    }
}

impl std::error::Error for WebError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_builder() {
        let req = WebSearchRequest::new("rust async").with_max_results(5);
        assert_eq!(req.query, "rust async");
        assert_eq!(req.max_results, Some(5));
        assert!(req.engines.is_none());

        let req = WebSearchRequest::new("q").with_engines(vec!["deepseek".into()]);
        assert_eq!(req.engines.as_deref(), Some(["deepseek".to_string()].as_slice()));
    }

    #[test]
    fn test_source_display_fallbacks() {
        let mut s = WebSearchSource::new("https://example.com/page");
        assert_eq!(s.display_title(), "https://example.com/page");
        assert_eq!(s.snippet_text(), "");
        s.title = Some("Example".into());
        s.snippet = Some("desc".into());
        assert_eq!(s.display_title(), "Example");
        assert_eq!(s.snippet_text(), "desc");
        // 空串标题也回退
        s.title = Some(String::new());
        assert_eq!(s.display_title(), "https://example.com/page");
    }

    #[test]
    fn test_error_display() {
        let e = WebError::provider_error("searxng", "请求失败");
        assert_eq!(
            e.to_string(),
            "[WEB_PROVIDER_ERROR] searxng: 请求失败"
        );
        assert_eq!(e.code.as_str(), "WEB_PROVIDER_ERROR");
    }

    #[test]
    fn test_result_has_content() {
        assert!(!WebSearchResult::empty().has_content());
        let r = WebSearchResult {
            content: Some("answer".into()),
            sources: vec![],
            truncated: false,
        };
        assert!(r.has_content());
    }
}
