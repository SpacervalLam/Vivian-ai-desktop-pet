//! 写入时 LLM 增强：在记忆入库前抽取 description / keywords / importance / semantic_type。
//!
//! 核心思想：把所有 LLM 调用集中到写入路径，读路径零 LLM。
//! 调用方负责失败兜底（LLM 不可用时退化为规则化标签推断）。

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::VivianResult;
use crate::memory::types::SemanticType;
use crate::types::response::ChatMessage;

/// LLM 客户端抽象（与 `auto_extractor::ExtractorLlmClient` 同构）
#[async_trait]
pub trait EnricherLlmClient: Send + Sync {
    async fn complete(&self, prompt: &str) -> VivianResult<String>;
}

/// 为 `ModelRouter` 实现 LLM 客户端
#[async_trait]
impl EnricherLlmClient for crate::providers::ModelRouter {
    async fn complete(&self, prompt: &str) -> VivianResult<String> {
        let messages = vec![ChatMessage::user(prompt.to_string())];
        let schema = {
            let root = schemars::schema_for!(EnrichedMeta);
            serde_json::to_value(&root.schema).unwrap_or_else(|_| serde_json::json!({"type": "object"}))
        };
        self.generate(crate::providers::base::LLMRequest::new("memory", messages).with_json_schema(schema))
            .await
    }
}

/// LLM 抽取结果
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
pub struct EnrichedMeta {
    /// 一句话描述（用于检索时 manifest 与 LLM 选择）
    pub description: Option<String>,
    /// 关键词列表（写入 tags）
    #[serde(default)]
    pub keywords: Vec<String>,
    /// 重要性分数 0.0-1.0（LLM 判定）
    pub importance: Option<f64>,
    /// 语义类型字符串（由 LLM 分类，符合 SemanticType 枚举值）
    /// 解析后转为 `SemanticType`，未知值退化为 `General`。
    #[serde(default)]
    pub semantic_type: Option<String>,
    /// 记忆产生时的情绪余温（0-3 个标签），用于跨轮传递与摘要化输入
    #[serde(default)]
    pub mood_tags: Vec<String>,
    /// 长文本摘要（仅 content > 200 字时 LLM 输出，用于 embedding 替代原始文本以提升检索精度）
    #[serde(default)]
    pub summary: Option<String>,
}

/// 允许的情绪余温标签（与 emotion 子系统对齐）
pub const ALLOWED_MOOD_TAGS: &[&str] = &[
    "calm", "warm", "affectionate", "happy", "playful", "curious", "thoughtful",
    "touched", "proud", "worried", "lonely", "sad", "embarrassed", "tense",
    "annoyed", "determined",
];

fn sanitize_mood_tags(raw: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in raw {
        let lower = s.trim().to_lowercase();
        if lower.is_empty() {
            continue;
        }
        if !ALLOWED_MOOD_TAGS.contains(&lower.as_str()) {
            tracing::warn!("[MemoryEnricher] 未知 mood_tag: {lower}，已丢弃");
            continue;
        }
        if !out.iter().any(|x| x == &lower) {
            out.push(lower);
        }
        if out.len() >= 3 {
            break;
        }
    }
    out
}

impl EnrichedMeta {
    /// 返回解析后的 `SemanticType`，未知或缺失时返回 `General`。
    pub fn semantic_type_or_general(&self) -> SemanticType {
        self.semantic_type
            .as_deref()
            .and_then(SemanticType::from_str)
            .unwrap_or(SemanticType::General)
    }
}

/// 写入时 LLM 增强器
pub struct MemoryEnricher {
    llm: Arc<dyn EnricherLlmClient>,
}

impl MemoryEnricher {
    pub fn new(llm: Arc<dyn EnricherLlmClient>) -> Self {
        Self { llm }
    }

    /// 从 `llm` 构造；若 `llm` 为 None 返回 None。
    pub fn from_optional(llm: Option<Arc<dyn EnricherLlmClient>>) -> Option<Self> {
        llm.map(Self::new)
    }

    /// 抽取记忆元数据；失败时返回 Err，由调用方兜底。
    pub async fn enrich(&self, content: &str) -> VivianResult<EnrichedMeta> {
        let prompt = build_enrich_prompt(content);
        let resp = self.llm.complete(&prompt).await?;
        parse_enrich_response(&resp)
    }
}

