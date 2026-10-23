//! 会话历史检索工具（`session_*` 五件套）。
//!
//! 用途：跨会话回忆「上次那个改动是怎么做的」「这个问题以前踩过吗」。
//! 数据源就是落盘的 `coding_sessions/*.json`——不另建索引，因为会话文件本身不大，
//! 而额外索引会引入"索引过期"这类更难查的问题。
//!
//! 口径说明：检索结果是**线索**，不是事实。命中一条历史不代表当时的结论是对的，
//! 引用时必须带上会话 id 与消息序号，让上层能回原文核对。

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};
use crate::utils::path::get_user_data_dir;

/// 单次扫描的会话文件上限（按修改时间取最近 N 个）。
const MAX_SESSIONS_SCAN: usize = 200;

fn sessions_dir() -> PathBuf {
    get_user_data_dir().join("coding_sessions")
}

fn recent_session_files(limit: usize) -> Vec<PathBuf> {
    let dir=sessions_dir();
    let heads=match crate::brain::coding_session_store::load_heads::<Value>(&dir) {
        Ok(Some(heads))=>heads,
        Ok(None)=>return Vec::new(),
        Err(error)=>{tracing::warn!("Session catalog unavailable: {error}");return Vec::new();}
    };
    let mut files: Vec<(std::time::SystemTime,PathBuf)>=heads.into_iter()
        .filter_map(|(id,_)| {
            let path=crate::brain::coding_session_store::body_path(&dir,&id);
            let modified=std::fs::metadata(&path).and_then(|m|m.modified()).ok()?;
            Some((modified,path))
        }).collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.into_iter().take(limit).map(|(_, p)| p).collect()
}

fn load_session(path: &PathBuf) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn session_id_of(doc: &Value) -> String {
    doc.get("session_id")
        .and_then(|v| v.as_str())
        .or_else(|| doc.get("id").and_then(|v| v.as_str()))
        .unwrap_or("unknown")
        .to_string()
}

