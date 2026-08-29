//! 记忆工具 - save_memory, search_memory, clear_memory
//!
//! 通过 MemoryManager 操作统一记忆后端，与核心记忆系统共享数据。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::diary;
use crate::memory::{MemoryManager, MemoryType, RetrievalStrategy};
use crate::tools::services::MemoryService;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult,
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
        "Search the long-term memory store for memories containing the specified keywords. Results are sorted by importance.\
         Use when you need to recall what was said/done in the past. Do NOT use to store new info (that's save_memory), and do not search when the answer is already in recent conversation."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "在长期记忆库中搜索包含指定关键词的记忆，结果按重要性排序。当你需要回忆过去说过/做过的事时使用。不要用它来保存新信息（那是 save_memory 的职责）；如果答案已经在最近的对话里，也不必搜索。",
            "ja" => "長期記憶ストアから指定キーワードを含む記憶を検索し、重要度順でソートする。過去に話した・行ったことを思い出すときに使用。新しい情報の保存には使わないこと（それは save_memory の役割）。答えが最近の会話にすでにある場合は検索不要。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search keywords"
                },
                "category": {
                    "type": "string",
                    "description": "Filter by category (optional)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results, default 10",
                    "minimum": 1
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
                        "description": "搜索关键词"
                    },
                    "category": {
                        "type": "string",
                        "description": "按类别过滤（可选）"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "返回结果的最大数量，默认 10",
                        "minimum": 1
                    }
                },
                "required": ["query"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "検索キーワード"
                    },
                    "category": {
                        "type": "string",
                        "description": "カテゴリで絞り込み（任意）"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "結果の最大数、デフォルト 10",
                        "minimum": 1
                    }
                },
                "required": ["query"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.is_empty() => q.to_string(),
            _ => return ValidationResult::failure("query 是必填项且不能为空", 2),
        };
        let _ = query;
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
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let category = args.get("category").and_then(|v| v.as_str());
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        if let Some(mgr) = get_manager_for_context(context) {
            // Fetch extra results so category filtering doesn't starve the result set
            let fetch_limit = if category.is_some() { limit * 3 + 20 } else { limit };
            match mgr
                .search_memories(query, RetrievalStrategy::Auto, fetch_limit)
                .await
            {
                Ok(items) => {
                    let mut filtered = items;
                    if let Some(cat) = category {
                        if !cat.is_empty() {
                            let cat_type = parse_memory_type(cat);
                            filtered.retain(|item| {
                                item.tags.iter().any(|t| t == cat)
                                    || parse_memory_type(&item.granularity) == cat_type
                            });
                        }
                    }

                    // Truncate to the requested limit after filtering
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
                                "timestamp": item.timestamp,
                            })
                        })
                        .collect();

                    return ToolResult::standard_success(
                        &format!("找到 {} 条记忆", total),
                        Some(json!({
                            "query": query,
                            "category": category,
                            "results": serialized,
                            "total": total,
                        })),
                    );
                }
                Err(e) => {
                    return ToolResult::standard_error(
                        &format!("搜索记忆失败: {e}"),
                        None,
                        None,
                    );
                }
            }
        }

        ToolResult::standard_error("记忆系统未初始化", None, None)
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
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
        "Get recent interaction records between the user and Vivian within a specified time range."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "获取指定时间范围内用户与 Vivian 的最近互动记录。",
            "ja" => "指定された時間範囲内のユーザーと Vivian の最近のやり取り記録を取得する。",
            _ => self.description(),
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

    fn search_hint(&self) -> &str {
        "recall session summaries by date or time of day"
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
