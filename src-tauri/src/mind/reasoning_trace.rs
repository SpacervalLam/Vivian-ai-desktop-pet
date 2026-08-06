//! ReasoningTrace —— 全链路推理轨迹基础设施。
//!
//! 为「心智观察器」前端提供认知调试数据：
//! - [`ReasoningTrace`]：一次 LLM 调用的完整推理轨迹（含多步骤）
//! - [`PromptBreakdown`]：Prompt 组装分区分解
//! - [`SessionView`]：会话列表视图
//!
//! 存储策略：按角色 ID 索引的环形缓冲（每角色最近 50 条）。使用全局单例
//! [`TRACE_STORE`]（与 `CONVERSATION_MANAGER` 同模式），让 `BrainChatChain`
//! 无需 `AppHandle` 即可写入，同时 `AppState` 持有同一 `Arc` 供 Tauri 命令读取。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use once_cell::sync::Lazy;
use parking_lot::RwLock;

/// 单个推理步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningStep {
    /// 步骤名称（Observation/MindUpdate/Retrieval/Assembly/LLM/Reply/Intent/Tool/MemoryUpdate 等）
    pub name: String,
    /// 输入摘要（≤200 字符）
    pub input_summary: String,
    /// 输出摘要（≤200 字符）
    pub output_summary: String,
    /// 耗时（毫秒）
    pub duration_ms: u32,
    /// 步骤详情（步骤特定的 JSON 数据）
    pub details: serde_json::Value,
    /// 是否成功
    pub success: bool,
    /// 错误信息（失败时）
    pub error: Option<String>,
}

/// 一次完整的 LLM 调用推理轨迹
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningTrace {
    /// 唯一 ID
    pub id: String,
    /// 角色 ID
    pub character_id: String,
    /// 会话 ID（如果有）
    pub session_id: Option<String>,
    /// 开始时间（Unix 时间戳，秒）
    pub started_at: f64,
    /// 结束时间
    pub ended_at: Option<f64>,
    /// 用户输入
    pub user_input: String,
    /// 最终回复
    pub final_reply: Option<String>,
    /// 推理步骤列表（按时间顺序）
    pub steps: Vec<ReasoningStep>,
}

impl ReasoningTrace {
    pub fn new(character_id: &str, user_input: &str) -> Self {
        Self {
            id: format!("trace_{}_{}", character_id, Utc::now().timestamp_millis()),
            character_id: character_id.to_string(),
            session_id: None,
            started_at: Utc::now().timestamp_millis() as f64 / 1000.0,
            ended_at: None,
            user_input: user_input.to_string(),
            final_reply: None,
            steps: Vec::new(),
        }
    }

    pub fn add_step(&mut self, step: ReasoningStep) {
        self.steps.push(step);
    }

    pub fn finish(&mut self, reply: Option<String>) {
        self.ended_at = Some(Utc::now().timestamp_millis() as f64 / 1000.0);
        self.final_reply = reply;
    }
}

/// Prompt 分区信息（用于 Context Pipeline 页）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSection {
    /// 分区名称（Identity/Current Mind/World Snapshot/Observations/Belief/Relevant Episode/Recent Conversation/Task）
    pub name: String,
    /// 内容预览（前 300 字符）
    pub preview: String,
    /// 完整内容
    pub full_content: String,
    /// 字符数
    pub char_count: usize,
    /// Section 唯一 ID（snake_case，如 "character", "mind", "tools"）
    #[serde(default)]
    pub section_id: String,
    /// 所属层级（framework/character/advanced/mind/world/observation/episode/profile/tail）
    #[serde(default)]
    pub layer: String,
    /// 估算 token 数
    #[serde(default)]
    pub token_estimate: usize,
    /// 是否为条件注入（optional=true 表示非每轮都有内容）
    #[serde(default)]
    pub optional: bool,
    /// 本次是否实际注入内容
    #[serde(default)]
    pub present: bool,
}

/// 场景模式预览数据（用于前端模式切换展示）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneModePreview {
    pub mode: String,
    pub description: String,
    pub instructions: Vec<String>,
}

