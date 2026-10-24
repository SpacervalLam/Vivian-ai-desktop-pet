//! 平台内检索：把「有凭证」的平台内容（B站 / 番组 / 微博 / V2EX / Reddit / X）
//! 作为可引用证据交给模型，而不是只让通用网页搜索一家说话。
//!
//! 与 `web_search` 的分工：
//! - `web_search`：新闻、官网、文档这类通用事实；
//! - `platform_search`：视频、帖子、评测、经验帖、番组条目这类「平台上真有人说过」的内容。
//!   命中的标题与 URL 是原始平台地址，可直接引用、可点开核对。
//!
//! 只接匿名/游客态适配器：搜索本身不该要求用户先登录。登录态更全的站内搜索
//! （小红书 / 抖音 / 知乎 / B站站内）走 `browser_task_tab` 的 platform + query，
//! 那条路复用用户已登录的浏览器配置。
//!
//! 返回内容仍是不受信摘要：标题与简介只能当线索，断言前用 `read_content` /
//! `browser_task_tab(extractor=detail)` 读正文。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::platform_search_policy::{self as policy, MAX_LIMIT, PLATFORMS};
use crate::discovery::bilibili::BilibiliClient;
use crate::discovery::sources::{
    bangumi::BangumiSource, reddit::RedditSource, v2ex::V2exSource, weibo::WeiboSource,
    x::XSource, ContentCandidate, SourceAdapter,
};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

