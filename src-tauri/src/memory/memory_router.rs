//! Memory Router —— 记忆写入路径上的分层路由器
//!
//! 在记忆写入之前，决定每条记忆应进入哪一层存储：
//! - `Personal`：角色私有 MemoryManager（默认）
//! - `RelationshipFact`：关系认知事实层（A 对 B 的陈述性认知）
//! - `SharedWorld`：共享世界记忆层（两角色共同知晓的事实）
//! - `Ephemeral`：不长期存储，只走事件账本
//!
//! 路由分两阶段：
//! 1. `route_sync`：规则前置过滤（同步、0 成本），产出候选 destination
//! 2. `route_with_llm`：对候选条目调用 LLM 仲裁确认（异步）
//!
//! 设计原则：
//! - 读路径零 LLM：路由只在写路径执行
//! - 失败兜底：LLM 不可用或返回非法 JSON 时降级为 `route_sync` 的结果
//! - 仅候选条目走 LLM：`Personal` / `Ephemeral` 直接落盘，节省成本
//!
//! 注意：本模块不做实际写入，写入由调用方 `manager.rs` 根据 destination 分发。

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::VivianResult;

/// 记忆目标存储层
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryDestination {
    /// 写入角色私有 MemoryManager（默认）
    Personal,
    /// 写入关系认知事实层 RelationshipFacts（A 对 B 的陈述性认知）
    RelationshipFact,
    /// 写入共享世界记忆层 WorldKnowledge（两角色共同知晓的事实）
    SharedWorld,
    /// 不长期存储，只走事件账本
    Ephemeral,
}

/// 路由上下文
pub struct RouteContext<'a> {
    pub content: &'a str,
    pub importance: f64,
    pub channel: &'a str,
    pub speaker: &'a str,
    pub listener: &'a str,
    pub perspective: &'a str,
    pub char_id: &'a str,
}

/// 持久性词汇表：命中任一关键词则视为可能涉及共享世界事实
///
/// 采用简单子串匹配（`content.contains(keyword)`），不引入正则，避免过度工程化。
const PERSISTENCE_KEYWORDS: &[&str] = &[
    "喜欢", "讨厌", "总是", "从来", "习惯", "规则", "约定", "住", "工作", "职业",
];

/// 阶段 1：规则前置过滤（同步、0 成本）
///
/// 规则（按优先级）：
/// 1. `importance < 0.2` 且 `channel != "cross_character"` → `Ephemeral`
/// 2. `channel == "cross_character"` 且 `importance >= 0.5` → `RelationshipFact`（候选，需 LLM 仲裁确认）
/// 3. 含持久性词汇 → `SharedWorld`（候选，需 LLM 仲裁确认）
/// 4. 其他 → `Personal`
pub fn route_sync(ctx: &RouteContext) -> MemoryDestination {
    // 规则 1：低重要性且非跨角色 → 临时
    if ctx.importance < 0.2 && ctx.channel != "cross_character" {
        return MemoryDestination::Ephemeral;
    }

    // 规则 2：跨角色且重要性较高 → 关系事实候选
    if ctx.channel == "cross_character" && ctx.importance >= 0.5 {
        return MemoryDestination::RelationshipFact;
    }

    // 规则 3：含持久性词汇 → 共享世界候选
    if contains_persistence_keyword(ctx.content) {
        return MemoryDestination::SharedWorld;
    }

    // 规则 4：默认走个人记忆
    MemoryDestination::Personal
}

/// 持久性词汇命中检测
fn contains_persistence_keyword(content: &str) -> bool {
    PERSISTENCE_KEYWORDS.iter().any(|kw| content.contains(kw))
}

/// RouterLlmClient 使用的 LLM 客户端抽象
#[async_trait]
pub trait RouterLlmClient: Send + Sync {
    async fn complete(&self, prompt: &str) -> VivianResult<String>;
}

/// 为 ModelRouter 实现
#[async_trait]
impl RouterLlmClient for crate::providers::ModelRouter {
    async fn complete(&self, prompt: &str) -> VivianResult<String> {
        let messages = vec![crate::types::response::ChatMessage::user(prompt.to_string())];
        let schema = {
            let root = schemars::schema_for!(RouterVerdict);
            serde_json::to_value(&root.schema).unwrap_or_else(|_| serde_json::json!({"type": "object"}))
        };
        self.generate(crate::providers::base::LLMRequest::new("memory", messages).with_json_schema(schema))
            .await
    }
}

