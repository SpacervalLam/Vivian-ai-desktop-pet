//! 记忆工具 - save_memory, search_memory, clear_memory
//!
//! 通过 MemoryManager 操作统一记忆后端，与核心记忆系统共享数据。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::diary;
use crate::memory::agentic_recall::{
    recall_in_parallel, sanitize_queries, RecallQuery, RecallScope,
};
use crate::memory::{MemoryItem, MemoryManager, MemoryType, RetrievalStrategy};
use crate::tools::services::MemoryService;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult,
    ToolRiskTier,
};

/// 按角色 ID 获取 MemoryManager（多角色架构下的唯一入口）
///
/// char_id 为空时返回 None —— 工具执行时 ToolUseContext.char_id 由 Brain 注入，
/// 必须非空，否则说明调用链有 bug。
fn get_manager_for_context(context: &ToolUseContext) -> Option<MemoryManager> {
    if context.char_id.is_empty() {
        tracing::warn!("[memory_tools] ToolUseContext.char_id 为空，无法路由到角色记忆");
        return None;
    }
    crate::character_registry::get_memory_manager(&context.char_id)
}

fn parse_memory_type(category: &str) -> MemoryType {
    match category {
        "preference" | "preferences" => MemoryType::User,
        "feedback" => MemoryType::Feedback,
        "project" => MemoryType::Project,
        "reference" => MemoryType::Reference,
        "long_term" => MemoryType::LongTerm,
        "mid_term" => MemoryType::MidTerm,
        "short_term" => MemoryType::ShortTerm,
        _ => MemoryType::General,
    }
}

/// 单条检索与深查共用的结果整理：可见性过滤、类别过滤、截断、序列化与快速笔记。
async fn render_recall_results(
    mgr: &MemoryManager,
    items: Vec<MemoryItem>,
    limit: usize,
    category: Option<&str>,
    query: Option<&str>,
    scope: Option<&str>,
    plan: Vec<Value>,
    quick_notes_query: &str,
) -> ToolResult {
    let mut filtered = items;
    filtered.retain(|item| {
        !item.tags.iter().any(|tag| tag == "quick_note")
            && crate::memory::provenance::visible_to(&item.metadata, mgr.char_id())
    });
    if let Some(cat) = category.filter(|cat| !cat.is_empty()) {
        let cat_type = parse_memory_type(cat);
        filtered.retain(|item| {
            item.tags.iter().any(|tag| tag == cat)
                || parse_memory_type(&item.granularity) == cat_type
        });
    }
    filtered.truncate(limit);
    let total = filtered.len();
    let serialized: Vec<Value> = filtered
        .into_iter()
        .map(|item| {
            json!({
                "id": item.id,
                "content": item.content,
                "importance": item.importance,
                "tags": item.tags,
                "timestamp": crate::utils::prompt_time::format_prompt_time(item.timestamp),
                "record_type": crate::memory::provenance::recall_label(&item.metadata),
                "attribution": crate::memory::provenance::attribution(&item.metadata),
                "source_time": item.metadata["source_timestamp"].as_f64().filter(|t| t.is_finite() && *t > 0.0).map(crate::utils::prompt_time::format_prompt_time),
            })
        })
        .collect();
    let quick_notes_query = quick_notes_query.to_string();
    let embedding = mgr.embedding();
    let user_notes = tokio::task::spawn_blocking(move || {
        crate::user_quick_notes::search(&quick_notes_query, 3, Some(embedding))
    })
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or_default();
    ToolResult::standard_success(
        &format!("找到 {} 条记忆", total),
        Some(json!({
            "query": query,
            "scope": scope,
            "queries": plan,
            "category": category,
            "results": serialized,
            "total": total,
            "user_quick_notes": crate::user_quick_notes::context(&user_notes),
        })),
    )
}

pub struct SaveMemoryTool;

impl SaveMemoryTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SaveMemoryTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SaveMemoryTool {
    fn name(&self) -> &str {
        "save_memory"
    }

    fn description(&self) -> &str {
        "Save a memory entry to the long-term memory store. Optionally specify category, importance (0-1), and tags."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "将一条记忆保存到长期记忆库。可选指定 category（类别）、importance（重要性 0-1）和 tags（标签）。",
            "ja" => "記憶エントリを長期記憶ストアに保存する。任意で category（カテゴリ）、importance（重要度 0-1）、tags（タグ）を指定可能。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "记住这个\n把这个记下来\n别忘了这件事\n我要你记住\n以后记得提醒我\n存一下这个",
            "ja" => "これ覚えておいて\nメモしておいて\n忘れないでね",
            _ => "remember this\nnote this down\ndon't forget this",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Memory content"
                },
                "category": {
                    "type": "string",
                    "description": "Memory category, e.g. fact/preference/event",
                    "default": "general"
                },
                "importance": {
                    "type": "number",
                    "description": "Importance (0-1), default 0.5",
                    "minimum": 0.0,
                    "maximum": 1.0
                },
                "tags": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Tag list"
                }
            },
            "required": ["content"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "记忆内容"
                    },
                    "category": {
                        "type": "string",
                        "description": "记忆类别，例如 fact/preference/event",
                        "default": "general"
                    },
                    "importance": {
                        "type": "number",
                        "description": "重要性（0-1），默认 0.5",
                        "minimum": 0.0,
                        "maximum": 1.0
                    },
                    "tags": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "标签列表"
                    }
                },
                "required": ["content"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "記憶の内容"
                    },
                    "category": {
                        "type": "string",
                        "description": "記憶カテゴリ（例：fact/preference/event）",
                        "default": "general"
                    },
                    "importance": {
                        "type": "number",
                        "description": "重要度（0-1）、デフォルト 0.5",
                        "minimum": 0.0,
                        "maximum": 1.0
                    },
                    "tags": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "タグリスト"
                    }
                },
                "required": ["content"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let content = match input.get("content").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => return ValidationResult::failure("content 是必填项且不能为空", 2),
        };
        if let Some(imp) = input.get("importance").and_then(|v| v.as_f64()) {
            if !(0.0..=1.0).contains(&imp) {
                return ValidationResult::failure("importance 必须在 0.0 到 1.0 之间", 2);
            }
        }
        let _ = content;
        let mut data = input.clone();
        if data.get("category").is_none() {
            data["category"] = json!("general");
        }
        if data.get("importance").is_none() {
            data["importance"] = json!(0.5);
        }
        if data.get("tags").is_none() {
            data["tags"] = json!([]);
        }
        ValidationResult::success(Some(data))
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let category = args
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("general")
            .to_string();
        let importance = args
            .get("importance")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.5);
        let mut tags: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        // 强制注入 assistant 标记：LLM 通过工具写记忆属于该角色大脑的行为
        if !tags.iter().any(|t| t == "assistant") {
            tags.push("assistant".to_string());
        }

        if let Some(mgr) = get_manager_for_context(context) {
            let memory_type = parse_memory_type(&category);
            let char_id_for_mem = mgr.char_id().to_string();
            let meta = json!({
                "channel": "inner",
                "speaker": char_id_for_mem,
                "listener": char_id_for_mem,
                "perspective": "speaker",
            });
            match mgr
                .add_memory_with_metadata(content, memory_type, importance, tags.clone(), meta)
                .await
            {
                Ok(item) => {
                    return ToolResult::standard_success(
                        "记忆已保存",
                        Some(json!({
                            "id": item.id,
                            "content": item.content,
                            "category": category,
                            "importance": item.importance,
                            "tags": item.tags,
                            "timestamp": item.timestamp,
                        })),
                    );
                }
                Err(e) => {
                    return ToolResult::standard_error(
                        &format!("保存记忆失败: {e}"),
                        None,
                        None,
                    );
                }
            }
        }

        ToolResult::standard_error("记忆系统未初始化", None, None)
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    /// 权限风险等级：写入本地文件 / 持久化数据
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsWrite
    }

    /// 始终全量加载（核心工具）
    fn always_load(&self) -> bool {
        true
    }

    /// 搜索提示
    fn search_hint(&self) -> &str {
        "save memory and important events"
    }
}

