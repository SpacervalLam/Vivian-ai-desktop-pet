//! 网络搜索工具 —— 模型可见层（三层解耦中的工具层）。
//!
//! 参考 deepseek-harness 的 dsh-tool-web 设计：本工具只拥有模型可见的
//! schema、参数校验与结果格式化；网络访问、provider 选择、多引擎扇出
//! 合并全部委托 `network::web` 服务缝隙，绝不直接碰网络。
//!
//! 设计哲学：
//! - **LLM 原生搜索仍是首选**（OpenAI web_search_options / Gemini google_search 等）
//! - 本工具作为**补充方案**，仅在以下场景被 LLM 主动调用：
//!   * 当前模型无原生联网能力（如本地 Ollama 模型、未实现的 provider）
//!   * 用户要求限定搜索来源 / 域名 / 时间窗（原生搜索控制力弱）
//!   * 需要拿到结构化搜索结果（title/url/snippet）做后续处理
//!
//! 诚实失败：缝隙返回 `WebError` 时向模型报告结构化错误（错误码 +
//! provider），而不是把「搜索失败」伪装成「没有结果」。

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::Manager;

use crate::network::web::{
    read_search_config, current_app_handle, WebSearchRequest, WebSearchService,
};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

// ============================================================================
// WebSearchTool
// ============================================================================

