//! 网络搜索工具 - 把自建搜索后端（DuckDuckGo / SearXNG / Tavily）暴露为 LLM 可调用工具
//!
//! 设计哲学：
//! - **LLM 原生搜索仍是首选**（OpenAI web_search_options / Gemini google_search 等）
//! - 本工具作为**补充方案**，仅在以下场景被 LLM 主动调用：
//!   * 当前模型无原生联网能力（如本地 Ollama 模型、未实现的 provider）
//!   * 用户要求限定搜索来源 / 域名 / 时间窗（原生搜索控制力弱）
//!   * 需要拿到结构化搜索结果（title/url/snippet）做后续处理
//!
//! 多引擎混用：由 `WebSearchConfig.providers` 决定启用的引擎列表，搜索工具会并发调用
//! 所有已配置的引擎并合并去重结果。

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::config::WebSearchConfig;
use crate::network::web_context::WebSearcher;
use crate::state::AppState;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

/// 全局 AppHandle（由 lib.rs setup 注入，用于读取 AppState 中的 WebSearchConfig）
static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用一次）
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// 读取当前 WebSearchConfig（AppHandle 未注入时返回 None，走 DuckDuckGo 默认）
fn read_web_search_config() -> Option<WebSearchConfig> {
    APP_HANDLE.read().clone().map(|handle| {
        handle
            .state::<Arc<AppState>>()
            .config
            .read()
            .get_all()
            .web_search
            .clone()
    })
}

// ============================================================================
// WebSearchTool
// ============================================================================

/// 网络搜索工具
///
/// 让 LLM 在需要时主动发起网络搜索，返回结构化结果（title / url / snippet）。
/// 与 provider 原生搜索（如 OpenAI web_search_options）互补：
/// - 原生搜索：模型自己读网页、总结、引用，开发者零控制
/// - 本工具：拿到 raw 结果，LLM 可二次加工、限定来源、做后续工具链
pub struct WebSearchTool;

impl WebSearchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the internet for the latest information, returning titles, links, and snippets. Use when time-sensitive information is needed (news, weather,\
         stock prices, latest releases), or when search sources need to be restricted (e.g. only GitHub / Zhihu / papers).\
         If the model has built-in native web search capability, prefer the native search; this tool serves as a supplement."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "搜索互联网以获取最新信息，返回标题、链接和摘要。当需要时效性信息（新闻、天气、股价、最新发布）\
            或需要限定搜索来源（例如仅 GitHub / 知乎 / 论文）时使用。如果模型已内置原生联网搜索能力，\
            优先使用原生搜索；本工具作为补充方案。",
            "ja" => "インターネットを検索して最新情報を取得し、タイトル、リンク、スニペットを返す。時効性の高い情報\
            （ニュース、天気、株価、最新リリース）が必要な場合や、検索ソースを制限したい場合（例：GitHub / 知乎 / 論文のみ）に使用。\
            モデルにネイティブのウェブ検索機能が内蔵されている場合はネイティブ検索を優先し、本ツールは補助として使用する。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search keywords; prefer a specific query rather than a full question (e.g. 'RTX 5090 release' instead of 'tell me news about RTX 5090')"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (1-20); when omitted, the configured default is used (usually 5)",
                    "minimum": 1,
                    "maximum": 20
                }
            },
            "required": ["query"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "搜索关键词；建议用具体查询而非完整问题（例如用 'RTX 5090 release' 而非 '告诉我 RTX 5090 的新闻'）"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "返回结果的最大数量（1-20）；省略时使用配置的默认值（通常为 5）",
                        "minimum": 1,
                        "maximum": 20
                    }
                },
                "required": ["query"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "検索キーワード；完全な質問ではなく具体的なクエリを推奨（例：'RTX 5090 のニュースを教えて' ではなく 'RTX 5090 release'）"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "返却結果の最大数（1-20）；省略時は設定されたデフォルト値を使用（通常 5）",
                        "minimum": 1,
                        "maximum": 20
                    }
                },
                "required": ["query"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if query.is_empty() {
            return ValidationResult::failure("query 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if query.is_empty() {
            return ToolResult::standard_error("搜索关键词不能为空", Some("query is empty"), None);
        }

        // 读取配置（未注入 AppHandle 时走 None = DuckDuckGo 默认）
        let config = read_web_search_config();

        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|n| n.clamp(1, 20) as usize)
            .or_else(|| config.as_ref().map(|c| c.max_results as usize))
            .unwrap_or(5);

        let config_ref = config.as_ref();

        tracing::info!(
            "[WebSearchTool] 发起搜索: query={:?}, max_results={}, providers={:?}",
            query,
            max_results,
            config_ref
                .map(|c| c.providers.clone())
                .unwrap_or_else(|| vec!["duckduckgo".to_string()])
        );

        let results = WebSearcher::search_with_config(&query, max_results, config_ref).await;

        if results.is_empty() {
            return ToolResult::standard_success(
                &format!("未找到与「{}」相关的搜索结果", query),
                Some(json!({
                    "query": query,
                    "results": [],
                    "count": 0,
                })),
            );
        }

        // 记录主题提示：对话中搜索过的关键词会留给后台知识采集任务优先处理
        if let Some(handle) = APP_HANDLE.read().as_ref() {
            if let Ok(memory) = handle.state::<std::sync::Arc<AppState>>().memory() {
                memory.push_topic_hint(&query);
            }
        }

        // 返回结构化结果给 LLM
        ToolResult::standard_success(
            &format!("找到 {} 条与「{}」相关的搜索结果", results.len(), query),
            Some(json!({
                "query": query,
                "results": results,
                "count": results.len(),
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Web
    }

    fn always_load(&self) -> bool {
        true
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Network
    }

    fn search_hint(&self) -> &str {
        "internet search web retrieval news query real-time information time-sensitive"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Query local file contents (use read_file)",
            "Query the memory database (use search_memory)",
            "Chit-chat or subjective feelings (no search needed)",
        ]
    }
}
