use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{VivianError, VivianResult};
use crate::network::http_client::get_global_client;
use crate::providers::reasoning::ReasoningPreference;
use crate::resilience::{register_circuit_breaker, CircuitBreaker};
use crate::types::response::ChatMessage;

const CACHE_TTL: Duration = Duration::from_secs(300);
const CACHE_MAX_ENTRIES: usize = 256;
const CB_FAILURE_THRESHOLD: u32 = 5;
const CB_FAILURE_RATE: f64 = 0.5;
const CB_RESET_TIMEOUT: Duration = Duration::from_secs(30);

/// 结构化工具调用（来自 LLM 的原生 function calling 响应）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredToolCall {
    /// 原生 tool_call_id（用于多轮工具调用上下文关联）
    pub id: String,
    /// 工具名
    pub name: String,
    /// 工具参数（JSON）
    pub arguments: serde_json::Value,
}

/// LLM 对话响应的结构化表示
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    /// 文本部分
    pub content: String,
    /// 结构化工具调用列表（若模型支持原生 function calling）
    pub tool_calls: Vec<StructuredToolCall>,
    /// 结束原因（stop / tool_calls / length / content_filter 等）
    pub finish_reason: Option<String>,
    /// 推理/思维链内容
    ///
    /// 由 provider 从响应中提取，字段来源因服务商而异：
    /// - OpenAI 兼容端点：`message.reasoning_content`（DeepSeek / Qwen / GLM / 火山）
    ///   或 `message.reasoning_details[].summary`（部分 o 系列兼容端点）
    /// - Anthropic：`content[].type=="thinking"` 块的 `thinking` 字段
    ///
    /// 不入可见输出流，仅用于多轮回传与持久化。
    #[serde(default)]
    pub reasoning: Option<String>,
    /// 原始响应（兜底，用于调试或额外字段提取）
    pub raw: serde_json::Value,
}

impl ChatResponse {
    /// 从纯文本构造（无工具调用）
    pub fn from_text(text: String) -> Self {
        Self {
            content: text,
            tool_calls: Vec::new(),
            finish_reason: None,
            reasoning: None,
            raw: serde_json::Value::Null,
        }
    }

    /// 是否包含工具调用
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

/// 工具定义（用于 bind_tools）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// 统一 LLM 请求结构（Brain → ModelRouter 的标准入口）
///
/// 把原本散落的 `task_type` / `messages` / `tools` / `stream` / `enable_search`
/// 等参数打包成单一结构,让 Brain 无需关心底层是 Responses API / Chat Completions
/// / Anthropic Messages / Gemini GenerateContent 中的哪一种。
///
/// 设计原则:
/// - Brain 永远只构造 `LLMRequest`,不直接调用 provider 特定方法
/// - ModelRouter 根据 `task_type` 路由到对应 provider,内部转调 BaseProvider 方法
/// - `tools` 为空表示走文本路径;非空且 provider 支持时走原生 function calling
/// - temperature / max_tokens 运行时覆盖优先使用请求级字段,无则沿用 ModelRouter setter
///   (保持现有 emotion→temperature / focus_boost 机制)
///
/// 不包含 `previous_response_id`:Vivian 已有完整记忆架构(MemoryManager +
/// TimeStampedMemory + ConsolidationPipeline),Brain 每轮传完整 messages,
/// 不依赖服务端 Conversation State,避免双 Context 问题。Responses API 始终当 Stateless 用。
#[derive(Debug, Clone)]
pub struct LLMRequest {
    /// 任务类型(路由 key):chat / reasoning / reflection / consolidation /
    /// inner_monologue / vision_describe 等
    pub task_type: String,
    /// 对话消息数组(含 system / user / assistant / tool 角色)
    pub messages: Vec<ChatMessage>,
    /// 工具定义列表(空 = 不启用原生 function calling,走文本路径)
    pub tools: Vec<ToolDefinition>,
    /// 是否流式响应
    pub stream: bool,
    /// 是否启用联网搜索(部分 provider 支持)
    pub enable_search: bool,
    /// 请求级 temperature 覆盖(None 表示沿用 ModelRouter 默认值)
    pub temperature_override: Option<f64>,
    /// 请求级 max_tokens 覆盖(None 表示沿用 ModelRouter 默认值)
    pub max_tokens_override: Option<u32>,
    /// JSON Schema 结构化输出约束(None 表示不启用)
    pub json_schema: Option<serde_json::Value>,
    /// 推理/思维链偏好(模式 + 档位，按模型能力映射为各家 wire 字段)
    pub reasoning: ReasoningPreference,
}

impl LLMRequest {
    /// 构造基础请求(无工具、非流式、不联网、默认参数)
    pub fn new(task_type: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            task_type: task_type.into(),
            messages,
            tools: Vec::new(),
            stream: false,
            enable_search: false,
            temperature_override: None,
            max_tokens_override: None,
            json_schema: None,
            reasoning: ReasoningPreference::AUTO,
        }
    }