/// search_memory 工具 - 搜索记忆
pub struct SearchMemoryTool;

impl SearchMemoryTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SearchMemoryTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SearchMemoryTool {
    fn name(&self) -> &str {
        "search_memory"
    }

    fn description(&self) -> &str {
        "Deep keyword recall over one unified index covering dialogues, session summaries, user facts, notes/knowledge, and diary entries. Call it only when your own evidence feels thin: the user refers to something you cannot place, a detail is fuzzy, or you are about to guess. Not needed when the answer is already in recent conversation. Give concrete keywords (names, places, dates, objects, events), not sentences or questions. You may pass up to 3 different keyword/scope pairs in one call; they are searched in parallel."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "在统一记忆索引中按关键词深入回忆，覆盖对话原文、会话摘要、用户事实、笔记/采集资料和日记。只在你自己的证据不够时才调用：用户提到你想不起来的事、细节模糊、或你正要靠猜的时候；答案已经在最近对话里就不必搜索。查询词由你自己决定，用人名、地点、日期、物品、事件等具体关键词，不要照抄整句或写成问句。一次最多给 3 组关键词/渠道，它们会并行检索。不要用它保存新信息（那是 save_memory 的职责）。",
            "ja" => "会話原文・セッション要約・ユーザー事実・ノート/収集資料・日記を横断する統一インデックスをキーワードで深く検索する。自分の根拠が薄いときだけ呼ぶ：ユーザーが思い出せない話をした、細部が曖昧、当て推量になりそうなとき。答えが最近の会話にすでにある場合は検索不要。検索語は自分で決め、人名・場所・日付・物・出来事などの具体的なキーワードを使う（文章丸ごとや疑問文は使わない）。1 回で最大 3 組のキーワード/スコープを渡せ、並列で検索される。新しい情報の保存には使わないこと（それは save_memory の役割）。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "你还记得我说过什么吗\n上次我们聊到哪儿了\n我之前跟你提过那件事\n我说过我不吃什么来着\n那件事我跟你讲过的",
            "ja" => "前に言ったこと覚えてる\nこの間話したやつ\n私が言ったあれ覚えてる",
            _ => "do you remember what I said\nwhat did we talk about last time\nI told you about this before",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Single search: concrete keywords"
                },
                "scope": {
                    "type": "string",
                    "enum": ["all", "diary", "note", "summary", "fact", "dialogue"],
                    "description": "Channel for the single query, default all"
                },
                "category": {
                    "type": "string",
                    "description": "Legacy category filter (optional)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results, default 10",
                    "minimum": 1
                }
            },
            "required": []
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "单条检索：具体关键词"
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["all", "diary", "note", "summary", "fact", "dialogue"],
                        "description": "单条检索的渠道，默认 all"
                    },
                    "category": {
                        "type": "string",
                        "description": "旧版类别过滤（可选）"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "返回结果的最大数量，默认 10",
                        "minimum": 1
                    }
                },
                "required": []
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "単一検索：具体的なキーワード"
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["all", "diary", "note", "summary", "fact", "dialogue"],
                        "description": "単一検索のスコープ（デフォルト all）"
                    },
                    "category": {
                        "type": "string",
                        "description": "旧カテゴリフィルター（任意）"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "結果の最大数、デフォルト 10",
                        "minimum": 1
                    }
                },
                "required": []
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
        if query.trim().is_empty() {
            return ValidationResult::failure("query 是必填项且不能为空", 2);
        }
        let mut data = input.clone();
        if data.get("limit").is_none() {
            data["limit"] = json!(10);
        }
        ValidationResult::success(Some(data))
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .unwrap_or("")
            .to_string();
        let scope = args
            .get("scope")
            .and_then(|v| v.as_str())
            .and_then(RecallScope::parse);
        let category = args.get("category").and_then(|v| v.as_str());
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        let Some(mgr) = get_manager_for_context(context) else {
            return ToolResult::standard_error("记忆系统未初始化", None, None);
        };

        // Fetch extra results so category filtering doesn't starve the result set
        let fetch_limit = if category.is_some() { limit * 3 + 20 } else { limit };
        let recalled = match scope {
            Some(scope) => {
                mgr.search_memories_filtered(
                    &query,
                    RetrievalStrategy::Auto,
                    fetch_limit,
                    &scope.filter(),
                )
                .await
            }
            None => {
                mgr.search_memories(&query, RetrievalStrategy::Auto, fetch_limit)
                    .await
            }
        };
        let items = match recalled {
            Ok(items) => items,
            Err(error) => {
                return ToolResult::standard_error(
                    &format!("搜索记忆失败: {error}"),
                    None,
                    None,
                );
            }
        };

        render_recall_results(
            &mgr,
            items,
            limit,
            category,
            Some(&query),
            scope.map(RecallScope::as_str),
            Vec::new(),
            &query,
        )
        .await
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    /// 权限风险等级：读取本地文件 / 持久化数据，无写入
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsRead
    }

    /// 始终全量加载（核心工具）
    fn always_load(&self) -> bool {
        true
    }

    /// 搜索提示
    fn search_hint(&self) -> &str {
        "search memory and historical conversations"
    }
}