fn messages_of(doc: &Value) -> Vec<Value> {
    doc.get("messages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn msg_role(m: &Value) -> &str {
    m.get("role").and_then(|v| v.as_str()).unwrap_or("")
}

fn msg_content(m: &Value) -> String {
    m.get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn msg_tool(m: &Value) -> Option<&str> {
    m.get("tool_name").and_then(|v| v.as_str())
}

/// 截取命中位置附近的一段文本。
fn snippet(text: &str, needle_lower: &str, radius: usize) -> String {
    let lower = text.to_lowercase();
    let Some(pos) = lower.find(needle_lower) else {
        return text.chars().take(radius * 2).collect();
    };
    let mut start = pos.saturating_sub(radius).min(text.len());
    let mut end = (pos + needle_lower.len() + radius).min(text.len());
    while !text.is_char_boundary(start) { start -= 1; }
    while !text.is_char_boundary(end) { end -= 1; }
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(&text[start..end]);
    if end < text.len() {
        out.push('…');
    }
    out.replace('\n', " ")
}

// ============================================================================
// session_search
// ============================================================================

pub struct SessionSearchTool;

impl SessionSearchTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for SessionSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SessionSearchTool {
    fn name(&self) -> &str {
        "session_search"
    }

    fn description(&self) -> &str {
        "Search past coding sessions (other conversations, persisted on disk) for a keyword, and return matching \
         messages with session id + message index. Use it when the user references earlier work you cannot see, \
         or before re-solving a problem the project may have already solved.\n\
         Results are leads, not facts: verify in the original session before relying on them."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "在**过去的编程会话**（落盘的其他对话）里搜关键词，返回命中消息 + 会话 id + 消息序号。\
                     用户提到你看不到的旧工作时、或者怀疑这个问题以前解过时用它。\n\
                     结果是**线索不是事实**：要依赖某条结论，先回原会话核对。",
            "ja" => "過去のコーディングセッション（ディスク上の他对话）をキーワード検索し、命中メッセージとセッション ID・メッセージ番号を返す。\
                     見えない過去の作業に触れられたとき、既に解決済みか疑わしいときに使う。\n\
                     結果は**手がかりであり事実ではない**。依存する前に元セッションで確認する。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "关键词（不区分大小写，子串匹配）" },
                "limit": { "type": "integer", "description": "最多返回多少条命中（默认 20，上限 100）", "minimum": 1 },
                "max_sessions": { "type": "integer", "description": "每页最多扫描多少个会话（默认 40，上限 200）；可用返回游标继续查更早历史", "minimum": 1 },
                "session_offset": { "type": "integer", "minimum": 0, "description": "返回游标中的 session_offset" },
                "message_offset": { "type": "integer", "minimum": 0, "description": "返回游标中的 message_offset" },
                "all_workspaces": { "type": "boolean", "description": "显式跨工作区搜索当前角色的会话；默认只查当前工作区" }
            },
            "required": ["query"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input.get("query").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
            return ValidationResult::failure("query 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let needle = query.to_lowercase();
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20).clamp(1, 100) as usize;
        let max_sessions = args
            .get("max_sessions")
            .and_then(|v| v.as_u64())
            .unwrap_or(40)
            .clamp(1, MAX_SESSIONS_SCAN as u64) as usize;

        let session_offset = args.get("session_offset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let message_offset = args.get("message_offset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let all_workspaces = args.get("all_workspaces").and_then(Value::as_bool).unwrap_or(false);
        let files = recent_session_files(usize::MAX);
        let mut hits: Vec<Value> = Vec::new();
        let mut scanned = 0usize;
        let mut next_cursor = None;
        'sessions: for (file_index, path) in files.iter().enumerate().skip(session_offset).take(max_sessions) {
            scanned += 1;
            next_cursor = (file_index + 1 < files.len()).then(|| json!({"session_offset":file_index + 1,"message_offset":0}));
            let Some(doc) = load_session(path) else { continue };
            let sid = session_id_of(&doc);
            if sid == ctx.session_id || doc.get("char_id").and_then(Value::as_str) != Some(ctx.char_id.as_str()) { continue; }
            if !all_workspaces && !ctx.working_directory.trim().is_empty() {
                let wd = doc.get("working_directory").and_then(Value::as_str).unwrap_or("");
                let same = std::path::Path::new(wd).canonicalize().ok().zip(std::path::Path::new(&ctx.working_directory).canonicalize().ok())
                    .is_some_and(|(a,b)| a == b) || wd == ctx.working_directory;
                if !same { continue; }
            }
            let messages = messages_of(&doc);
            for (idx, msg) in messages.iter().enumerate().skip(if file_index == session_offset { message_offset } else { 0 }) {
                let content = msg.get("content").and_then(Value::as_str).unwrap_or("");
                if content.to_lowercase().contains(&needle) {
                    hits.push(json!({"session_id":sid,"message_index":idx,"role":msg_role(msg),"tool":msg_tool(msg),"snippet":snippet(content,&needle,120)}));
                    if hits.len() >= limit {
                        next_cursor = if idx + 1 < messages.len() { Some(json!({"session_offset":file_index,"message_offset":idx+1})) }
                            else { (file_index+1 < files.len()).then(|| json!({"session_offset":file_index+1,"message_offset":0})) };
                        break 'sessions;
                    }
                }
            }
        }
        let mut text = format!("本页命中 {} 条（扫描 {scanned} 个会话）；结果仅为线索，需核对原文。\n", hits.len());
        for h in &hits { text.push_str(&format!("- {} #{} {}\n", h["session_id"], h["message_index"], h["snippet"])); }
        if let Some(cursor) = &next_cursor { text.push_str(&format!("尚有更早历史，请把游标参数传入下次 session_search：{cursor}\n")); }
        text.push_str("用 session_event_read 读取命中会话原文。游标基于当前历史排序，新会话写入后可从头重新检索。\n");
        ToolResult::standard_success(&text, Some(json!({"query":query,"hits":hits,"scanned":scanned,"next_cursor":next_cursor})))
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
    fn search_hint(&self) -> &str {
        "session history search past conversations 历史会话 以前 上次"
    }
}

// ============================================================================
// session_trace
// ============================================================================

pub struct SessionTraceTool;

impl SessionTraceTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for SessionTraceTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SessionTraceTool {
    fn name(&self) -> &str {
        "session_trace"
    }

    fn description(&self) -> &str {
        "Show the outline of one past session: title, goal, turn count, and a compact list of the tools it called \
         in order. Use it to judge whether a session is worth reading in full — then use session_event_read."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "给一个历史会话画骨架：标题、目标、消息条数，以及按顺序调过哪些工具。\
                     用来判断「这个会话值不值得通读」，值得再用 session_event_read 读原文。",
            "ja" => "過去セッションの骨格を示す：タイトル・目標・メッセージ数・呼ばれたツールの順序。\
                     通読に値するか判断するのに使い、必要なら session_event_read で本文を読む。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "session_id": { "type": "string", "description": "会话 id" } },
            "required": ["session_id"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input.get("session_id").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
            return ValidationResult::failure("session_id 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let sid = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("").trim();
        let Some(doc) = find_session(sid, &ctx.session_id, &ctx.char_id) else {
            return ToolResult::standard_error(
                &format!("找不到会话 {sid}。用 session_search 拿到正确的 id。"),
                Some("NotFound"),
                None,
            );
        };
        let messages = messages_of(&doc);
        let title = doc
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.chars().take(80).collect::<String>())
            .unwrap_or_default();
        let goal = doc
            .get("goal")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(120)
            .collect::<String>();
        let mut chains: Vec<String> = Vec::new();
        for m in &messages {
            if let Some(tool) = msg_tool(m) {
                chains.push(format!("{tool}{}", if m.get("tool_success").and_then(|v| v.as_bool()) == Some(false) { "✗" } else { "" }));
            }
        }
        let text = format!(
            "会话 {sid}\n标题：{title}\n目标：{goal}\n消息数：{}\n工具链：{}\n",
            messages.len(),
            if chains.is_empty() { "（无）".to_string() } else { chains.join(" → ") }
        );
        ToolResult::standard_success(
            &text,
            Some(json!({
                "session_id": sid, "title": title, "goal": goal,
                "message_count": messages.len(), "tool_chain": chains,
            })),
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
    fn search_hint(&self) -> &str {
        "session trace outline 历史会话大纲 概览"
    }
}

fn find_session(session_id: &str, current: &str, char_id: &str) -> Option<Value> {
    if session_id == current { return None; }
    let dir = sessions_dir();
    let heads = crate::brain::coding_session_store::load_heads::<Value>(&dir).ok().flatten()?;
    if !heads.iter().any(|(id, _)| id == session_id) { return None; }
    let doc = load_session(&crate::brain::coding_session_store::body_path(&dir, session_id))?;
    (session_id_of(&doc) == session_id && doc.get("char_id").and_then(Value::as_str) == Some(char_id)).then_some(doc)
}

// ============================================================================
// session_event_read / session_event_search / session_event_trace
// ============================================================================

pub struct SessionEventReadTool;

impl SessionEventReadTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for SessionEventReadTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SessionEventReadTool {
    fn name(&self) -> &str {
        "session_event_read"
    }

    fn description(&self) -> &str {
        "Read a slice of a past session's message stream by index (use the offsets from session_search / \
         session_event_search). Returns role, tool name and content, truncated per message. Use source=audit and after_seq to read the append-only execution journal instead; request_id links Workflow snapshots."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "按序号读取历史会话的一段消息流（序号来自 session_search / session_event_search）。\
                     返回角色、工具名与内容，单条会截断。source=audit 可读取追加式执行日志，用 after_seq 续读，request_id 对应 Workflow 请求快照。",
            "ja" => "過去セッションのメッセージ列を番号で読む（番号は session_search / session_event_search から）。\
                     ロール・ツール名・内容を返し、1 件ごとに切り詰める。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "session_id": { "type": "string", "description": "会话 id" },
                "source": { "type":"string", "enum":["messages","audit"], "description":"Default messages preserves existing message offsets; audit reads durable execution events" },
                "after_seq": { "type":"integer", "minimum":0, "description":"Audit event sequence to continue after, default 0" },
                "snapshot_bytes": { "type":"integer", "minimum":0, "description":"For audit pagination, pass the value from the first page to freeze its boundary" },
                "offset": { "type": "integer", "description": "起始序号，默认 0", "minimum": 0 },
                "limit": { "type": "integer", "description": "读多少条，默认 8，上限 30", "minimum": 1 }
            },
            "required": ["session_id"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input.get("session_id").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
            return ValidationResult::failure("session_id 不能为空", 2);
        }
        if input.get("source").is_some_and(|v| !matches!(v.as_str(), Some("messages" | "audit"))) {
            return ValidationResult::failure("source 必须为 messages 或 audit", 2);
        }
        if input.get("after_seq").is_some_and(|v| v.as_u64().is_none()) {
            return ValidationResult::failure("after_seq 必须为非负整数", 2);
        }
        if input.get("snapshot_bytes").is_some_and(|v| v.as_u64().is_none()) {
            return ValidationResult::failure("snapshot_bytes 必须为非负整数", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let sid = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("").trim();
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(8).clamp(1, 30) as usize;
        if sid != ctx.session_id && find_session(sid, &ctx.session_id, &ctx.char_id).is_none() {
            return ToolResult::standard_error("无法访问该角色之外或不存在的会话", Some("NotFound"), None);
        }
        if args["source"].as_str() == Some("audit") {
            return match crate::brain::coding_event_log::read_page_snapshot(&get_user_data_dir().join("coding_events"), sid,
                args["after_seq"].as_u64().unwrap_or(0), limit, args["snapshot_bytes"].as_u64()) {
                Ok(page) => ToolResult::success(page),
                Err(error) => ToolResult::standard_error("无法读取会话审计日志；旧会话可能尚无事件记录", Some("AuditUnavailable"), Some(json!({"detail":error}))),
            };
        }
        let Some(doc) = find_session(sid, &ctx.session_id, &ctx.char_id) else {
            return ToolResult::standard_error(&format!("找不到会话 {sid}。"), Some("NotFound"), None);
        };
        let messages = messages_of(&doc);
        if messages.is_empty() {
            return ToolResult::standard_success("该会话没有消息。", Some(json!({"events": []})));
        }
        let mut text = format!("会话 {sid} 第 {offset}..{} 条（共 {} 条）：\n", offset + limit, messages.len());
        let mut events = Vec::new();
        for (idx, m) in messages.iter().enumerate().skip(offset).take(limit) {
            let role = msg_role(m);
            let tool = msg_tool(m);
            let content: String = msg_content(m).chars().take(1200).collect();
            text.push_str(&format!(
                "\n#{} [{role}]{} \n{content}\n",
                idx,
                tool.map(|t| format!(" tool={t}")).unwrap_or_default()
            ));
            events.push(json!({"index": idx, "role": role, "tool": tool, "content": content}));
        }
        let next = offset + limit;
        if next < messages.len() {
            text.push_str(&format!("\n（还有 {} 条，用 offset={} 继续）", messages.len() - next, next));
        }
        ToolResult::standard_success(&text, Some(json!({"events": events, "next_offset": next})))
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
    fn search_hint(&self) -> &str {
        "session event read 读历史原文 翻记录"
    }
}

pub struct SessionEventSearchTool;

impl SessionEventSearchTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for SessionEventSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SessionEventSearchTool {
    fn name(&self) -> &str {
        "session_event_search"
    }

    fn description(&self) -> &str {
        "Search inside ONE past session (narrower and faster than session_search when you already know which \
         session). Returns message indices you can feed to session_event_read."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "在**某一个**历史会话里搜（已经知道是哪个会话时，比 session_search 更窄更快）。\
                     返回可喂给 session_event_read 的消息序号。",
            "ja" => "**特定の 1 セッション**内を検索（対象が分かっているなら session_search より速い）。\
                     session_event_read に渡せるメッセージ番号を返す。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "session_id": { "type": "string", "description": "会话 id" },
                "query": { "type": "string", "description": "关键词" },
                "limit": { "type": "integer", "description": "最多返回多少条（默认 10，上限 50）", "minimum": 1 }
            },
            "required": ["session_id", "query"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        for k in ["session_id", "query"] {
            if input.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
                return ValidationResult::failure(&format!("{k} 不能为空"), 2);
            }
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let sid = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("").trim();
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10).clamp(1, 50) as usize;
        let Some(doc) = find_session(sid, &ctx.session_id, &ctx.char_id) else {
            return ToolResult::standard_error(&format!("找不到会话 {sid}。"), Some("NotFound"), None);
        };
        let needle = query.to_lowercase();
        let mut hits = Vec::new();
        for (idx, m) in messages_of(&doc).iter().enumerate() {
            if hits.len() >= limit {
                break;
            }
            let content = msg_content(m);
            if content.to_lowercase().contains(&needle) {
                hits.push(json!({
                    "message_index": idx,
                    "role": msg_role(m),
                    "tool": msg_tool(m),
                    "snippet": snippet(&content, &needle, 120),
                }));
            }
        }
        if hits.is_empty() {
            return ToolResult::standard_success(
                &format!("会话 {sid} 里没有命中「{query}」。"),
                Some(json!({"hits": []})),
            );
        }
        let mut text = format!("会话 {sid} 命中 {} 条：\n", hits.len());
        for h in &hits {
            text.push_str(&format!(
                "- #{} {}\n",
                h["message_index"],
                h["snippet"].as_str().unwrap_or("")
            ));
        }
        ToolResult::standard_success(&text, Some(json!({"hits": hits})))
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
    fn search_hint(&self) -> &str {
        "session event search 会话内搜索"
    }
}

pub struct SessionEventTraceTool;

impl SessionEventTraceTool {
    pub fn new() -> Self {
        Self
    }
}
impl Default for SessionEventTraceTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SessionEventTraceTool {
    fn name(&self) -> &str {
        "session_event_trace"
    }

    fn description(&self) -> &str {
        "Show only the tool-call chain of a past session (name + success/failure + a one-line argument summary). \
         This is the fastest way to see HOW something was done without reading the whole transcript."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "只看某历史会话的**工具调用链**（名称 + 成败 + 一行参数摘要）。\
                     想快速看「当时是怎么做的」又不想通读全文时，用这个最快。",
            "ja" => "過去セッションの**ツール呼び出し連鎖**だけを見る（名前・成否・引数の一行要約）。\
                     全文を読まずに「どうやったか」を掴むのに一番速い。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "session_id": { "type": "string", "description": "会话 id" },
                "limit": { "type": "integer", "description": "最多多少条（默认 60，上限 200）", "minimum": 1 }
            },
            "required": ["session_id"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input.get("session_id").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
            return ValidationResult::failure("session_id 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let sid = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("").trim();
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(60).clamp(1, 200) as usize;
        let Some(doc) = find_session(sid, &ctx.session_id, &ctx.char_id) else {
            return ToolResult::standard_error(&format!("找不到会话 {sid}。"), Some("NotFound"), None);
        };
        let mut rows = Vec::new();
        let mut text = String::new();
        for (idx, m) in messages_of(&doc).iter().enumerate() {
            if rows.len() >= limit {
                break;
            }
            let Some(tool) = msg_tool(m) else { continue };
            let ok = m.get("tool_success").and_then(|v| v.as_bool()).unwrap_or(true);
            let args_text = m
                .get("tool_arguments")
                .map(|v| v.to_string().chars().take(100).collect::<String>().replace('\n', " "))
                .unwrap_or_default();
            text.push_str(&format!("#{idx} {tool}{} {args_text}\n", if ok { "" } else { " ✗" }));
            rows.push(json!({"index": idx, "tool": tool, "success": ok, "arguments": args_text}));
        }
        if rows.is_empty() {
            return ToolResult::standard_success("该会话没有工具调用记录。", Some(json!({"chain": []})));
        }
        ToolResult::standard_success(
            &format!("会话 {sid} 的工具链（{} 步）：\n{text}", rows.len()),
            Some(json!({"chain": rows})),
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
    fn search_hint(&self) -> &str {
        "session event trace tool chain 调用链 当时怎么做的"
    }
}