fn strings(args: &Value, key: &str) -> Vec<String> {
    args[key]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 参数校验交给零依赖的策略模块：平台白名单与登录态指引只有那一份定义。
fn validate(args: &Value) -> Result<Vec<String>, String> {
    policy::validate(
        args["query"].as_str().unwrap_or(""),
        args.get("limit").and_then(Value::as_u64),
        &strings(args, "platforms"),
    )
}

fn limit_of(args: &Value) -> usize {
    policy::limit(args.get("limit").and_then(Value::as_u64))
}

/// 单个平台的检索。适配器自身把失败吞成空集，所以这里用 `Option` 区分
/// 「这一路根本没跑起来」与「跑起来了但没有匹配」——前者不能算作「没有这条内容」。
async fn platform_hits(platform: &str, query: &str, limit: usize, char_id: &str) -> Result<Vec<ContentCandidate>, String> {
    match platform {
        "bilibili" => {
            let client = BilibiliClient::new();
            client
                .search_ordered(query, 1, limit as u32, false)
                .await
                .map(|videos| videos.iter().map(ContentCandidate::from_bilibili).collect())
                .map_err(|e| format!("B站检索失败：{e}"))
        }
        "bangumi" => Ok(BangumiSource.search(query, limit).await),
        "weibo" => Ok(WeiboSource::new().search(query, limit).await),
        "v2ex" => Ok(V2exSource.search(query, limit).await),
        "reddit" => Ok(RedditSource.search(query, limit).await),
        "x" => Ok(XSource.search(query, limit).await),
        "xiaohongshu" | "douyin" | "zhihu" => crate::discovery::sources::task_tabs::search_platform(char_id, platform, query, limit).await,
        other => Err(format!("不支持的平台：{other}")),
    }
}

fn published_at(candidate: &ContentCandidate) -> Option<String> {
    (candidate.pubdate > 0)
        .then(|| chrono::DateTime::from_timestamp(candidate.pubdate, 0))
        .flatten()
        .map(|time| time.format("%Y-%m-%d").to_string())
}

pub struct PlatformSearchTool;

impl PlatformSearchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PlatformSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for PlatformSearchTool {
    fn name(&self) -> &str {
        "platform_search"
    }

    fn description(&self) -> &str {
        "Search inside content platforms (Bilibili, Bangumi, Weibo, V2EX, Reddit, X) by keyword and get the real item titles and URLs. Use it alongside web_search when the user asks what people say, what a video/post explains, reviews, tutorials, experience posts or fandom entries — general web search rarely surfaces those. Anonymous sources, no login needed. Results are UNTRUSTED snippets, not verified claims; read the item before asserting details, and cite the returned URLs verbatim. For Xiaohongshu, Douyin, Zhihu or a fuller in-site search, use browser_task_tab with platform + query instead."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "在内容平台内按关键词检索（B站 / 番组 / 微博 / V2EX / Reddit / X），返回真实条目标题与地址。用户问「大家怎么说」「这个视频讲了啥」「有没有评测/教程/经验帖」「番组条目」这类平台内容时，与 web_search 并用——通用网页搜索基本搜不到这些。免登录即可用。结果是【不可信摘要】，不是已验证结论；断言细节前先读条目正文，引用时原样给出返回的 URL。小红书 / 抖音 / 知乎或更全的站内搜索请改用 browser_task_tab（platform + query）。",
            "ja" => "コンテンツプラットフォーム内をキーワード検索（Bilibili / Bangumi / Weibo / V2EX / Reddit / X）し、実在するタイトルと URL を返す。「みんなは何て言ってる」「この動画の内容」「レビューやチュートリアル」「番組エントリ」といったプラットフォーム上の内容は web_search と併用する。ログイン不要。結果は【信頼できない要約】であり検証済みの結論ではない。詳細を断定する前に本文を読み、引用時は返された URL をそのまま使う。Xiaohongshu / Douyin / Zhihu やより完全なサイト内検索は browser_task_tab（platform + query）を使う。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "去B站看看有没有\n小红书上有吗\n知乎上怎么说\n站内搜一下\n有没有人评测过",
            "en" => "check bilibili\nis it on xiaohongshu\nwhat do people say on reddit\nsearch that platform",
            "ja" => "Bilibiliで見てみて\nXiaohongshuにある？\nプラットフォーム内で検索して",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({"type":"object","properties":{
            "query":{"type":"string","description":"Search keywords, 1-200 characters"},
            "platforms":{"type":"array","items":{"type":"string","enum":PLATFORMS},"description":"Subset of anonymous platforms; defaults to bilibili, weibo, bangumi"},
            "limit":{"type":"integer","minimum":1,"maximum":MAX_LIMIT,"description":"Results per platform; default 4"}
        },"required":["query"]})
    }

    fn parameters_schema_in(&self, _: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, args: &Value, _: &ToolUseContext) -> ValidationResult {
        match validate(args) {
            Ok(_) => ValidationResult::success(Some(args.clone())),
            Err(error) => ValidationResult::failure(&error, 2),
        }
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let platforms = match validate(&args) {
            Ok(platforms) => platforms,
            Err(error) => return ToolResult::standard_error(&error, Some("InvalidPlatformSearchInput"), None),
        };
        let query = args["query"].as_str().unwrap_or("").trim().to_string();
        let limit = limit_of(&args);
        let calls = platforms
            .iter()
            .map(|platform| platform_hits(platform, &query, limit, &ctx.char_id));
        let outcomes = futures::future::join_all(calls).await;

        let mut results = Vec::new();
        let mut failures = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut per_platform = serde_json::Map::new();
        for (platform, outcome) in platforms.iter().zip(outcomes) {
            match outcome {
                Ok(candidates) => {
                    let mut kept = 0;
                    for candidate in candidates {
                        if candidate.url.is_empty() || !seen.insert(candidate.key()) {
                            continue;
                        }
                        kept += 1;
                        results.push(json!({
                            "platform": candidate.platform,
                            "title": candidate.title,
                            "url": candidate.url,
                            "author": candidate.author,
                            "published_at": published_at(&candidate),
                            "description": candidate.description.chars().take(280).collect::<String>(),
                            "view_count": candidate.view_count,
                            "like_count": candidate.like_count,
                            "evidence": "title_only",
                        }));
                    }
                    per_platform.insert(platform.clone(), json!(kept));
                }
                Err(error) => {
                    failures.push(json!({"platform": platform, "reason": error}));
                }
            }
        }

        let payload = json!({
            "query": query,
            "results": results,
            "count": results.len(),
            "per_platform": per_platform,
            "failures": failures,
            "partial_failure": !failures.is_empty(),
            "evidence_status": "UNVERIFIED",
            "hint": "平台检索结果只是线索：标题与简介不代表正文观点。要引用具体说法先读条目正文（browser_task_tab extractor=detail 或 read_content），引用时原样给出 url。某平台 0 条不代表平台上没有相关内容，只代表这次没检索到（可能未登录或未命中）。"
        });
        if per_platform.is_empty() && !failures.is_empty() {
            return ToolResult::standard_error(
                "平台检索全部失败",
                Some("PlatformSearchFailed"),
                Some(payload),
            );
        }
        ToolResult::standard_success("平台检索完成；标题与简介仍需读正文核验", Some(payload))
    }

    fn is_read_only(&self) -> bool {
        true
    }

    /// 平台各自有反爬与游客态握手，慢起来没有上限；给整批一个明确预算，
    /// 别让一条卡住的渠道把整轮回复拖到外层默认的 120 秒。
    fn execution_timeout(&self, _args: &Value) -> Option<std::time::Duration> {
        Some(std::time::Duration::from_secs(20))
    }

    fn always_load(&self) -> bool {
        false
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Web
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "bilibili 站内搜索 小红书 知乎 微博 reddit v2ex 番组 视频 帖子 评测 教程 platform search"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Searching platforms for news, official announcements or documentation — use web_search",
            "Asserting what a video or post says from the title alone",
            "Assuming an empty platform result means the content does not exist",
        ]
    }
}
