//! 内容发现工具集 — 推荐内容 / 反馈 / 兴趣探针
//!
//! 四个工具构成推荐闭环的 LLM 入口：
//! - `recommend_content`：用户要推荐时调用（推荐点视频/内容看看）
//! - `submit_content_feedback`：用户对推荐内容表态后写反馈
//! - `get_interest_probes`：读取待确认的兴趣猜测（角色像朋友一样问用户）
//! - `answer_interest_probe`：把用户对猜测的三态回应写回（confirm/reject/defer）

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};

fn resolve_char_id(ctx: &ToolUseContext) -> String {
    if ctx.char_id.is_empty() {
        "vivian".to_string()
    } else {
        ctx.char_id.clone()
    }
}

// ============================================================================
// RecommendContentTool
// ============================================================================

pub struct RecommendContentTool;

impl RecommendContentTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RecommendContentTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for RecommendContentTool {
    fn name(&self) -> &str {
        "recommend_content"
    }

    fn description(&self) -> &str {
        "Recommend videos/content the user might like, selected from a pool discovered based on \
their interest profile (Bilibili). Each item comes with a friend-style reason explaining WHY \
the user would like it. Use when the user asks for recommendations ('recommend something', \
'anything fun to watch', 'share some videos'). After calling, present the results in your own \
words and optionally share the best one as a link card via share_link. If the user reacted to \
a previous recommendation, call submit_content_feedback first."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "根据用户兴趣画像推荐其可能喜欢的视频/内容（B站来源，发现引擎基于画像主动搜寻）。\
每条附带朋友式的推荐理由（为什么觉得用户会喜欢）。用户说「推荐点什么」「有什么好玩的」「分享点视频」时调用。\
拿到结果后用自己的口吻转述，最值得看的一条可用 share_link 做成链接卡片分享。\
用户对之前的推荐有表态时先调用 submit_content_feedback。",
            "ja" => "ユーザーの興味プロファイルに基づいて、好きそうな動画/コンテンツを推薦する（Bilibiliソース）。\
各項目には友達風のおすすめ理由が付く。ユーザーが「何かおすすめ」「面白い動画ある？」と言ったら呼び出す。\
結果は自分の言葉で伝え、一番良いものは share_link でカード共有してもよい。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Number of recommendations to return (1-5, default 3)",
                    "minimum": 1,
                    "maximum": 5
                }
            }
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "description": "返回推荐条数（1-5，默认 3）",
                        "minimum": 1,
                        "maximum": 5
                    }
                }
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "description": "返す推薦数（1-5、デフォルト 3）",
                        "minimum": 1,
                        "maximum": 5
                    }
                }
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if let Some(limit) = input.get("limit") {
            if !limit.is_u64() {
                return ValidationResult::failure("limit 必须是整数", 2);
            }
            let n = limit.as_u64().unwrap_or(0);
            if !(1..=5).contains(&n) {
                return ValidationResult::failure("limit 范围 1-5", 2);
            }
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let char_id = resolve_char_id(ctx);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n.clamp(1, 5) as usize)
            .unwrap_or(3);

        match crate::discovery::recommend_for_user(&char_id, limit).await {
            Ok(views) => {
                let items: Vec<Value> = views
                    .iter()
                    .map(|v| {
                        json!({
                            "title": v.title,
                            "url": v.url,
                            "platform": v.platform,
                            "author": v.up_name,
                            "duration_secs": v.duration_secs,
                            "topic": v.topic_group,
                            "match_score": (v.score * 100.0).round() / 100.0,
                            "reason": v.expression,
                        })
                    })
                    .collect();
                ToolResult::standard_success(
                    &format!("为你挑选了 {} 条内容", items.len()),
                    Some(json!({
                        "items": items,
                        "hint": "用自己的口吻转述推荐理由；最值得看的一条可用 share_link 工具分享为链接卡片",
                    })),
                )
            }
            Err(e) => ToolResult::standard_error("暂无推荐", Some(&e), None),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Web
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "recommend video content bilibili discover share 推荐 视频 内容"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Answering general questions that have nothing to do with content recommendations",
            "Searching for specific information the user asked for (use web_search instead)",
        ]
    }
}

// ============================================================================
// SubmitContentFeedbackTool
// ============================================================================

pub struct SubmitContentFeedbackTool;

impl SubmitContentFeedbackTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SubmitContentFeedbackTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SubmitContentFeedbackTool {
    fn name(&self) -> &str {
        "submit_content_feedback"
    }