    /// 设置工具定义(启用原生 function calling 路径)
    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = tools;
        self
    }

    /// 启用流式响应
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }

    /// 启用联网搜索
    pub fn with_search(mut self, enable: bool) -> Self {
        self.enable_search = enable;
        self
    }

    /// 设置请求级 temperature
    pub fn with_temperature(mut self, temp: f64) -> Self {
        self.temperature_override = Some(temp);
        self
    }

    /// 设置请求级 max_tokens
    pub fn with_max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens_override = Some(tokens);
        self
    }

    /// 设置 JSON Schema 约束
    pub fn with_json_schema(mut self, schema: serde_json::Value) -> Self {
        self.json_schema = Some(schema);
        self
    }

    /// 启用推理/思维链（true = On 无指定档位，false = Auto 不干预）
    pub fn with_reasoning(mut self, enable: bool) -> Self {
        self.reasoning = if enable {
            ReasoningPreference::on(None)
        } else {
            ReasoningPreference::AUTO
        };
        self
    }

    /// 设置推理偏好（模式 + 档位）
    pub fn with_reasoning_pref(mut self, pref: ReasoningPreference) -> Self {
        self.reasoning = pref;
        self
    }

    /// 是否请求原生 function calling 路径(tools 非空)
    pub fn wants_tools(&self) -> bool {
        !self.tools.is_empty()
    }

    /// 是否启用结构化 JSON 输出
    pub fn wants_json(&self) -> bool {
        self.json_schema.is_some()
    }
}

#[async_trait]
pub trait BaseProvider: Send + Sync {
    async fn call_chat(&self, messages: Vec<ChatMessage>) -> VivianResult<String>;
    async fn call_stream_chat(
        &self,
        messages: Vec<ChatMessage>,
        json_schema: Option<Value>,
    ) -> VivianResult<tokio::sync::mpsc::Receiver<String>>;
    fn get_model(&self) -> &str;
    fn get_circuit_breaker_stats(&self) -> serde_json::Value;

    /// 设置该 provider 是否启用联网搜索（运行时可切换）
    ///
    /// 默认空实现，由具体 provider 覆盖。使用 `&self` + 内部可变性（AtomicBool）。
    fn set_enable_search(&self, _enable: bool) {}

    /// 设置 max_tokens 运行时覆盖（0 表示恢复默认）。
    ///
    /// 默认空实现，由持有 `ProviderBase` 的具体 provider 覆盖。
    /// 凝神模式激活时由生成层调用，给混合推理模型留出思考 token 余量。
    fn set_max_tokens_override(&self, _tokens: u32) {}

    /// 设置 temperature 运行时覆盖。
    ///
    /// 默认空实现，由持有 `ProviderBase` 的具体 provider 覆盖。
    /// 传入 None 清除覆盖（恢复配置默认值）；Some(t) 设置覆盖温度。
    /// 由 emotion→temperature 映射在每轮对话前调用。
    fn set_temperature_override(&self, _temp: Option<f64>) {}

    /// 设置是否在请求体中省略 temperature 字段（工作智能体模型用）。
    ///
    /// 默认空实现，由持有 `ProviderBase` 的具体 provider 覆盖。
    /// ModelRouter 在构建工作智能体覆盖 provider 时设置为 true，
    /// provider 构造请求体后按厂商路径移除 temperature（服务端默认）。
    fn set_omit_temperature(&self, _omit: bool) {}

    /// 设置推理偏好运行时覆盖（请求级）。
    ///
    /// 默认空实现，由持有 `ProviderBase` 的具体 provider 覆盖。
    /// ModelRouter 在每次请求前按 LLMRequest.reasoning 设置、请求后恢复为
    /// None（不干预），provider 构造请求体时读取并映射为各家 wire 字段。
    fn set_reasoning_pref(&self, _pref: Option<ReasoningPreference>) {}