/// 构造 LLM 抽取 prompt
///
/// 包含语义类型分类和"不保存什么"约束：
/// - 代码片段/技术细节（应通过 grep 获得而非记忆）
/// - 临时情绪波动（应写入 psychology 系统）
/// - 系统消息/错误日志
/// - 明确无价值的一般闲聊
///
/// 当 content > 200 字时，额外要求 LLM 输出 summary 字段（≤100 字摘要），
/// 用于 embedding 替代原始长文本以提升检索精度。
fn build_enrich_prompt(content: &str) -> String {
    let char_count = content.chars().count();
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
    let summary_instruction = if char_count > 200 {
        match lang_norm {
            "en" => "6. summary: Condense the text into a summary of ≤100 chars (preserve key info, for vector retrieval). Keep only factual points; drop pleasantries and repetitions.\n",
            "ja" => "6. summary：テキストを100字以内の要約にまとめる（重要情報を保持、ベクトル検索用）。事実要点のみ残し、挨拶や重複は省く。\n",
            _ => "6. summary：将文本浓缩为≤100 字的摘要（保留关键信息，用于向量检索）。只保留事实要点，去掉寒暄和重复内容。\n",
        }
    } else {
        ""
    };
    let summary_field = if char_count > 200 {
        ",\"summary\":\"...\""
    } else {
        ""
    };
    match lang_norm {
        "en" => format!(
            "You are a memory extractor for a companion AI. Analyze the following conversation text and extract metadata for long-term retrieval.\n\n\
            Text:\n{content}\n\n\
            ## Task\n\
            1. description: One-sentence description (≤30 chars), used to determine whether this memory is relevant to future conversations\n\
            2. keywords: 3-5 keywords for keyword retrieval (only for search, not used as tags)\n\
            3. importance: 0.0-1.0 importance score, unified scoring criteria:\n\
               - 0.9-1.0: Hard constraints, core identity attributes, health/allergy info, major relationship milestones\n\
               - 0.6-0.8: Long-term preferences, project context, key decisions, relationship events, shared experiences\n\
               - 0.3-0.5: General facts, contextual info, explanatory content\n\
               - 0.0-0.2: Small talk, greetings, temporary questions, one-time topics\n\
            4. semantic_type: Semantic classification, choose one of the following 7 categories:\n\
               - user: User identity/preference/personality (e.g., \"likes reading at night\")\n\
               - feedback: User feedback on Vivian's behavior (e.g., \"doesn't like being called 'baby'\")\n\
               - relationship: Relationship events (intimacy changes, shared memories, agreements)\n\
               - shared_memory: Shared conversations/events (e.g., \"discussed X last week\")\n\
               - project: User's current project/task (programming/work context)\n\
               - reference: External information pointers (links/bookmarks/citations)\n\
               - general: General conversation (no special semantic value)\n\
            5. mood_tags: The emotional afterglow you felt when remembering this. Choose 0-3 from the following 16 tags:\n\
               calm, warm, affectionate, happy, playful, curious, thoughtful,\n\
               touched, proud, worried, lonely, sad, embarrassed, tense,\n\
               annoyed, determined\n\
               (Return empty array if no clear emotional afterglow)\n\
            {summary_instruction}\n\
            ## What NOT to save (if the text falls into any of these, set importance to 0.0-0.1)\n\
            - Code snippets/technical details (should be obtained via grep, not stored as memory)\n\
            - Temporary emotional fluctuations (should be written to the emotion system, not memory)\n\
            - System messages/error logs\n\
            - Clearly valueless small talk\n\n\
            Output JSON only, no markdown code fences or extra explanations. Format:\n\
            {{\"description\":\"...\",\"keywords\":[\"...\"],\"importance\":0.5,\"semantic_type\":\"user\",\"mood_tags\":[\"happy\"]{summary_field}}}"
        ),
        "ja" => format!(
            "あなたはコンパニオンAIの記憶抽出器です。以下の会話テキストを分析し、長期検索用のメタデータを抽出してください。\n\n\
            テキスト：\n{content}\n\n\
            ## タスク\n\
            1. description：一文の説明（≤30字）、将来この記憶が会話に関連するか判断するために使用\n\
            2. keywords：3-5個のキーワード、キーワード検索用（検索のみに使用、タグとしては使用しない）\n\
            3. importance：0.0-1.0の重要度スコア、統一評価基準：\n\
               - 0.9-1.0：ハード制約、中核的身元属性、健康/アレルギー情報、重要な関係の節目\n\
               - 0.6-0.8：長期的な好み、プロジェクト背景、重要な決定、関係イベント、共有経験\n\
               - 0.3-0.5：一般的事実、コンテキスト情報、説明的内容\n\
               - 0.0-0.2：雑談、挨拶、一時的な質問、一回限りの話題\n\
            4. semantic_type：意味分類、以下の7種類から1つを選択：\n\
               - user：ユーザーの身元/好み/性格（例「夜に本を読むのが好き」）\n\
               - feedback：Vivianの行動に対するユーザーのフィードバック（例「ベビーと呼ばれるのが嫌い」）\n\
               - relationship：関係イベント（親密度の変化、共有思い出、約束）\n\
               - shared_memory：共有した会話/イベント（例「先週Xについて議論した」）\n\
               - project：ユーザーの現在のプロジェクト/タスク（プログラミング/作業コンテキスト）\n\
               - reference：外部情報ポインタ（リンク/ブックマーク/引用）\n\
               - general：一般的な会話（特別な意味的価値なし）\n\
            5. mood_tags：この出来事を記憶した時に感じた感情的余韻。以下の16個のタグから0-3個を選択：\n\
               calm, warm, affectionate, happy, playful, curious, thoughtful,\n\
               touched, proud, worried, lonely, sad, embarrassed, tense,\n\
               annoyed, determined\n\
               （明確な感情的余韻がない場合は空配列を返す）\n\
            {summary_instruction}\n\
            ## 保存しないもの（テキストが以下に該当する場合、importanceを0.0-0.1に設定）\n\
            - コードスニペット/技術的詳細（grepで取得すべき、記憶には入れない）\n\
            - 一時的な感情の揺らぎ（感情システムに書き込むべき、記憶には入れない）\n\
            - システムメッセージ/エラーログ\n\
            - 明らかに価値のない雑談\n\n\
            JSONのみを出力、マークダウンコードブロックや余分な説明は追加しない。形式：\n\
            {{\"description\":\"...\",\"keywords\":[\"...\"],\"importance\":0.5,\"semantic_type\":\"user\",\"mood_tags\":[\"happy\"]{summary_field}}}"
        ),
        _ => format!(
            "你是陪伴型 AI 的记忆抽取器。分析下面的对话文本，抽取用于长期检索的元数据。\n\n\
            文本：\n{content}\n\n\
            ## 任务\n\
            1. description：一句话描述（≤30 字），用于将来判断这条记忆是否与对话相关\n\
            2. keywords：3-5 个关键词，便于关键词检索（仅用于搜索，不作为标签）\n\
            3. importance：0.0-1.0 重要性分数，统一评分标准：\n\
               - 0.9-1.0：硬性约束、核心身份属性、健康/过敏信息、重大关系里程碑\n\
               - 0.6-0.8：长期偏好、项目背景、关键决策、关系事件、共同经历\n\
               - 0.3-0.5：一般事实、上下文信息、解释性内容\n\
               - 0.0-0.2：闲聊、寒暄、临时性问题、一次性话题\n\
            4. semantic_type：语义分类，从以下 7 类中选择一个：\n\
               - user：用户身份/偏好/性格（如\"喜欢晚上看书\"）\n\
               - feedback：用户对 Vivian 行为的反馈（如\"不喜欢被叫宝贝\"）\n\
               - relationship：关系事件（亲密度变化、共同回忆、约定）\n\
               - shared_memory：共同经历的对话/事件（如\"上周讨论了 X\"）\n\
               - project：用户当前的项目/任务（编程/工作上下文）\n\
               - reference：外部信息指针（链接/书签/引用）\n\
               - general：一般对话（无特殊语义价值）\n\
            5. mood_tags：你当时记住这件事时的情感余温，从以下 16 个标签中选 0-3 个：\n\
               calm, warm, affectionate, happy, playful, curious, thoughtful,\n\
               touched, proud, worried, lonely, sad, embarrassed, tense,\n\
               annoyed, determined\n\
               （无明确情感余温时返回空数组）\n\
            {summary_instruction}\n\
            ## 不保存什么（如果文本属于以下情况，importance 设为 0.0-0.1）\n\
            - 代码片段/技术细节（应通过 grep 获得，不入记忆）\n\
            - 临时情绪波动（应写入情绪系统，不入记忆）\n\
            - 系统消息/错误日志\n\
            - 明确无价值的一般闲聊\n\n\
            只输出 JSON，不要加 markdown 代码块标记或多余解释。格式：\n\
            {{\"description\":\"...\",\"keywords\":[\"...\"],\"importance\":0.5,\"semantic_type\":\"user\",\"mood_tags\":[\"happy\"]{summary_field}}}"
        ),
    }
}