    fn description(&self) -> &str {
        "Record the user's feedback on a previously recommended content item (like / dislike / \
neutral). Feedback updates the interest profile and changes future recommendations — liked \
topics get boosted, disliked topics get suppressed. Call when the user reacts to a \
recommendation: positive ('this is great', 'exactly my taste'), negative ('not interested', \
'stop showing this'), or neutral ('meh')."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "记录用户对推荐内容的反馈（like 喜欢 / dislike 不喜欢 / neutral 一般）。\
反馈会写入兴趣画像并改变后续推荐——喜欢的主题被强化，不喜欢的被抑制。\
用户对推荐内容表态时调用：正面（「很赞」「正合我口味」）→ like；负面（「不感兴趣」「别推这种」）→ dislike；\
无感 → neutral。target 填推荐结果里的 url 或 bvid。",
            "ja" => "推薦コンテンツに対するユーザーのフィードバックを記録する（like / dislike / neutral）。\
フィードバックは興味プロファイルに反映され、以降の推薦が変わる。target には url または bvid を指定。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "description": "The recommended content's URL or bvid (from recommend_content result)"
                },
                "feedback": {
                    "type": "string",
                    "enum": ["like", "dislike", "neutral"],
                    "description": "User's reaction: like (enjoyed it) / dislike (not interested) / neutral (indifferent)"
                }
            },
            "required": ["target", "feedback"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "推荐内容的 URL 或 bvid（recommend_content 结果中给出）"
                    },
                    "feedback": {
                        "type": "string",
                        "enum": ["like", "dislike", "neutral"],
                        "description": "用户态度：like（喜欢）/ dislike（不感兴趣）/ neutral（一般）"
                    }
                },
                "required": ["target", "feedback"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "推薦コンテンツの URL または bvid"
                    },
                    "feedback": {
                        "type": "string",
                        "enum": ["like", "dislike", "neutral"],
                        "description": "ユーザーの反応：like（良かった）/ dislike（興味ない）/ neutral（普通）"
                    }
                },
                "required": ["target", "feedback"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let target = input.get("target").and_then(|v| v.as_str()).unwrap_or("").trim();
        let feedback = input
            .get("feedback")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if target.is_empty() {
            return ValidationResult::failure("target 不能为空", 2);
        }
        if !matches!(feedback, "like" | "dislike" | "neutral") {
            return ValidationResult::failure("feedback 必须是 like / dislike / neutral", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let char_id = resolve_char_id(ctx);
        let target = args
            .get("target")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let feedback = args
            .get("feedback")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        let (hit, message) = crate::discovery::apply_feedback(&char_id, &target, feedback);
        if hit {
            ToolResult::standard_success(&message, None)
        } else {
            ToolResult::standard_error("反馈未记录", Some(&message), None)
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "content feedback like dislike preference learn 反馈 喜欢 不感兴趣"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Recording feedback for links you shared from web_search results (only for recommend_content results)",
            "Submitting feedback the user never expressed",
        ]
    }
}

// ============================================================================
// GetInterestProbesTool
// ============================================================================

pub struct GetInterestProbesTool;

impl GetInterestProbesTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GetInterestProbesTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GetInterestProbesTool {
    fn name(&self) -> &str {
        "get_interest_probes"
    }

    fn description(&self) -> &str {
        "Get speculative interest directions the agent has guessed the user might like but hasn't \
confirmed yet (e.g. 'architectural aesthetics', 'mechanical watch craftsmanship'). Each probe \
comes with WHY it was guessed (bridged from existing interests) and example subtopics. Use when \
chatting casually and wanting to explore the user's taste boundaries — ask about ONE probe at a \
time, like a friend: 'I have a hunch you might be into X, am I right?'. After the user responds, \
call answer_interest_probe."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "获取智能体猜测用户可能感兴趣但尚未确认的兴趣方向（如「建筑美学」「制表工艺」）。\
每个猜测附带猜测理由（从已有兴趣如何桥接而来）和细分话题示例。闲聊中想探索用户兴趣边界时调用——\
一次只问一个方向，像朋友一样随口问：「我猜你可能对 X 有点兴趣，猜对了吗？」。\
用户回应后调用 answer_interest_probe 记录回应。",
            "ja" => "ユーザーが興味を持ちそうだと推測している未確認の興味方向を取得する。\
各推測には理由と具体例が付く。雑談でユーザーの興味の境界を探りたい時に呼び出し、\
一度に一つだけ友達のように尋ねる。回答後は answer_interest_probe を呼ぶ。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, _args: Value, ctx: &ToolUseContext) -> ToolResult {
        let char_id = resolve_char_id(ctx);
        let probes = crate::discovery::speculator::InterestSpeculator::active_probes(&char_id);
        if probes.is_empty() {
            return ToolResult::standard_success(
                "当前没有待确认的兴趣猜测",
                Some(json!({ "probes": [], "hint": "稍后后台会生成新的猜测" })),
            );
        }
        let items: Vec<Value> = probes
            .iter()
            .map(|p| {
                json!({
                    "domain": p.domain,
                    "category": p.category,
                    "reason": p.reason,
                    "specifics": p.specifics,
                    "probe_mode": p.probe_mode,
                    "hint": "一次只问一个方向；用户回应后用 answer_interest_probe 记录",
                })
            })
            .collect();
        ToolResult::standard_success(
            &format!("有 {} 个待确认的兴趣猜测", items.len()),
            Some(json!({ "probes": items })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "interest probe speculate guess taste explore 兴趣 猜测 探索 试探"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Asking the user about ALL probes at once (pick one, keep it conversational)",
            "Using it in every reply (only when naturally exploring interests)",
        ]
    }
}

// ============================================================================
// AnswerInterestProbeTool
// ============================================================================

pub struct AnswerInterestProbeTool;

impl AnswerInterestProbeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AnswerInterestProbeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for AnswerInterestProbeTool {
    fn name(&self) -> &str {
        "answer_interest_probe"
    }

    fn description(&self) -> &str {
        "Record the user's verdict on a speculated interest direction. Responses: confirm \
(user likes it — becomes a formal interest, discovery starts searching it), reject (user \
dislikes it — won't be guessed again for 30 days), defer (user says 'maybe later' — snoozed \
and re-asked later). Call after the user responds to a probe question. The comment field is \
optional and stores nuance from the user's reply."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "记录用户对兴趣猜测的回应。response 三态：confirm（用户认可，升级为正式兴趣并开始为其搜寻内容）、\
reject（用户不感兴趣，30 天内不再猜测该方向）、defer（「以后再说」，暂缓稍后再问）。\
用户对猜测问题表态后调用。comment 可选，存用户回答中的细微态度。",
            "ja" => "興味推測に対するユーザーの回答を記録する。response：confirm（正式な興味に昇格）、\
reject（30日間再推測しない）、defer（後でまた）。推測への回答の後に呼び出す。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "domain": {
                    "type": "string",
                    "description": "The speculated interest domain (from get_interest_probes result)"
                },
                "response": {
                    "type": "string",
                    "enum": ["confirm", "reject", "defer"],
                    "description": "confirm (user likes it) / reject (user dislikes it) / defer (ask again later)"
                },
                "comment": {
                    "type": "string",
                    "description": "Optional: nuance from the user's actual reply"
                }
            },
            "required": ["domain", "response"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "domain": {
                        "type": "string",
                        "description": "兴趣猜测方向（get_interest_probes 结果中的 domain）"
                    },
                    "response": {
                        "type": "string",
                        "enum": ["confirm", "reject", "defer"],
                        "description": "confirm（用户喜欢）/ reject（不感兴趣）/ defer（以后再说）"
                    },
                    "comment": {
                        "type": "string",
                        "description": "可选：用户回答中的细微态度"
                    }
                },
                "required": ["domain", "response"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "domain": {
                        "type": "string",
                        "description": "興味推測の方向（get_interest_probes の domain）"
                    },
                    "response": {
                        "type": "string",
                        "enum": ["confirm", "reject", "defer"],
                        "description": "confirm（好き）/ reject（興味ない）/ defer（後で）"
                    },
                    "comment": {
                        "type": "string",
                        "description": "任意：ユーザー回答のニュアンス"
                    }
                },
                "required": ["domain", "response"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let domain = input.get("domain").and_then(|v| v.as_str()).unwrap_or("").trim();
        let response = input
            .get("response")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if domain.is_empty() {
            return ValidationResult::failure("domain 不能为空", 2);
        }
        if !matches!(response, "confirm" | "reject" | "defer") {
            return ValidationResult::failure("response 必须是 confirm / reject / defer", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let char_id = resolve_char_id(ctx);
        let domain = args
            .get("domain")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let response = args
            .get("response")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let _comment = args
            .get("comment")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        use crate::discovery::speculator::InterestSpeculator;
        let handled = match response.as_str() {
            "confirm" => {
                let ok = InterestSpeculator::user_confirm(&char_id, &domain);
                if ok {
                    // 确认即写入画像，发现引擎下一轮就会为它搜寻内容
                    let mut profile = crate::discovery::profile::InterestProfile::load(&char_id);
                    profile.upsert_interest(&domain, 0.85, "probe");
                    profile.save(&char_id);
                }
                ok
            }
            "reject" => InterestSpeculator::user_reject(&char_id, &domain),
            _ => InterestSpeculator::user_defer(&char_id, &domain).is_some(),
        };

        if !handled {
            return ToolResult::standard_error(
                "未找到该兴趣猜测",
                Some(&format!("活跃猜测中不存在：{}", domain)),
                None,
            );
        }

        let message = match response.as_str() {
            "confirm" => format!("已将「{}」升级为正式兴趣，之后会主动帮你留意相关内容", domain),
            "reject" => format!("已记录「{}」不感兴趣，30 天内不会再猜这个方向", domain),
            _ => format!("已暂缓「{}」，过几天会再找机会问问你", domain),
        };
        ToolResult::standard_success(&message, None)
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "interest probe answer confirm reject defer 猜测 回应 确认 拒绝 暂缓"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Answering a probe the user was never asked about",
            "Guessing the user's response instead of waiting for their actual reply",
        ]
    }
}