    /// 带联网搜索参数的对话查询
    ///
    /// `enable_search` 控制本次调用是否注入联网搜索字段：
    /// - DeepSeek/Qwen/通用：顶层 `enable_search=true`
    /// - GPT-4o: `web_search_options={"search_context_size": "high"}`
    /// - Gemini: `tools=[{"google_search": {}}]`（REST 注入）
    ///
    /// 默认实现忽略 `enable_search`，回退到普通 `call_chat`，保证向后兼容。
    async fn call_chat_with_search(
        &self,
        messages: Vec<ChatMessage>,
        enable_search: bool,
        json_schema: Option<Value>,
    ) -> VivianResult<String> {
        let _ = (enable_search, json_schema);
        self.call_chat(messages).await
    }

    /// 结构化对话调用
    ///
    /// 默认实现回退到 `call_chat`，把文本包装成不含 tool_calls 的 ChatResponse。
    /// 支持原生 function calling 的 provider 应覆盖此方法以返回结构化 tool_calls。
    async fn invoke(&self, messages: Vec<ChatMessage>) -> VivianResult<ChatResponse> {
        let content = self.call_chat(messages).await?;
        Ok(ChatResponse::from_text(content))
    }

    /// 绑定工具列表
    ///
    /// 返回一个新的 provider 实例，该实例在后续调用中会向 LLM 透传工具 schema。
    /// 默认实现返回 `NotImplemented` 错误，表示当前 provider 不支持原生 function calling。
    /// 支持的 provider（如 OpenAiCompatProvider）应覆盖此方法。
    fn bind_tools(
        &self,
        _tools: Vec<ToolDefinition>,
    ) -> VivianResult<Box<dyn BaseProvider>> {
        Err(VivianError::NotImplemented(format!(
            "bind_tools 未实现: provider {} 不支持原生 function calling",
            self.get_model()
        )))
    }

    /// 是否支持原生 function calling
    ///
    /// 返回 true 时调用方应优先走 `bind_tools` + `invoke` 路径，以获得：
    /// - 结构化 tool_calls 响应（无非法 JSON 风险）
    /// - 节省 prompt token（schema 走 API 专用通道，部分服务商可缓存）
    /// - 更高的调用准确率（模型经训练识别 schema）
    ///
    /// 默认 false，由支持原生 function calling 的 provider 覆盖。
    /// 配置层可通过 `enable_native_function_calling=false` 全局禁用，绕过此能力。
    fn supports_native_function_calling(&self) -> bool {
        false
    }

    /// 是否支持 Structured Outputs（JSON Schema 约束）
    ///
    /// 返回 true 时调用方可以通过请求级 `json_schema` 参数传入 schema，
    /// provider 会把 schema 转成对应协议的字段注入请求：
    /// - OpenAI Responses：`text.format.type=json_schema`
    /// - 豆包 Responses：`response_format.type=json_schema`
    /// - Gemini：`generationConfig.responseSchema`
    /// - Anthropic：包装成 `emit_response` 伪工具（tool_use 通道）
    ///
    /// 与 `supports_native_function_calling` 是叠加关系：FC 走工具调用通道，
    /// schema 走结构化字段通道，两者可以同时启用。
    ///
    /// 默认 false，由支持 Structured Outputs 的 provider 覆盖。
    fn supports_structured_output(&self) -> bool {
        false
    }

    /// 是否支持 JSON Mode（仅保证返回合法 JSON，不约束字段）
    ///
    /// 返回 true 时即使请求级 `json_schema` 被传入，provider 也只能退化为
    /// `response_format.type=json_object`（OpenAI 兼容）或同等语义。
    /// 后端需自行做 schema 校验（失败回退到 JsonParser 兜底）。
    ///
    /// 默认 false。DeepSeek / Qwen 等 OpenAI 兼容端点覆盖为 true。
    fn supports_json_mode(&self) -> bool {
        false
    }