/// 网络搜索工具
///
/// 让 LLM 在需要时主动发起网络搜索，返回结构化结果（title / url / snippet
/// 及来源权威分级）。执行链路：工具 → `network::web` 缝隙 → provider。
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
        "Search the internet for the latest information, returning titles, links, snippets, and each result's source authority tier (P0 official / P1 authoritative / P2 professional community / P3 general reference) plus a confidence tag. Use when time-sensitive information is needed (news, weather,\
         stock prices, latest releases), or when search sources need to be restricted (e.g. only GitHub / Zhihu / papers).\
         Do NOT use for pure chit-chat, personal feelings, or questions you can answer from memory/conversation — search is for verifying external facts, not for every reply.\
         If the model has built-in native web search capability, prefer the native search; this tool serves as a supplement.\n\n\
         Treat results with evidence discipline: cross-check key numbers or conclusions across 2+ independent sources before asserting them; prefer P0/P1 over P3; when sources conflict, present both sides honestly instead of picking one arbitrarily; and run at most 2 rounds — stop once your core claims have enough independent support, or the remaining gaps are negligible, and do not keep re-searching the same query.\n\n\
         Engine selection (optional `engines` parameter, only engines the user has enabled are honored):\
         - Omitted → all enabled engines run concurrently and results are merged (maximum coverage, the default).\
         - `deepseek` → DeepSeek's official native web search: citation-grade snippets, highest quality; one search = one model call, so use it for high-stakes fact verification.\
         - `tavily` → search API optimized for LLM agents.\
         - `bing` → official Bing API.\
         - `searxng` → self-hosted meta-search aggregation.\
         - `duckduckgo` → zero-config fallback.\
         Specify one engine for a targeted, cheaper search, or several (e.g. [\"deepseek\", \"bing\"]) to cross-verify across independent sources."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "搜索互联网以获取最新信息，返回标题、链接、摘要，并为每条结果附上来源权威分级（P0 官方/原始 / P1 权威二手 / P2 专业社区 / P3 一般参考）和信心标注。当需要时效性信息（新闻、天气、股价、最新发布）\
            或需要限定搜索来源（例如仅 GitHub / 知乎 / 论文）时使用。\
            不要在纯闲聊、抒发感受、或你能凭记忆/对话直接回答时使用——搜索是为了核验外部事实，不是每句回复都要搜。\
            如果模型已内置原生联网搜索能力，优先使用原生搜索；本工具作为补充方案。\
            请对结果保持证据纪律：关键数字或结论在断言前要在 2 个以上独立来源间交叉验证；优先采用 P0/P1 而非 P3；当来源冲突时，如实呈现双方而非任取其一；最多搜索两轮即可终止——一旦核心结论已获得足够的独立来源支持，或剩余缺口可忽略，就停止，不要对同一查询反复搜索。\n\n\
            引擎选择（可选 engines 参数，仅可使用用户已启用的引擎）：\
            - 省略 → 并发使用全部已启用引擎并合并结果（覆盖面最大，默认推荐）。\
            - deepseek → DeepSeek 官方原生联网搜索：引用级摘要、质量最高；一次搜索消耗一次模型调用，适合关键事实核验。\
            - tavily → 为 LLM 优化的搜索 API。\
            - bing → 微软官方搜索 API。\
            - searxng → 自部署元搜索聚合。\
            - duckduckgo → 零配置兜底。\
            指定单个引擎做定向低成本搜索，或指定多个（如 [\"deepseek\", \"bing\"]）跨独立来源交叉验证。",
            "ja" => "インターネットを検索して最新情報を取得し、タイトル、リンク、スニペット、および各結果のソース権威レベル（P0 公式/一次 / P1 権威ある二次 / P2 専門コミュニティ / P3 一般参考）と信頼タグを返す。時効性の高い情報\
            （ニュース、天気、株価、最新リリース）が必要な場合や、検索ソースを制限したい場合（例：GitHub / 知乎 / 論文のみ）に使用。\
            ただの雑談、感情の表現、記憶や会話からすぐ答えられる質問では使わないこと——検索は外部事実の検証のためであり、毎回の返信には不要。\
            モデルにネイティブのウェブ検索機能が内蔵されている場合はネイティブ検索を優先し、本ツールは補助として使用する。\
            結果には証拠規律を持つこと：重要な数字や結論を断言する前に 2 つ以上の独立したソースでクロスチェック；P0/P1 を P3 より優先；ソースが矛盾する場合は一方を恣意的に選ばず両者を正直に提示；検索は最大 2 ラウンドで終了——中核の主張が十分な独立ソースで裏付けられたら、または残りのギャップが無視できるなら停止し、同じクエリを繰り返し検索しない。\n\n\
            エンジン選択（オプション engines パラメータ、ユーザーが有効化したエンジンのみ使用可能）：\
            - 省略 → 有効化された全エンジンを並行実行し結果を統合（最大カバレッジ、デフォルト）。\
            - deepseek → DeepSeek 公式ネイティブ検索：引用グレードのスニペット、最高品質；1 検索 = 1 モデル呼び出しのため、重要な事実検証に使用。\
            - tavily → LLM 向けに最適化された検索 API。\
            - bing → Microsoft 公式検索 API。\
            - searxng → セルフホストのメタ検索集約。\
            - duckduckgo → ゼロ設定のフォールバック。\
            単一エンジンで的を絞った低コスト検索、または複数指定（例 [\"deepseek\", \"bing\"]）で独立ソース間のクロス検証。",
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
                    "description": "Maximum number of results to return (1-20); when omitted, the default differs by caller: 10 results in chat, 15 in work/coding sessions (an explicit user config value, if set, takes precedence)",
                    "minimum": 1,
                    "maximum": 20
                },
                "engines": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "enum": ["duckduckgo", "searxng", "tavily", "bing", "deepseek"]
                    },
                    "description": "Optional: one or more search engines to use (only engines the user enabled are honored; omitted = all enabled engines concurrently). Use 'deepseek' for highest-quality citation-grade results, or several engines to cross-verify."
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
                        "description": "返回结果的最大数量（1-20）；省略时按调用方取默认：聊天 10 条 / 工作 15 条（用户显式配置的值优先）",
                        "minimum": 1,
                        "maximum": 20
                    },
                    "engines": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["duckduckgo", "searxng", "tavily", "bing", "deepseek"]
                        },
                        "description": "可选：本次搜索使用的一个或多个引擎（仅用户已启用的引擎有效；省略 = 并发使用全部已启用引擎）。deepseek 质量最高（引用级摘要），指定多个可交叉验证。"
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
                        "description": "返却結果の最大数（1-20）；省略時は呼び出し元別のデフォルト：チャット 10 件 / 仕事（コーディング）15 件（ユーザーが明示的に設定した値が優先）",
                        "minimum": 1,
                        "maximum": 20
                    },
                    "engines": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["duckduckgo", "searxng", "tavily", "bing", "deepseek"]
                        },
                        "description": "オプション：この検索で使用する 1 つ以上のエンジン（ユーザーが有効化したエンジンのみ有効；省略 = 有効な全エンジンを並行使用）。deepseek は最高品質、複数指定でクロス検証。"
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

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if query.is_empty() {
            return ToolResult::standard_error("搜索关键词不能为空", Some("query is empty"), None);
        }

        // 读取配置（未注入 AppHandle 时走 None = 缝隙默认 DuckDuckGo 链）
        let (config, proxy_url) = read_search_config();

        // 默认结果数按调用方智能体差异化：聊天 10 / 工作 15
        // 优先级：模型显式传参 > 用户在设置面板配置的值 > 差异化默认
        // （配置 0 = 自动；旧默认 5 的清理由配置加载时的一次性迁移负责，
        //   用户后来显式设置的任何值——包括 5——都无条件生效）
        let agent_default = if ctx.is_work_agent() { 15 } else { 10 };
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|n| n.clamp(1, 20) as usize)
            .or_else(|| {
                config
                    .as_ref()
                    .map(|c| c.max_results as usize)
                    .filter(|&n| n > 0)
                    .map(|n| n.clamp(1, 20))
            })
            .unwrap_or(agent_default);

        tracing::info!(
            "[WebSearchTool] 发起搜索: query={:?}, max_results={}, engines={:?}",
            query,
            max_results,
            args.get("engines")
        );

        // 请求级引擎指定（LLM 自主选择一个或多个；缝隙会与已启用池取交集）
        let engines: Option<Vec<String>> = args
            .get("engines")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| e.as_str())
                    .map(|e| e.trim().to_string())
                    .filter(|e| !e.is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty());

        // 委托服务缝隙：选择 / 扇出 / 合并 / 截断全部由缝隙负责
        let mut request = WebSearchRequest::new(query.clone()).with_max_results(max_results);
        if let Some(engines) = engines {
            request = request.with_engines(engines);
        }
        let result = WebSearchService::shared()
            .search(&request, config.as_ref(), proxy_url.as_deref())
            .await;

        match result {
            Ok(r) if !r.sources.is_empty() => {
                // 记录主题提示：对话中搜索过的关键词会留给后台知识采集任务优先处理
                if let Some(handle) = current_app_handle() {
                    if let Ok(memory) = handle
                        .state::<std::sync::Arc<crate::state::AppState>>()
                        .memory()
                    {
                        memory.push_topic_hint(&query);
                    }
                }

                // 返回结构化结果给 LLM
                ToolResult::standard_success(
                    &format!("找到 {} 条与「{}」相关的搜索结果", r.sources.len(), query),
                    Some(json!({
                        "query": query,
                        "results": r.sources,
                        "count": r.sources.len(),
                        "truncated": r.truncated,
                    })),
                )
            }
            // 搜索成功但无匹配（区别于失败：请勿反复重搜）
            Ok(_) => ToolResult::standard_success(
                &format!(
                    "未找到与「{}」相关的搜索结果。可能该查询确实无匹配内容。\
                    请勿对同一查询反复调用 web_search，改为基于你已有的知识回答用户。",
                    query
                ),
                Some(json!({
                    "query": query,
                    "results": [],
                    "count": 0,
                })),
            ),
            // 诚实失败：报告错误码与 provider，而非伪装成「没有结果」
            Err(e) => ToolResult::standard_error(
                &format!("搜索失败：{e}"),
                Some(e.code.as_str()),
                Some(json!({
                    "query": query,
                    "provider": e.provider,
                })),
            ),
        }
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
        // web_search 是只读的网络获取，无副作用，归为 Safe 避免每次调用弹窗确认
        ToolRiskTier::Safe
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