/// LLM 仲裁结果
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct RouterVerdict {
    /// "personal" / "relationship_fact" / "shared_world" / "ephemeral"
    destination: String,
    /// LLM 返回的路由原因，目前仅用于调试日志，未在业务逻辑中读取
    #[serde(default)]
    reason: Option<String>,
}

/// 阶段 2：LLM 仲裁（异步、仅候选条目）
///
/// 流程：
/// 1. 先调用 `route_sync` 得到同步结果
/// 2. 仅当同步结果为候选层（`RelationshipFact` / `SharedWorld`）时调用 LLM 仲裁
/// 3. `Personal` / `Ephemeral` 直接返回同步结果，节省 LLM 成本
/// 4. LLM 失败或返回非法 JSON 时降级为 `route_sync` 的结果
pub async fn route_with_llm(
    ctx: &RouteContext<'_>,
    llm: &dyn RouterLlmClient,
) -> MemoryDestination {
    let sync_result = route_sync(ctx);

    // 仅候选条目走 LLM 仲裁
    if !matches!(
        sync_result,
        MemoryDestination::RelationshipFact | MemoryDestination::SharedWorld
    ) {
        return sync_result;
    }

    let prompt = build_router_prompt(ctx);
    match llm.complete(&prompt).await {
        Ok(resp) => match parse_verdict(&resp) {
            Some(verdict) => {
                tracing::debug!(
                    destination = %verdict.destination,
                    reason = ?verdict.reason,
                    "[MemoryRouter] LLM 仲裁结果"
                );
                map_verdict(&verdict.destination).unwrap_or(sync_result)
            }
            None => {
                tracing::warn!(
                    "[MemoryRouter] LLM 返回非法 JSON，降级为同步结果: {:?}",
                    sync_result
                );
                sync_result
            }
        },
        Err(e) => {
            tracing::warn!(
                "[MemoryRouter] LLM 仲裁失败，降级为同步结果: {:?}（错误: {}）",
                sync_result,
                e
            );
            sync_result
        }
    }
}

/// 构造 LLM 仲裁 prompt
fn build_router_prompt(ctx: &RouteContext) -> String {
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
    // perspective 为 "observer" 时附加旁观视角提示
    let is_observer = ctx.perspective == "observer";
    match lang_norm {
        "en" => format!(
            "You are a memory classifier. Determine which layer the following content should be stored in:\n\n\
            Content: {content}\n\
            Channel: {channel}\n\
            Speaker: {speaker}\n\
            Listener: {listener}\n\
            Importance: {importance}\n\
            Perspective: {perspective}\n\
            Current Character: {char_id}\n\
            {observer_hint}\
            Classification rules:\n\
            - personal: the character's personal conversational memory (conversations about {char_id} itself → personal), not involving the other party's personality or shared facts\n\
            - relationship_fact: cognition about another agent's personality/preferences/habits (e.g., \"she likes to tease me\")\n\
            - shared_world: world facts both characters know (e.g., \"the user likes Genshin Impact\", \"don't disturb the user when working\")\n\
            - ephemeral: low-information temporary conversation, not worth long-term storage\n\n\
            Return JSON: {{\"destination\": \"personal|relationship_fact|shared_world|ephemeral\", \"reason\": \"brief reason\"}}",
            content = ctx.content,
            channel = ctx.channel,
            speaker = ctx.speaker,
            listener = ctx.listener,
            importance = ctx.importance,
            perspective = ctx.perspective,
            char_id = ctx.char_id,
            observer_hint = if is_observer { "Note: This is a memory from an observer perspective.\n" } else { "" },
        ),
        "ja" => format!(
            "あなたはメモリー分類器です。以下の内容をどの層に保存すべきか判断してください：\n\n\
            内容：{content}\n\
            チャネル：{channel}\n\
            話者：{speaker}\n\
            聞き手：{listener}\n\
            重要度：{importance}\n\
            視点：{perspective}\n\
            現在のキャラクター：{char_id}\n\
            {observer_hint}\
            分類ルール：\n\
            - personal：キャラクター個人の会話メモリー（{char_id} 自身に関する会話 → personal）、相手の人格や共有事実には関与しない\n\
            - relationship_fact：別のエージェントの人格/好み/習慣に対する認知（例：「彼女は私をツッコミたがる」）\n\
            - shared_world：二人のキャラクターが共に知っている世界の事実（例：「ユーザーは原神が好き」「ユーザーが仕事中は邪魔しない」）\n\
            - ephemeral：情報量の少ない一時的な会話、長期保存に値しない\n\n\
            JSON を返してください：{{\"destination\": \"personal|relationship_fact|shared_world|ephemeral\", \"reason\": \"簡潔な理由\"}}",
            content = ctx.content,
            channel = ctx.channel,
            speaker = ctx.speaker,
            listener = ctx.listener,
            importance = ctx.importance,
            perspective = ctx.perspective,
            char_id = ctx.char_id,
            observer_hint = if is_observer { "注意：これは傍観視点の記憶です。\n" } else { "" },
        ),
        _ => format!(
            "你是一个记忆分类器。请判断以下内容应该存储在哪一层：\n\n\
            内容：{content}\n\
            渠道：{channel}\n\
            说话者：{speaker}\n\
            听话者：{listener}\n\
            重要性：{importance}\n\
            视角：{perspective}\n\
            当前角色：{char_id}\n\
            {observer_hint}\
            分类规则：\n\
            - personal：角色个人的对话记忆（关于 {char_id} 自己的对话 → personal），不涉及对方人格或共享事实\n\
            - relationship_fact：对另一个智能体的人格/偏好/习惯的认知（如\"她喜欢吐槽我\"）\n\
            - shared_world：两个角色共同知晓的世界事实（如\"用户喜欢原神\"\"用户工作时不要打扰\"）\n\
            - ephemeral：低信息量的临时对话，不值得长期存储\n\n\
            请返回 JSON：{{\"destination\": \"personal|relationship_fact|shared_world|ephemeral\", \"reason\": \"简要原因\"}}",
            content = ctx.content,
            channel = ctx.channel,
            speaker = ctx.speaker,
            listener = ctx.listener,
            importance = ctx.importance,
            perspective = ctx.perspective,
            char_id = ctx.char_id,
            observer_hint = if is_observer { "注意：这是旁观视角的记忆。\n" } else { "" },
        ),
    }
}