    /// 流式 + 原生 function calling 路径
    ///
    /// 与 `invoke` 的区别：返回 `StreamEvent` 流而非一次性 `ChatResponse`。
    /// 调用方应在 `supports_native_function_calling` 返回 true 时使用此方法。
    ///
    /// 流式工具调用的事件序列（OpenAI 风格）：
    /// 1. 多个 `StreamEvent::Text(chunk)` —— 模型生成的文本增量
    /// 2. 多个 `StreamEvent::ToolCallDelta { index, id?, name?, arguments_delta? }`
    ///    —— 工具调用增量，调用方需按 `index` 累积 `id`/`name`/`arguments`
    /// 3. `StreamEvent::Done` —— 流结束
    ///
    /// Anthropic 风格的差异：通过 `content_block_start` 给出完整的 `id` 和 `name`，
    /// 后续 `input_json_delta` 仅推送 arguments 增量。`StreamEvent::ToolCallDelta`
    /// 统一封装两种风格，调用方无需区分。
    ///
    /// 默认实现返回 `NotImplemented`，由支持的 provider 覆盖。
    async fn stream_with_tools(
        &self,
        _messages: Vec<ChatMessage>,
        _tools: Vec<ToolDefinition>,
    ) -> VivianResult<tokio::sync::mpsc::Receiver<StreamEvent>> {
        Err(VivianError::NotImplemented(format!(
            "stream_with_tools 未实现: provider {} 不支持流式原生 function calling",
            self.get_model()
        )))
    }
}

/// 流式 + 原生 function calling 的事件类型
///
/// 调用方按以下规则累积工具调用：
/// - 收到 `ToolCallDelta` 时按 `index` 分组
/// - `id` 和 `name` 仅在首个 delta 中出现（OpenAI 风格）或 `content_block_start` 中给出（Anthropic 风格）
/// - `arguments_delta` 是字符串增量，需拼接后 JSON.parse 得到最终参数
/// - `Done` 标志流结束，此时所有工具调用应已完整
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StreamEvent {
    /// 文本增量（模型生成的自然语言片段）
    Text {
        content: String,
    },
    /// 推理/思维链增量
    ///
    /// 不入可见输出流。来源：
    /// - OpenAI 兼容：`delta.reasoning_content`（DeepSeek / Qwen / GLM / 火山）
    /// - Anthropic：`thinking_delta` 事件（Claude extended thinking）
    Thinking {
        content: String,
    },
    /// 工具调用增量
    ToolCallDelta {
        /// 工具调用索引（用于多工具并发调用时区分）
        index: usize,
        /// 工具调用 ID（仅首个 delta 携带）
        id: Option<String>,
        /// 工具名（仅首个 delta 携带）
        name: Option<String>,
        /// 参数 JSON 字符串增量（多次累积后 JSON.parse）
        arguments_delta: Option<String>,
    },
    /// 流结束
    Done {
        /// 结束原因（stop / tool_calls / length / content_filter）
        finish_reason: Option<String>,
    },
    /// Token 用量（部分 provider 在流末尾返回，如 DeepSeek 缓存命中统计）
    ///
    /// 语义：input_tokens 是**未命中缓存**的输入，缓存读取/写入单独计（计费输入 = 三者之和）。
    Usage {
        /// 未命中缓存的输入 token
        input_tokens: u64,
        /// 输出 token
        output_tokens: u64,
        /// 前缀缓存命中 token（DeepSeek prompt_cache_hit_tokens / OpenAI cached_tokens）
        #[serde(default)]
        cache_read_tokens: u64,
        /// 缓存写入 token（Anthropic 风格；其余 provider 为 0）
        #[serde(default)]
        cache_write_tokens: u64,
    },
    /// 错误事件（流中断）
    Error {
        message: String,
    },
}