/// 解析 LLM 响应为 `EnrichedMeta`
fn parse_enrich_response(resp: &str) -> VivianResult<EnrichedMeta> {
    let cleaned = strip_code_fence(resp);
    let mut meta: EnrichedMeta = serde_json::from_str(cleaned)
        .map_err(|e| crate::error::VivianError::Other(format!("解析 LLM 增强响应失败: {e}")))?;
    if let Some(imp) = meta.importance.as_mut() {
        *imp = imp.clamp(0.0, 1.0);
    }
    meta.keywords.retain(|s| !s.trim().is_empty());
    if let Some(desc) = meta.description.as_mut() {
        let trimmed = desc.trim();
        if trimmed.is_empty() {
            meta.description = None;
        } else {
            *desc = trimmed.to_string();
        }
    }
    // 校验 semantic_type 是否合法，非法值清空（退化为 General）
    if let Some(st) = meta.semantic_type.as_ref() {
        if SemanticType::from_str(st.trim()).is_none() {
            tracing::warn!(
                "[MemoryEnricher] LLM 返回未知的 semantic_type: {st}，退化为 general"
            );
            meta.semantic_type = None;
        }
    }
    meta.mood_tags = sanitize_mood_tags(std::mem::take(&mut meta.mood_tags));
    // summary 空字符串转 None
    if let Some(s) = meta.summary.as_ref() {
        if s.trim().is_empty() {
            meta.summary = None;
        } else {
            meta.summary = Some(s.trim().to_string());
        }
    }
    Ok(meta)
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

    #[test]
    fn parse_full_response() {
        let resp = r#"{"description":"用户喜欢晚上看书","keywords":["阅读","晚上","偏好"],"importance":0.85}"#;
        let meta = parse_enrich_response(resp).unwrap();
        assert_eq!(meta.description.as_deref(), Some("用户喜欢晚上看书"));
        assert_eq!(meta.keywords.len(), 3);
        assert!((meta.importance.unwrap() - 0.85).abs() < 1e-6);
    }

    #[test]
    fn parse_with_code_fence() {
        let resp = "```json\n{\"description\":\"x\",\"keywords\":[],\"importance\":0.3}\n```";
        let meta = parse_enrich_response(resp).unwrap();
        assert_eq!(meta.description.as_deref(), Some("x"));
    }

    #[test]
    fn parse_importance_clamped() {
        let resp = r#"{"description":"x","keywords":[],"importance":1.5}"#;
        let meta = parse_enrich_response(resp).unwrap();
        assert!((meta.importance.unwrap() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn parse_empty_keywords_filtered() {
        let resp = r#"{"description":"x","keywords":["","  "],"importance":0.3}"#;
        let meta = parse_enrich_response(resp).unwrap();
        assert!(meta.keywords.is_empty());
    }

    #[test]
    fn parse_empty_description_dropped() {
        let resp = r#"{"description":"  ","keywords":[],"importance":0.3}"#;
        let meta = parse_enrich_response(resp).unwrap();
        assert!(meta.description.is_none());
    }

    #[test]
    fn parse_with_semantic_type() {
        let resp = r#"{"description":"用户喜欢晚上看书","keywords":["阅读"],"importance":0.85,"semantic_type":"user"}"#;
        let meta = parse_enrich_response(resp).unwrap();
        assert_eq!(meta.semantic_type.as_deref(), Some("user"));
        assert_eq!(meta.semantic_type_or_general(), SemanticType::User);
    }

    #[test]
    fn parse_unknown_semantic_type_degrades_to_general() {
        let resp = r#"{"description":"x","keywords":[],"importance":0.3,"semantic_type":"unknown_type"}"#;
        let meta = parse_enrich_response(resp).unwrap();
        // 未知值应被清空，semantic_type_or_general 返回 General
        assert!(meta.semantic_type.is_none());
        assert_eq!(meta.semantic_type_or_general(), SemanticType::General);
    }

    #[test]
    fn parse_missing_semantic_type_defaults_to_general() {
        let resp = r#"{"description":"x","keywords":[],"importance":0.3}"#;
        let meta = parse_enrich_response(resp).unwrap();
        assert_eq!(meta.semantic_type_or_general(), SemanticType::General);
    }

    #[test]
    fn parse_summary_present() {
        let resp = r#"{"description":"x","keywords":[],"importance":0.5,"summary":"用户讨论了 Rust 异步编程的陷阱"}"#;
        let meta = parse_enrich_response(resp).unwrap();
        assert_eq!(meta.summary.as_deref(), Some("用户讨论了 Rust 异步编程的陷阱"));
    }

    #[test]
    fn parse_summary_empty_becomes_none() {
        let resp = r#"{"description":"x","keywords":[],"importance":0.5,"summary":"  "}"#;
        let meta = parse_enrich_response(resp).unwrap();
        assert!(meta.summary.is_none());
    }

    #[test]
    fn parse_summary_absent_is_none() {
        let resp = r#"{"description":"x","keywords":[],"importance":0.5}"#;
        let meta = parse_enrich_response(resp).unwrap();
        assert!(meta.summary.is_none());
    }

    #[test]
    fn build_prompt_includes_summary_for_long_content() {
        let long_content = "这是一段很长的文本".repeat(50); // > 200 字
        let prompt = build_enrich_prompt(&long_content);
        assert!(prompt.contains("summary"), "长文本 prompt 应包含 summary 指令");
    }

    #[test]
    fn build_prompt_excludes_summary_for_short_content() {
        let short_content = "短文本";
        let prompt = build_enrich_prompt(short_content);
        assert!(!prompt.contains("6. summary"), "短文本 prompt 不应包含 summary 指令");
    }
}