/// 解析 LLM 返回的 JSON 判决（容错处理代码围栏）
fn parse_verdict(raw: &str) -> Option<RouterVerdict> {
    let cleaned = strip_code_fence(raw);
    serde_json::from_str(cleaned).ok()
}

/// 将 LLM 返回的 destination 字符串映射为 `MemoryDestination`
fn map_verdict(s: &str) -> Option<MemoryDestination> {
    match s.trim() {
        "personal" => Some(MemoryDestination::Personal),
        "relationship_fact" => Some(MemoryDestination::RelationshipFact),
        "shared_world" => Some(MemoryDestination::SharedWorld),
        "ephemeral" => Some(MemoryDestination::Ephemeral),
        _ => None,
    }
}

/// 去除 ```json ... ``` 围栏
fn strip_code_fence(s: &str) -> &str {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```json") {
        return rest.trim().trim_end_matches("```").trim();
    }
    if let Some(rest) = t.strip_prefix("```") {
        return rest.trim().trim_end_matches("```").trim();
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(
        content: &'a str,
        importance: f64,
        channel: &'a str,
    ) -> RouteContext<'a> {
        RouteContext {
            content,
            importance,
            channel,
            speaker: "user",
            listener: "ai",
            perspective: "ai",
            char_id: "ai",
        }
    }

    #[test]
    fn rule1_low_importance_non_cross_is_ephemeral() {
        let c = ctx("嗯", 0.1, "chat");
        assert_eq!(route_sync(&c), MemoryDestination::Ephemeral);
    }

    #[test]
    fn rule1_low_importance_cross_channel_not_ephemeral() {
        // cross_character + 低重要性 + 无持久性词汇 → 走默认 Personal
        let c = ctx("hi", 0.1, "cross_character");
        assert_eq!(route_sync(&c), MemoryDestination::Personal);
    }

    #[test]
    fn rule2_cross_character_high_importance_is_relationship_fact() {
        let c = ctx("她总是吐槽我", 0.7, "cross_character");
        // 注意：这条同时命中持久性词汇"总是"，但规则 2 优先于规则 3
        assert_eq!(route_sync(&c), MemoryDestination::RelationshipFact);
    }

    #[test]
    fn rule3_persistence_keyword_is_shared_world() {
        let c = ctx("我喜欢原神", 0.5, "chat");
        assert_eq!(route_sync(&c), MemoryDestination::SharedWorld);
    }

    #[test]
    fn rule4_default_personal() {
        let c = ctx("今天天气不错", 0.5, "chat");
        assert_eq!(route_sync(&c), MemoryDestination::Personal);
    }

    #[test]
    fn persistence_keyword_work_detection() {
        assert!(contains_persistence_keyword("我在北京工作"));
        assert!(contains_persistence_keyword("用户的职业是老师"));
        assert!(!contains_persistence_keyword("今天吃什么"));
    }

    #[test]
    fn parse_verdict_plain_json() {
        let raw = r#"{"destination":"shared_world","reason":"用户偏好"}"#;
        let v = parse_verdict(raw).expect("应解析成功");
        assert_eq!(v.destination, "shared_world");
        assert_eq!(v.reason.as_deref(), Some("用户偏好"));
    }

    #[test]
    fn parse_verdict_with_code_fence() {
        let raw = "```json\n{\"destination\":\"personal\"}\n```";
        let v = parse_verdict(raw).expect("应解析成功");
        assert_eq!(v.destination, "personal");
        assert!(v.reason.is_none());
    }

    #[test]
    fn map_verdict_known_strings() {
        assert_eq!(
            map_verdict("personal").unwrap(),
            MemoryDestination::Personal
        );
        assert_eq!(
            map_verdict("relationship_fact").unwrap(),
            MemoryDestination::RelationshipFact
        );
        assert_eq!(
            map_verdict("shared_world").unwrap(),
            MemoryDestination::SharedWorld
        );
        assert_eq!(
            map_verdict("ephemeral").unwrap(),
            MemoryDestination::Ephemeral
        );
    }

    #[test]
    fn map_verdict_unknown_returns_none() {
        assert!(map_verdict("unknown_layer").is_none());
    }

    #[test]
    fn strip_fence_works() {
        assert_eq!(strip_code_fence("```json\n{\"x\":1}\n```"), r#"{"x":1}"#);
        assert_eq!(strip_code_fence(r#"{"x":1}"#), r#"{"x":1}"#);
    }

    /// 简单 mock LLM 客户端，用于 route_with_llm 单元测试
    struct MockRouterLlm {
        response: String,
    }

    #[async_trait]
    impl RouterLlmClient for MockRouterLlm {
        async fn complete(&self, _prompt: &str) -> VivianResult<String> {
            Ok(self.response.clone())
        }
    }

    #[tokio::test]
    async fn route_with_llm_skips_personal() {
        // Personal 不走 LLM，直接返回同步结果
        let c = ctx("今天天气不错", 0.5, "chat");
        let llm = MockRouterLlm {
            response: r#"{"destination":"ephemeral"}"#.to_string(),
        };
        let dest = route_with_llm(&c, &llm).await;
        assert_eq!(dest, MemoryDestination::Personal);
    }

    #[tokio::test]
    async fn route_with_llm_confirms_candidate() {
        // SharedWorld 候选 + LLM 返回 shared_world → 确认
        let c = ctx("我喜欢原神", 0.5, "chat");
        let llm = MockRouterLlm {
            response: r#"{"destination":"shared_world","reason":"用户偏好"}"#.to_string(),
        };
        let dest = route_with_llm(&c, &llm).await;
        assert_eq!(dest, MemoryDestination::SharedWorld);
    }

    #[tokio::test]
    async fn route_with_llm_overrides_candidate() {
        // SharedWorld 候选 + LLM 返回 personal → 覆盖为 personal
        let c = ctx("我喜欢原神", 0.5, "chat");
        let llm = MockRouterLlm {
            response: r#"{"destination":"personal"}"#.to_string(),
        };
        let dest = route_with_llm(&c, &llm).await;
        assert_eq!(dest, MemoryDestination::Personal);
    }

    #[tokio::test]
    async fn route_with_llm_falls_back_on_invalid_json() {
        let c = ctx("我喜欢原神", 0.5, "chat");
        let llm = MockRouterLlm {
            response: "这不是 JSON".to_string(),
        };
        let dest = route_with_llm(&c, &llm).await;
        // 降级为同步结果
        assert_eq!(dest, MemoryDestination::SharedWorld);
    }
}