/// 从流式响应末尾的 usage 对象解析 token 用量（兼容 OpenAI / DeepSeek / Anthropic 字段名）。
///
/// - 总输入：`input_tokens`（Responses API）/ `prompt_tokens`（Chat Completions）
/// - 缓存命中：`input_tokens_details.cached_tokens`（OpenAI）/ `prompt_cache_hit_tokens`（DeepSeek）
/// - 返回的 `input_tokens` 已折算为**未命中缓存**的输入（总输入 − 缓存命中）
pub fn parse_stream_usage(usage: &serde_json::Value) -> Option<StreamEvent> {
    if !usage.is_object() {
        return None;
    }
    let total_input = usage["input_tokens"]
        .as_u64()
        .or_else(|| usage["prompt_tokens"].as_u64())?;
    let output = usage["output_tokens"]
        .as_u64()
        .or_else(|| usage["completion_tokens"].as_u64())
        .unwrap_or(0);
    let cache_read = usage["input_tokens_details"]["cached_tokens"]
        .as_u64()
        .or_else(|| usage["prompt_cache_hit_tokens"].as_u64())
        .or_else(|| usage["prompt_tokens_details"]["cached_tokens"].as_u64())
        .unwrap_or(0)
        .min(total_input);
    let cache_write = usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
    Some(StreamEvent::Usage {
        input_tokens: total_input - cache_read,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStats {
    pub model: String,
    pub total_calls: u64,
    pub successful_calls: u64,
    pub failed_calls: u64,
}

pub struct ProviderBase {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub temperature: f64,
    pub max_tokens: u32,
    pub circuit_breaker: Arc<RwLock<CircuitBreaker>>,
    pub request_cache: Mutex<HashMap<String, (String, Instant)>>,
    /// 联网搜索开关（运行时可切换）
    pub enable_search: AtomicBool,
    /// 代理地址（None 表示直连）
    pub proxy: Option<String>,
    /// 专属 HTTP 客户端（带代理配置时创建，None 时回退到全局客户端）
    pub client: Option<reqwest::Client>,
    /// max_tokens 运行时覆盖（0 表示用 max_tokens 默认值；>0 时优先使用）
    /// 凝神模式激活时由生成层设置，退出后清零。
    pub max_tokens_override: AtomicU32,
    /// temperature 运行时覆盖（存储 f64::to_bits()；0 表示无覆盖，用配置默认值）。
    /// 由 emotion→temperature 映射在每轮对话前设置，让 LLM 输出温度随情绪变化。
    pub temperature_override: AtomicU64,
    /// 工作智能体模式：true 时请求体省略 temperature 字段（交服务端默认）。
    /// 编程任务对确定性要求高，且推理模型对非默认温度敏感
    /// （OpenAI o 系列仅接受默认值，reasoner 忽略该参数），
    /// 故工作智能体模型统一不发送 temperature。
    pub omit_temperature: AtomicBool,
    /// 推理偏好运行时覆盖（None 表示不干预，交由服务端默认）。
    /// ModelRouter 按请求设置 / 恢复，provider 构造请求体时读取。
    pub reasoning_pref: RwLock<Option<ReasoningPreference>>,
}

impl ProviderBase {
    pub fn new(
        api_key: String,
        base_url: String,
        model: String,
        temperature: f64,
        max_tokens: u32,
    ) -> Self {
        let breaker_name = format!("provider:{}", model);
        let circuit_breaker = register_circuit_breaker(
            breaker_name,
            CB_FAILURE_THRESHOLD,
            CB_FAILURE_RATE,
            CB_RESET_TIMEOUT,
        );
        Self {
            api_key,
            base_url,
            model,
            temperature,
            max_tokens,
            circuit_breaker,
            request_cache: Mutex::new(HashMap::new()),
            enable_search: AtomicBool::new(false),
            proxy: None,
            client: None,
            max_tokens_override: AtomicU32::new(0),
            temperature_override: AtomicU64::new(0),
            omit_temperature: AtomicBool::new(false),
            reasoning_pref: RwLock::new(None),
        }
    }

    /// 设置联网搜索开关（运行时可切换）
    pub fn set_enable_search(&self, enable: bool) {
        self.enable_search.store(enable, Ordering::Relaxed);
    }

    /// 返回当前生效的 max_tokens：覆盖值 > 0 时在默认值上叠加，否则用配置默认值。
    pub fn effective_max_tokens(&self) -> u32 {
        let ov = self.max_tokens_override.load(Ordering::Relaxed);
        if ov > 0 {
            self.max_tokens.saturating_add(ov)
        } else {
            self.max_tokens
        }
    }

    /// 设置 max_tokens 运行时覆盖（0 表示恢复默认）。
    pub fn set_max_tokens_override(&self, tokens: u32) {
        self.max_tokens_override.store(tokens, Ordering::Relaxed);
    }

    /// 设置 temperature 运行时覆盖。
    /// 传入 None 清除覆盖（恢复配置默认值）；传入 Some(t) 设置覆盖温度。
    pub fn set_temperature_override(&self, temp: Option<f64>) {
        match temp {
            Some(t) => {
                let bits = t.to_bits();
                // 0.0 的 bits 是 0，用特殊值 1 区分"无覆盖"
                let stored = if bits == 0 { 1 } else { bits };
                self.temperature_override.store(stored, Ordering::Relaxed);
            }
            None => {
                self.temperature_override.store(0, Ordering::Relaxed);
            }
        }
    }

    /// 返回当前生效的 temperature：有覆盖时用覆盖值，否则用配置默认值。
    pub fn effective_temperature(&self) -> f64 {
        let stored = self.temperature_override.load(Ordering::Relaxed);
        if stored == 0 {
            self.temperature
        } else {
            f64::from_bits(stored)
        }
    }

    /// 设置是否在请求体中省略 temperature 字段（工作智能体模型用）。
    pub fn set_omit_temperature(&self, omit: bool) {
        self.omit_temperature.store(omit, Ordering::Relaxed);
    }

    /// 是否处于省略 temperature 模式。
    pub fn should_omit_temperature(&self) -> bool {
        self.omit_temperature.load(Ordering::Relaxed)
    }

    /// 若处于省略模式，按各厂商请求体路径移除 temperature 字段。
    ///
    /// 路径覆盖：
    /// - 顶层 `temperature`：OpenAI 兼容 / Responses / Anthropic / 文心 / 豆包 / 智谱 等
    /// - `generationConfig.temperature`：Gemini
    /// - `parameter.chat.temperature`：讯飞星火
    ///
    /// 不做递归移除：消息内容 / 工具 JSON Schema 中可能出现名为
    /// temperature 的业务字段（如天气工具的参数定义），递归删除会破坏语义。
    pub fn strip_temperature(&self, body: &mut serde_json::Value) {
        if !self.should_omit_temperature() {
            return;
        }
        if let Some(obj) = body.as_object_mut() {
            obj.remove("temperature");
        }
        if let Some(cfg) = body
            .get_mut("generationConfig")
            .and_then(serde_json::Value::as_object_mut)
        {
            cfg.remove("temperature");
        }
        if let Some(chat) = body
            .get_mut("parameter")
            .and_then(serde_json::Value::as_object_mut)
            .and_then(|p| p.get_mut("chat"))
            .and_then(serde_json::Value::as_object_mut)
        {
            chat.remove("temperature");
        }
    }

    /// 设置推理偏好运行时覆盖（None 表示不干预，交由服务端默认）。
    pub fn set_reasoning_pref(&self, pref: Option<ReasoningPreference>) {
        *self.reasoning_pref.write() = pref;
    }

    /// 返回当前生效的推理偏好：无覆盖时为 Auto（不干预）。
    pub fn effective_reasoning(&self) -> ReasoningPreference {
        self.reasoning_pref.read().unwrap_or(ReasoningPreference::AUTO)
    }

    /// 读取联网搜索开关
    pub fn is_enable_search(&self) -> bool {
        self.enable_search.load(Ordering::Relaxed)
    }

    /// 获取 HTTP 客户端：优先使用专属客户端（带代理），否则回退到全局客户端
    pub fn get_client(&self) -> reqwest::Client {
        self.client
            .clone()
            .unwrap_or_else(get_global_client)
    }

    pub fn get_cache_key(&self, prompt: &str) -> String {
        use md5::{Digest, Md5};
        let digest = Md5::digest(format!("{}:{}", self.model, prompt).as_bytes());
        format!("{:x}", digest)
    }

    pub fn get_cached_response(&self, prompt: &str) -> Option<String> {
        let key = self.get_cache_key(prompt);
        let cache = self.request_cache.lock();
        if let Some((response, timestamp)) = cache.get(&key) {
            if timestamp.elapsed() < CACHE_TTL {
                return Some(response.clone());
            }
        }
        None
    }

    pub fn cache_response(&self, prompt: &str, response: &str) {
        let key = self.get_cache_key(prompt);
        let mut cache = self.request_cache.lock();
        if cache.len() >= CACHE_MAX_ENTRIES {
            cache.retain(|_, (_, ts)| ts.elapsed() < CACHE_TTL);
        }
        cache.insert(key, (response.to_string(), Instant::now()));
    }

    pub fn check_circuit(&self) -> VivianResult<()> {
        let mut breaker = self.circuit_breaker.write();
        if !breaker.allow_request() {
            return Err(VivianError::CircuitBreaker(format!(
                "熔断器已打开: {}",
                breaker.name
            )));
        }
        Ok(())
    }

    pub fn record_success(&self) {
        self.circuit_breaker.write().record_success();
    }

    pub fn record_failure(&self) {
        self.circuit_breaker.write().record_failure();
    }

    pub fn get_stats(&self) -> ProviderStats {
        let breaker = self.circuit_breaker.read();
        ProviderStats {
            model: self.model.clone(),
            total_calls: breaker.success_count as u64 + breaker.failure_count as u64,
            successful_calls: breaker.success_count as u64,
            failed_calls: breaker.failure_count as u64,
        }
    }
}