/// 非 messages 数组的 API 参数（发送给 LLM 但在 prompt 文本之外的内容）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiParamInfo {
    /// 参数类型标识: "native_tools" | "response_format" | "instructions"
    pub param_type: String,
    /// 显示名称
    pub label: String,
    /// 详细内容（JSON 字符串或文本描述）
    pub content: String,
    /// 是否本轮实际发送
    pub present: bool,
}

/// Prompt 组装分解（用于 get_last_prompt_breakdown 命令）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptBreakdown {
    pub character_id: String,
    pub sections: Vec<PromptSection>,
    pub total_chars: usize,
    /// 总估算 token 数
    #[serde(default)]
    pub total_tokens: usize,
    pub timestamp: f64,
    /// 所有可用场景模式（模板预览时填充，供前端切换视图展示）
    #[serde(default)]
    pub scene_modes: Vec<SceneModePreview>,
    /// 非 messages 数组的 API 参数（native FC tools、response_format、instructions）
    #[serde(default)]
    pub api_params: Vec<ApiParamInfo>,
}

/// 会话视图（用于 get_sessions 命令）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionView {
    pub id: String,
    pub participants: Vec<String>,
    pub started_at: f64,
    pub ended_at: Option<f64>,
    pub rounds: usize,
    pub energy: f32,
    pub novelty: f32,
    pub status: String, // Created/Active/Cooling/Closed
    pub close_reason: Option<String>,
    pub last_active_at: f64,
}

/// 按角色索引的 trace 环形缓冲（每角色保留最近 N 条）
pub struct TraceStore {
    /// character_id → traces（按时间倒序，最多 50 条）
    traces: HashMap<String, Vec<ReasoningTrace>>,
    /// character_id → 最近一条 PromptBreakdown
    last_prompt: HashMap<String, PromptBreakdown>,
    max_per_character: usize,
}

impl TraceStore {
    pub fn new() -> Self {
        Self {
            traces: HashMap::new(),
            last_prompt: HashMap::new(),
            max_per_character: 50,
        }
    }

    pub fn add_trace(&mut self, trace: ReasoningTrace) {
        let char_id = trace.character_id.clone();
        let list = self.traces.entry(char_id).or_default();
        list.insert(0, trace); // 最新的在前面
        if list.len() > self.max_per_character {
            list.truncate(self.max_per_character);
        }
    }

    pub fn get_recent_traces(&self, char_id: &str, limit: usize) -> Vec<ReasoningTrace> {
        self.traces
            .get(char_id)
            .map(|list| list.iter().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    pub fn get_last_trace(&self, char_id: &str) -> Option<&ReasoningTrace> {
        self.traces.get(char_id).and_then(|list| list.first())
    }

    pub fn set_last_prompt(&mut self, breakdown: PromptBreakdown) {
        self.last_prompt
            .insert(breakdown.character_id.clone(), breakdown);
    }

    pub fn get_last_prompt(&self, char_id: &str) -> Option<&PromptBreakdown> {
        self.last_prompt.get(char_id)
    }
}

impl Default for TraceStore {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedTraceStore = Arc<RwLock<TraceStore>>;

/// 全局 TraceStore 单例
///
/// `AppState.reasoning_traces` 持有此 `Arc` 的 clone，`BrainChatChain::ainvoke`
/// 直接通过此单例写入，避免在对话链中传递 `AppHandle`。
pub static TRACE_STORE: Lazy<SharedTraceStore> =
    Lazy::new(|| Arc::new(RwLock::new(TraceStore::new())));

static APP_HANDLE: Lazy<RwLock<Option<tauri::AppHandle>>> = Lazy::new(|| RwLock::new(None));

pub fn set_app_handle(handle: tauri::AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

pub fn emit_trace_added(character_id: &str) {
    use tauri::Emitter;
    if let Some(handle) = APP_HANDLE.read().as_ref() {
        let _ = handle.emit(
            "reasoning:trace_added",
            serde_json::json!({ "character_id": character_id }),
        );
    }
}

/// 截断字符串到指定字符数（用于 summary）
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{}…", truncated)
    }
}