/// deep_recall 工具 —— 一次并行查多组关键词/渠道（比 search_memory 更彻底）。
pub struct DeepRecallTool;

impl DeepRecallTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DeepRecallTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for DeepRecallTool {
    fn name(&self) -> &str {
        "deep_recall"
    }

    fn description(&self) -> &str {
        "Deep recall across diary, notes, summaries, facts and original dialogue in one call. Use ONLY when a single search_memory query or the recent conversation is not enough: the reference is fuzzy, several angles may matter, or you are about to guess. Provide 1-3 concrete keyword/scope pairs; they are searched in parallel. Prefer search_memory for a single straightforward lookup."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "一次并行深查日记、笔记、会话摘要、事实与对话原文。只在你已经查过一次、最近对话不够、指代模糊、需要多个侧面，或正要靠猜时使用。给 1-3 组具体关键词/渠道，它们会并行检索；单条简单查询优先用 search_memory。",
            "ja" => "日記・ノート・セッション要約・事実・会話原文を一度に並列で深く検索する。単発の search_memory や最近の会話では足りないとき（指代が曖昧、複数の側面が必要、当て推量になりそうなとき）だけ使う。1〜3 組の具体的なキーワード/スコープを渡すと並列検索される。単純な一回の検索には search_memory を使うこと。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "我一直想不起来那件事\n你翻翻日记和笔记\n上次那个到底是怎么回事\n把相关的记忆都找一下\n我是不是跟你说过",
            "en" => "I can't quite remember that\ncheck your diary and notes\nwhat was that thing again\nlook through everything related",
            "ja" => "あれ思い出せない\n日記とノートも見て\n前のあれって何だったっけ\n関係する記憶を全部探して",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "queries": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 3,
                    "description": "Up to 3 keyword/scope pairs searched in parallel",
                    "items": {
                        "type": "object",
                        "properties": {
                            "keywords": {"type": "string"},
                            "scope": {
                                "type": "string",
                                "enum": ["all", "diary", "note", "summary", "fact", "dialogue"]
                            }
                        },
                        "required": ["keywords"]
                    }
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of merged results, default 10",
                    "minimum": 1
                }
            },
            "required": ["queries"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "queries": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 3,
                        "description": "最多 3 组关键词/渠道，并行检索",
                        "items": {
                            "type": "object",
                            "properties": {
                                "keywords": {"type": "string"},
                                "scope": {
                                    "type": "string",
                                    "enum": ["all", "diary", "note", "summary", "fact", "dialogue"]
                                }
                            },
                            "required": ["keywords"]
                        }
                    },
                    "limit": {
                        "type": "integer",
                        "description": "合并后的最大结果数，默认 10",
                        "minimum": 1
                    }
                },
                "required": ["queries"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "queries": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 3,
                        "description": "最大 3 組のキーワード/スコープを並列検索",
                        "items": {
                            "type": "object",
                            "properties": {
                                "keywords": {"type": "string"},
                                "scope": {
                                    "type": "string",
                                    "enum": ["all", "diary", "note", "summary", "fact", "dialogue"]
                                }
                            },
                            "required": ["keywords"]
                        }
                    },
                    "limit": {
                        "type": "integer",
                        "description": "統合後の最大件数、デフォルト 10",
                        "minimum": 1
                    }
                },
                "required": ["queries"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let has_keywords = input
            .get("queries")
            .and_then(|v| v.as_array())
            .is_some_and(|queries| {
                queries.iter().any(|query| {
                    query
                        .get("keywords")
                        .and_then(|v| v.as_str())
                        .is_some_and(|keywords| !keywords.trim().is_empty())
                })
            });
        if !has_keywords {
            return ValidationResult::failure("queries 至少提供一个非空关键词", 2);
        }
        let mut data = input.clone();
        if data.get("limit").is_none() {
            data["limit"] = json!(10);
        }
        ValidationResult::success(Some(data))
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let queries: Vec<RecallQuery> = args
            .get("queries")
            .and_then(|v| v.as_array())
            .map(|queries| {
                queries
                    .iter()
                    .filter_map(|query| {
                        let keywords = query.get("keywords").and_then(|v| v.as_str())?.trim();
                        if keywords.is_empty() {
                            return None;
                        }
                        let scope = query
                            .get("scope")
                            .and_then(|v| v.as_str())
                            .and_then(RecallScope::parse)
                            .unwrap_or_default();
                        Some(RecallQuery {
                            keywords: keywords.to_string(),
                            scope,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let queries = sanitize_queries(queries);
        if queries.is_empty() {
            return ToolResult::standard_error("queries 至少提供一个非空关键词", None, None);
        }
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;
        let Some(mgr) = get_manager_for_context(context) else {
            return ToolResult::standard_error("记忆系统未初始化", None, None);
        };

        let items = recall_in_parallel(&mgr, &queries, mgr.char_id(), limit).await;
        let plan: Vec<Value> = queries
            .iter()
            .map(|query| json!({
                "keywords": query.keywords,
                "scope": query.scope.as_str(),
            }))
            .collect();
        let quick_notes_query = queries
            .first()
            .map(|query| query.keywords.as_str())
            .unwrap_or_default();
        render_recall_results(&mgr, items, limit, None, None, None, plan, quick_notes_query).await
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsRead
    }

    fn search_hint(&self) -> &str {
        "deep memory recall across diary notes summaries facts dialogues"
    }
}

/// recall_time 工具 —— 合并最近互动、按日期/时段回忆、日记与今日总结。
pub struct RecallTimeTool;

impl RecallTimeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RecallTimeTool {
    fn default() -> Self {
        Self::new()
    }
}

/// 旧工具没有 action 参数，按参数形状推断。
fn effective_recall_action(args: &Value) -> String {
    if let Some(action) = args.get("action").and_then(|v| v.as_str()).filter(|a| !a.is_empty()) {
        return action.to_string();
    }
    if args.get("date").is_some() || args.get("time_of_day").is_some() {
        "date".to_string()
    } else if args.get("hours").is_some() {
        "recent".to_string()
    } else {
        "today".to_string()
    }
}

#[async_trait]
impl Tool for RecallTimeTool {
    fn name(&self) -> &str {
        "recall_time"
    }

    fn description(&self) -> &str {
        "Recall past context by time: action=recent (hours/limit), action=date (date and/or time_of_day; returns both session summaries and diary entries), action=today (today's context summary). Use when the user points at a specific time or asks what happened that day."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "按时间回忆过去：action=recent（hours/limit，最近互动）、action=date（date 和/或 time_of_day，同时返回会话摘要与当天日记）、action=today（今日上下文总结）。用户指向具体时间或问\"那天/上周/昨天发生了什么\"时使用。",
            "ja" => "時間で過去を振り返る：action=recent（hours/limit、最近のやり取り）、action=date（date と time_of_day のいずれか／両方。セッション要約と当日の日記をまとめて返す）、action=today（今日のコンテキスト要約）。特定の時点を指す質問で使う。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "我们刚才聊了什么\n前几天我们说了啥\n昨天下午我在干嘛\n那天发生了什么\n今天都做了什么",
            "en" => "what were we just talking about\nwhat did we do that day\nyesterday afternoon\nwhat happened last week\ntoday's summary",
            "ja" => "さっき何を話した\nあの日何があった\n昨日の午後は\n先週の出来事\n今日のまとめ",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["recent", "date", "today"], "description": "Which time view to recall"},
                "hours": {"type": "integer", "minimum": 1, "description": "Lookback hours for action=recent, default 24"},
                "date": {"type": "string", "description": "YYYY-MM-DD for action=date"},
                "time_of_day": {"type": "string", "enum": ["morning", "afternoon", "evening", "night"], "description": "Optional time-of-day filter for action=date"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 30, "description": "Maximum results, default 10"}
            },
            "required": ["action"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["recent", "date", "today"], "description": "要回忆的时间视图"},
                    "hours": {"type": "integer", "minimum": 1, "description": "action=recent 时回溯多少小时，默认 24"},
                    "date": {"type": "string", "description": "action=date 的日期（YYYY-MM-DD）"},
                    "time_of_day": {"type": "string", "enum": ["morning", "afternoon", "evening", "night"], "description": "action=date 的可选时段过滤"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 30, "description": "返回条数上限，默认 10"}
                },
                "required": ["action"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["recent", "date", "today"], "description": "振り返る時間ビュー"},
                    "hours": {"type": "integer", "minimum": 1, "description": "action=recent の遡り時間（デフォルト 24）"},
                    "date": {"type": "string", "description": "action=date の日付（YYYY-MM-DD）"},
                    "time_of_day": {"type": "string", "enum": ["morning", "afternoon", "evening", "night"], "description": "action=date の任意の時間帯フィルター"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 30, "description": "最大件数、デフォルト 10"}
                },
                "required": ["action"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> ValidationResult {
        let action = effective_recall_action(input);
        match action.as_str() {
            "recent" => GetRecentInteractionsTool.validate_input(input, ctx).await,
            "today" => SummarizeTodayContextTool.validate_input(input, ctx).await,
            "date" => {
                let summaries = RecallByDateTimeTool.validate_input(input, ctx).await;
                if !summaries.result { return summaries; }
                if input.get("date").and_then(|v| v.as_str()).is_some_and(|date| !date.is_empty()) {
                    ReadDiaryByDateTool.validate_input(input, ctx).await
                } else {
                    ValidationResult::success(Some(input.clone()))
                }
            }
            other => ValidationResult::failure(
                format!("action must be recent/date/today (got {other})"), 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let action = effective_recall_action(&args);
        match action.as_str() {
            "recent" => GetRecentInteractionsTool.call(args, ctx).await,
            "today" => SummarizeTodayContextTool.call(args, ctx).await,
            "date" => {
                let has_date = args.get("date").and_then(|v| v.as_str()).is_some_and(|date| !date.is_empty());
                let summaries = RecallByDateTimeTool.call(args.clone(), ctx).await;
                let diary = if has_date {
                    ReadDiaryByDateTool.call(args, ctx).await
                } else {
                    ToolResult::standard_success("没有指定日期，跳过日记检索", Some(json!({"diaries": []})))
                };
                let message = format!(
                    "{}；{}",
                    summaries.data.as_ref().and_then(|data| data["message"].as_str()).unwrap_or("会话摘要"),
                    diary.data.as_ref().and_then(|data| data["message"].as_str()).unwrap_or("日记"),
                );
                ToolResult::standard_success(
                    &message,
                    Some(json!({
                        "diary": diary.data,
                        "summaries": summaries.data,
                    })),
                )
            }
            other => ToolResult::standard_error(
                &format!("不支持的 action: {other}"), Some("InvalidAction"), None),
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsRead
    }

    fn search_hint(&self) -> &str {
        "recall time yesterday that day recent interactions diary today summary"
    }
}

/// get_recent_interactions 工具 - 获取最近互动
pub struct GetRecentInteractionsTool;
impl GetRecentInteractionsTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GetRecentInteractionsTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GetRecentInteractionsTool {
    fn name(&self) -> &str {
        "get_recent_interactions"
    }

    fn description(&self) -> &str {
        "Get recent dialogue records within a specified time range. For physical pet dragging/flinging/edge-hit observations, use get_pet_interactions."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "获取指定时间范围内的最近对话记录；桌宠拖动、甩飞和碰撞的物理观测请用 get_pet_interactions。",
            "ja" => "指定期間内の最近の会話記録を取得。ペットのドラッグ・投げ・衝突の観測は get_pet_interactions を使用。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "我们刚才聊了什么\n最近我们说了些什么\n前几轮对话的内容\n你刚才是不是说过",
            "en" => "what were we just talking about\nwhat did we discuss recently\nearlier in this conversation",
            "ja" => "さっき何話したっけ\n最近何を話したか\nこの前の会話の内容",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "hours": {
                    "type": "integer",
                    "description": "Number of past hours to query for interactions, default 24",
                    "minimum": 1
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of interactions to return, default 20",
                    "minimum": 1
                }
            }
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "hours": {
                        "type": "integer",
                        "description": "查询过去多少小时的互动，默认 24",
                        "minimum": 1
                    },
                    "limit": {
                        "type": "integer",
                        "description": "返回互动记录的最大数量，默认 20",
                        "minimum": 1
                    }
                }
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "hours": {
                        "type": "integer",
                        "description": "過去何時間分のやり取りを検索するか、デフォルト 24",
                        "minimum": 1
                    },
                    "limit": {
                        "type": "integer",
                        "description": "返すやり取りの最大数、デフォルト 20",
                        "minimum": 1
                    }
                }
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let mut data = input.clone();
        if data.get("hours").is_none() {
            data["hours"] = json!(24);
        }
        if data.get("limit").is_none() {
            data["limit"] = json!(20);
        }
        ValidationResult::success(Some(data))
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let hours = args.get("hours").and_then(|v| v.as_u64()).unwrap_or(24);
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

        let mgr = match get_manager_for_context(context) {
            Some(m) => m,
            None => {
                return ToolResult::standard_error(
                    "记忆系统未初始化（角色未注册）",
                    None,
                    None,
                );
            }
        };

        match MemoryService::get_recent_interactions(&mgr, hours, limit).await {
            Ok(result) => {
                let count = result
                    .get("count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                ToolResult::standard_success(
                    &format!("过去 {} 小时有 {} 次互动记录", hours, count),
                    Some(result),
                )
            }
            Err(e) => ToolResult::standard_error(&format!("获取互动记录失败: {}", e), None, None),
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    /// 权限风险等级：读取本地文件 / 持久化数据，无写入
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsRead
    }
}

/// summarize_today_context 工具 - 总结今日上下文
pub struct SummarizeTodayContextTool;

impl SummarizeTodayContextTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SummarizeTodayContextTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SummarizeTodayContextTool {
    fn name(&self) -> &str {
        "summarize_today_context"
    }

    fn description(&self) -> &str {
        "Generate a today's context summary, including date, user preferences, today's memories, and Vivian's current state."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "生成今日上下文总结，包含日期、用户偏好、今日记忆和 Vivian 当前状态。",
            "ja" => "今日のコンテキストサマリーを生成する。日付、ユーザー設定、今日の記憶、Vivian の現在の状態を含む。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "今天都发生了什么\n总结一下今天\n我今天做了些什么\n今天过得怎么样",
            "en" => "what happened today\nsummarize today\nwhat did I do today",
            "ja" => "今日何があった\n今日をまとめて\n今日はどうだった",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "include_preferences": {
                    "type": "boolean",
                    "description": "Whether to include user preferences, default true"
                },
                "include_recent_events": {
                    "type": "boolean",
                    "description": "Whether to include recent events, default true"
                },
                "include_emotional_state": {
                    "type": "boolean",
                    "description": "Whether to include emotional state, default true"
                }
            }
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "include_preferences": {
                        "type": "boolean",
                        "description": "是否包含用户偏好，默认 true"
                    },
                    "include_recent_events": {
                        "type": "boolean",
                        "description": "是否包含最近事件，默认 true"
                    },
                    "include_emotional_state": {
                        "type": "boolean",
                        "description": "是否包含情绪状态，默认 true"
                    }
                }
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "include_preferences": {
                        "type": "boolean",
                        "description": "ユーザー設定を含めるか、デフォルト true"
                    },
                    "include_recent_events": {
                        "type": "boolean",
                        "description": "最近のイベントを含めるか、デフォルト true"
                    },
                    "include_emotional_state": {
                        "type": "boolean",
                        "description": "感情状態を含めるか、デフォルト true"
                    }
                }
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let mut data = input.clone();
        if data.get("include_preferences").is_none() {
            data["include_preferences"] = json!(true);
        }
        if data.get("include_recent_events").is_none() {
            data["include_recent_events"] = json!(true);
        }
        if data.get("include_emotional_state").is_none() {
            data["include_emotional_state"] = json!(true);
        }
        ValidationResult::success(Some(data))
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let include_prefs = args
            .get("include_preferences")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let include_events = args
            .get("include_recent_events")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let include_emotion = args
            .get("include_emotional_state")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let mgr = match get_manager_for_context(context) {
            Some(m) => m,
            None => {
                return ToolResult::standard_error(
                    "记忆系统未初始化（角色未注册）",
                    None,
                    None,
                );
            }
        };

        match MemoryService::summarize_today_context(&mgr).await {
            Ok(result) => {
                let date = result.get("date").and_then(|v| v.as_str()).unwrap_or("");
                let weekday = result
                    .get("weekday")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let weekday_cn = weekday_to_chinese(weekday);

                let mut summary_parts: Vec<String> =
                    vec![format!("今天是 {}，{}", date, weekday_cn)];

                let mut data = json!({});
                data["date"] = json!(date);
                data["weekday"] = json!(weekday_cn);

                if include_prefs {
                    let prefs = result.get("preferences").cloned().unwrap_or(json!([]));
                    let pref_count = prefs.as_array().map(|a| a.len()).unwrap_or(0);
                    data["preferences"] = prefs.clone();
                    summary_parts.push(format!("- 已记住 {} 条主人的偏好", pref_count));
                }

                if include_events {
                    let today_memories = result
                        .get("today_memories")
                        .cloned()
                        .unwrap_or(json!([]));
                    let mem_count = today_memories.as_array().map(|a| a.len()).unwrap_or(0);
                    data["today_memories"] = today_memories;
                    summary_parts.push(format!("- 今天新增 {} 条记忆", mem_count));
                }

                if include_emotion {
                    let pet_state = result.get("pet_state").cloned().unwrap_or(json!({}));
                    let mood = pet_state
                        .get("mood")
                        .and_then(|v| v.as_str())
                        .unwrap_or("idle")
                        .to_string();
                    data["pet_state"] = pet_state;
                    summary_parts.push(format!("- Vivian 当前状态：{}", mood));
                }

                data["summary"] = json!(summary_parts);

                let summary = summary_parts.join("\n");
                ToolResult::standard_success(
                    &format!("今日上下文总结：\n{}", summary),
                    Some(data),
                )
            }
            Err(e) => ToolResult::standard_error(&format!("总结失败: {}", e), None, None),
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    /// 权限风险等级：读取本地文件 / 持久化数据，无写入
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsRead
    }
}

/// read_diary_by_date 工具 - 按日期读取日记
pub struct ReadDiaryByDateTool;

impl ReadDiaryByDateTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ReadDiaryByDateTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ReadDiaryByDateTool {
    fn name(&self) -> &str {
        "read_diary_by_date"
    }

    fn description(&self) -> &str {
        "Read diary entries for the specified date (YYYY-MM-DD)."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "读取指定日期（YYYY-MM-DD）的日记条目。",
            "ja" => "指定された日付（YYYY-MM-DD）の日記エントリを読み取る。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "看看我昨天的日记\n我那天写了什么\n翻一下日记\n读读那天的记录",
            "en" => "read my diary from yesterday\nwhat did I write that day\nshow me that diary entry",
            "ja" => "昨日の日記を見せて\nあの日の日記は何を書いた\n日記を読んで",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "date": {
                    "type": "string",
                    "description": "Diary date in YYYY-MM-DD format"
                }
            },
            "required": ["date"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "date": {
                        "type": "string",
                        "description": "日记日期，格式为 YYYY-MM-DD"
                    }
                },
                "required": ["date"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "date": {
                        "type": "string",
                        "description": "日記の日付（YYYY-MM-DD 形式）"
                    }
                },
                "required": ["date"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let date = match input.get("date").and_then(|v| v.as_str()) {
            Some(d) if !d.is_empty() => d.to_string(),
            _ => return ValidationResult::failure("date 是必填项且不能为空", 2),
        };
        let _ = date;
        ValidationResult::success(None)
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let date = args.get("date").and_then(|v| v.as_str()).unwrap_or("");

        match diary::get_entries(&context.char_id, Some(date)) {
            Ok(diaries) => {
                let count = diaries.len();
                if count == 0 {
                    return ToolResult::standard_success(
                        &format!("{} 的日记空空如也", date),
                        Some(json!({
                            "date": date,
                            "diaries": [],
                            "count": 0,
                        })),
                    );
                }
                let diary_values: Vec<Value> = diaries
                    .iter()
                    .map(|d| {
                        json!({
                            "id": d.id,
                            "date": d.date,
                            "content": d.content,
                            "key_events": d.key_events,
                            "mood_tag": d.mood_tag,
                            "word_count": d.word_count,
                            "interaction_count": d.interaction_count,
                            "created_at": d.created_at,
                        })
                    })
                    .collect();
                let preview = truncate_preview(
                    diaries[0].content.as_str(),
                    80,
                );
                ToolResult::standard_success(
                    &format!("找到 {} 条 {} 的日记：{}", count, date, preview),
                    Some(json!({
                        "date": date,
                        "diaries": diary_values,
                        "count": count,
                    })),
                )
            }
            Err(e) => ToolResult::standard_error(&format!("读取日记失败: {}", e), None, None),
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    /// 权限风险等级：读取本地文件 / 持久化数据，无写入
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsRead
    }
}

/// recall_by_date_time 工具 - 按日期和/或时段检索会话摘要
pub struct RecallByDateTimeTool;

impl RecallByDateTimeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RecallByDateTimeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for RecallByDateTimeTool {
    fn name(&self) -> &str {
        "recall_by_date_time"
    }

    fn description(&self) -> &str {
        "Retrieve session summaries by date (YYYY-MM-DD) and/or time of day (morning/afternoon/evening/night). \
         Use when the user asks time-pointing questions like 'what did we talk about that day' or \
         'what happened last week evening'. Provide at least one of date or time_of_day."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "按日期（YYYY-MM-DD）和/或时段（morning/afternoon/evening/night）检索会话摘要。\
            当用户问时间指向性问题如\"那天我们聊了什么\"或\"上周晚上发生了什么\"时使用。\
            date 和 time_of_day 至少提供一个。",
            "ja" => "日付（YYYY-MM-DD）や時間帯（morning/afternoon/evening/night）でセッションサマリーを取得する。\
            「あの日何を話したっけ」「先週の夜何があったっけ」など時間を指す質問で使用。\
            date または time_of_day の少なくとも一方を提供すること。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "那天我们聊了什么\n上周晚上发生了什么\n昨天下午我在干嘛\n前天的事你还记得吗\n上个月那次",
            "en" => "what did we talk about that day\nwhat happened last week evening\nwhat was I doing yesterday afternoon",
            "ja" => "あの日何を話したっけ\n先週の夜何があった\n昨日の午後は何してた",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "date": {
                    "type": "string",
                    "description": "Date in YYYY-MM-DD format (optional, but at least one of date or time_of_day must be provided)"
                },
                "time_of_day": {
                    "type": "string",
                    "enum": ["morning", "afternoon", "evening", "night"],
                    "description": "Time of day filter (optional)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results, default 10",
                    "minimum": 1,
                    "maximum": 30
                }
            },
            "required": []
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "date": {
                        "type": "string",
                        "description": "日期，格式为 YYYY-MM-DD（可选，但 date 和 time_of_day 至少提供一个）"
                    },
                    "time_of_day": {
                        "type": "string",
                        "enum": ["morning", "afternoon", "evening", "night"],
                        "description": "时段过滤（可选）"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "返回结果的最大数量，默认 10",
                        "minimum": 1,
                        "maximum": 30
                    }
                },
                "required": []
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "date": {
                        "type": "string",
                        "description": "日付（YYYY-MM-DD 形式、任意。ただし date または time_of_day の少なくとも一方を提供すること）"
                    },
                    "time_of_day": {
                        "type": "string",
                        "enum": ["morning", "afternoon", "evening", "night"],
                        "description": "時間帯フィルター（任意）"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "結果の最大数、デフォルト 10",
                        "minimum": 1,
                        "maximum": 30
                    }
                },
                "required": []
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let date = input.get("date").and_then(|v| v.as_str()).unwrap_or("");
        let tod = input.get("time_of_day").and_then(|v| v.as_str()).unwrap_or("");
        if date.is_empty() && tod.is_empty() {
            return ValidationResult::failure("date 和 time_of_day 至少填一个", 2);
        }
        if !tod.is_empty()
            && !matches!(tod, "morning" | "afternoon" | "evening" | "night")
        {
            return ValidationResult::failure(
                "time_of_day 必须是 morning/afternoon/evening/night 之一",
                2,
            );
        }
        let mut data = input.clone();
        if data.get("limit").is_none() {
            data["limit"] = json!(10);
        }
        ValidationResult::success(Some(data))
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let date = args.get("date").and_then(|v| v.as_str()).unwrap_or("");
        let time_of_day = args.get("time_of_day").and_then(|v| v.as_str()).unwrap_or("");
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        let mgr = match get_manager_for_context(context) {
            Some(m) => m,
            None => return ToolResult::standard_error("记忆系统未初始化", None, None),
        };

        let all = match mgr.get_all_memories().await {
            Ok(v) => v,
            Err(e) => return ToolResult::standard_error(&format!("读取记忆失败: {e}"), None, None),
        };

        // 筛选 session_summary 且匹配 date/time_of_day 的条目
        let mut matched: Vec<_> = all
            .iter()
            .filter(|m| {
                let is_summary = m.tags.iter().any(|t| t == "session_summary")
                    || m.metadata
                        .get("memory_type")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "session_summary")
                        .unwrap_or(false);
                if !is_summary {
                    return false;
                }

                // 日期匹配：date_label 或 date_labels 包含目标日期
                if !date.is_empty() {
                    let primary = m.date_label().map(|d| d == date).unwrap_or(false);
                    let in_array = m
                        .metadata
                        .get("date_labels")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().any(|v| v.as_str() == Some(date)))
                        .unwrap_or(false);
                    if !primary && !in_array {
                        return false;
                    }
                }

                // 时段匹配：time_of_day 或 time_of_days 包含目标时段
                if !time_of_day.is_empty() {
                    let primary = m.time_of_day().map(|t| t == time_of_day).unwrap_or(false);
                    let in_array = m
                        .metadata
                        .get("time_of_days")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().any(|v| v.as_str() == Some(time_of_day)))
                        .unwrap_or(false);
                    if !primary && !in_array {
                        return false;
                    }
                }

                true
            })
            .collect::<Vec<_>>();

        // 按时间戳降序（最新在前）
        matched.sort_by(|a, b| {
            b.timestamp
                .partial_cmp(&a.timestamp)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let total = matched.len();
        matched.truncate(limit);

        if matched.is_empty() {
            let mut hint = String::new();
            if !date.is_empty() {
                hint.push_str(date);
            }
            if !date.is_empty() && !time_of_day.is_empty() {
                hint.push(' ');
            }
            if !time_of_day.is_empty() {
                hint.push_str(time_of_day);
            }
            return ToolResult::standard_success(
                &format!("没有找到匹配 {} 的会话摘要", hint),
                Some(json!({
                    "date": date,
                    "time_of_day": time_of_day,
                    "summaries": [],
                    "count": 0,
                })),
            );
        }

        let summaries: Vec<Value> = matched
            .iter()
            .map(|m| {
                let mood = m.mood_tags();
                json!({
                    "id": m.id,
                    "content": m.content,
                    "importance": m.importance,
                    "mood_tags": mood,
                    "date_label": m.date_label(),
                    "time_of_day": m.time_of_day(),
                    "timestamp": m.timestamp,
                })
            })
            .collect();

        let preview = truncate_preview(&matched[0].content, 80);
        ToolResult::standard_success(
            &format!("找到 {} 条匹配的会话摘要：{}", total, preview),
            Some(json!({
                "date": date,
                "time_of_day": time_of_day,
                "summaries": summaries,
                "count": total,
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    /// 权限风险等级：读取本地文件 / 持久化数据，无写入
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsRead
    }

    fn search_hint(&self) -> &str {
        "recall session summaries by date or time of day"
    }
}

/// memory_md 工具 - 维护角色的长期记忆笔记（memory.md）
///
/// 与 save_memory（结构化记忆条目）互补：这里维护的是一份每轮全量注入上下文的
/// markdown 笔记，由角色自己跨会话沉淀与整理。文件路径按角色隔离
/// （`%APPDATA%\Vivian\characters\<char_id>\memory\memory.md`）。
///
/// 维护纪律写在 description（中英日三语）而非 system prompt——与 work_todo_write
/// 同一模式：工具 schema 本身就是规则的载体。
pub struct MemoryMdTool;

impl MemoryMdTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MemoryMdTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for MemoryMdTool {
    fn name(&self) -> &str {
        "memory_md"
    }

    fn description(&self) -> &str {
        "View or manually tidy your long-term memory notes (memory.md). \
         Daily sedimentation is automatic (the system writes after each reply) — you do NOT need to call this to record things. \
         Use only when the user explicitly asks to view or tidy the notes. \
         Actions: \"read\" (current notes), \"history\" (local versions), \"read_version\" (historical content), \"rewrite\" (concise full rewrite using the current read revision)."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "查看或手动整理你的长期记忆笔记（memory.md）。\
                日常沉淀由系统在每轮回复后自动完成，**不需要调用本工具来记录**。\
                仅在用户明确要求查看或整理笔记时使用。\
                action：\"read\" 查看当前笔记 / \"history\" 查看本地历史 / \"read_version\" 读取旧版本 / \"rewrite\" 基于当前版本精简重写。",
            "ja" => "長期メモ（memory.md）の表示・手動整理。\
                日常の追記はシステムが毎回の返信後に自動で行うため、記録のために本ツールを呼ぶ必要はない。\
                ユーザーが明示的に見たい・整理したいと言った時だけ使用。\
                action：\"read\"（表示）/ \"rewrite\"（全文リライト：重複排除・整理、簡潔に）。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "看看你的长期记忆笔记\n整理一下你的记忆笔记\n你之前答应过我的事（查笔记）",
            "ja" => "長期メモを見せて\nメモを整理して\n前に約束したこと（メモ確認）",
            _ => "show me your long-term memory notes\ntidy up your memory notes\nwhat did you promise before (check notes)",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read", "rewrite", "history", "read_version"],
                    "description": "read=view notes, rewrite=full rewrite to merge/deduplicate/trim (must stay concise)"
                },
                "content": {
                    "type": "string",
                    "description": "The full rewritten note body (markdown). Required for rewrite."
                },
                "expected_revision": {
                    "type": "string",
                    "description": "For rewrite, first read the notes and pass the exact revision returned by read."
                },
                "revision": {
                    "type": "string",
                    "description": "For read_version, pass a commit ID returned by history. history lists recent local Git snapshots."
                }
            },
            "required": ["action"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["read", "rewrite", "history", "read_version"],
                        "description": "read=查看 / rewrite=全文重写（合并去重、删过时，需精简）"
                    },
                    "content": {
                        "type": "string",
                        "description": "重写后的完整笔记正文（markdown）。rewrite 必填。"
                    },
                    "expected_revision": {
                        "type": "string",
                        "description": "rewrite 必填：先 read，再原样传入 read 返回的 revision，避免覆盖新增约定。"
                    },
                    "revision": {
                        "type": "string",
                        "description": "history 查看最近的本地 Git 历史；read_version 必填 history 返回的 commit，以读取该版本。"
                    }
                },
                "required": ["action"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["read", "rewrite", "history", "read_version"],
                        "description": "read=表示 / rewrite=全文リライト（重複排除・整理、簡潔に）"
                    },
                    "content": {
                        "type": "string",
                        "description": "リライト後の全文（markdown）。rewrite では必須。"
                    },
                    "expected_revision": {
                        "type": "string",
                        "description": "rewrite 必須：先に read し、返された revision をそのまま渡す。"
                    },
                    "revision": {
                        "type": "string",
                        "description": "history で履歴を表示。read_version では history が返す commit を指定する。"
                    }
                },
                "required": ["action"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        if !matches!(action, "read" | "rewrite" | "history" | "read_version") {
            return ValidationResult::failure("action 必须是 read/rewrite/history/read_version 之一", 2);
        }
        if action == "read_version" && input.get("revision").and_then(Value::as_str).is_none_or(|revision|
            revision.len() != 40 || !revision.bytes().all(|b| b.is_ascii_hexdigit())) {
            return ValidationResult::failure("read_version 必须携带 history 返回的 commit", 2);
        }
        if action == "rewrite" {
            let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if content.trim().is_empty() {
                return ValidationResult::failure("rewrite 时 content 不能为空", 2);
            }
            if input.get("expected_revision").and_then(Value::as_str).is_none_or(|revision|
                revision.len() != 64 || !revision.bytes().all(|b| b.is_ascii_hexdigit())) {
                return ValidationResult::failure("rewrite 必须携带 read 返回的 expected_revision，请先读取笔记", 2);
            }
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("read");
        let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");

        if context.char_id.is_empty() {
            return ToolResult::standard_error(
                "ToolUseContext.char_id 为空，无法路由到角色记忆笔记",
                None,
                None,
            );
        }
        let path = crate::memory::memory_md::memory_md_path(&context.char_id);

        match action {
            "read" => match crate::memory::memory_md::read_memory_md_snapshot(&context.char_id) {
                Ok((Some(md), revision)) => ToolResult::standard_success(
                    &format!("当前长期记忆笔记全文：\n{md}"),
                    Some(json!({
                        "path": path.display().to_string(),
                        "content": md,
                        "revision": revision,
                    })),
                ),
                Ok((None, revision)) => ToolResult::standard_success(
                    "长期记忆笔记还是空的。值得记的相处约定/承诺/教训/梗会在每轮回复后由系统自动沉淀。",
                    Some(json!({
                        "path": path.display().to_string(),
                        "content": "",
                        "revision": revision,
                    })),
                ),
                Err(error) => ToolResult::standard_error(&error, None, None),
            },
            "rewrite" => {
                let revision = args.get("expected_revision").and_then(Value::as_str).unwrap_or("");
                match crate::memory::memory_md::write_memory_md_revision(&context.char_id, revision, content) {
                    Ok(()) => ToolResult::standard_success(
                        &format!(
                            "长期记忆笔记已重写为 {} 行。",
                            content.trim().lines().count()
                        ),
                        Some(json!({ "path": path.display().to_string() })),
                    ),
                    Err(e) => ToolResult::standard_error(&e, None, None),
                }
            }
            "history" | "read_version" => {
                let revision = if action == "read_version" { args.get("revision").and_then(Value::as_str) } else { None };
                let character = context.char_id.clone();
                let revision = revision.map(str::to_owned);
                match tokio::task::spawn_blocking(move ||
                    crate::memory::git_history::runtime_character_versions(&character, revision.as_deref())).await {
                    Ok(Ok(data)) => ToolResult::standard_success("已读取本地记忆历史。恢复历史内容前先 read 当前笔记，再使用当前 expected_revision 执行 rewrite。", Some(data)),
                    Ok(Err(error)) => ToolResult::standard_error(&error, None, None),
                    Err(error) => ToolResult::standard_error(&error.to_string(), None, None),
                }
            }
            _ => ToolResult::standard_error("未知 action", None, None),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    /// 权限风险等级：写入本地文件 / 持久化数据
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsWrite
    }

    /// 日常沉淀由系统在每轮回复后的反思步自动完成（无需本工具）；
    /// 工具仅作用户明确要求查看 / 整理时的手动入口，避免占满每轮 schema 预算。
    fn should_defer(&self) -> bool {
        true
    }

    /// 搜索提示
    fn search_hint(&self) -> &str {
        "view or manually tidy long-term memory notes"
    }
}

/// 截取预览文本（按字符计数）
fn truncate_preview(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > max_chars {
        format!("{}...", chars.iter().take(max_chars).collect::<String>())
    } else {
        s.to_string()
    }
}

/// 将英文星期转为中文
fn weekday_to_chinese(weekday: &str) -> String {
    match weekday {
        "Monday" => "星期一".to_string(),
        "Tuesday" => "星期二".to_string(),
        "Wednesday" => "星期三".to_string(),
        "Thursday" => "星期四".to_string(),
        "Friday" => "星期五".to_string(),
        "Saturday" => "星期六".to_string(),
        "Sunday" => "星期日".to_string(),
        _ => weekday.to_string(),
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    #[test]
    fn legacy_time_arguments_infer_actions() {
        assert_eq!(effective_recall_action(&json!({})), "today");
        assert_eq!(effective_recall_action(&json!({"hours": 6})), "recent");
        assert_eq!(effective_recall_action(&json!({"date": "2026-10-29"})), "date");
        assert_eq!(effective_recall_action(&json!({"time_of_day": "evening"})), "date");
        assert_eq!(effective_recall_action(&json!({"action": "today"})), "today");
    }
}
