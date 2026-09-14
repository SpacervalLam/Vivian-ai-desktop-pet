//! 厂商能力表 —— provider 层的厂商差异单一事实来源。
//!
//! 各模块（推理控制 / 缓存策略 / 能力门控）通过 `detect_vendor(model)` +
//! `vendor_capability(vendor)` 查询厂商元数据，避免在调度层散落
//! `if model.starts_with("glm-")` 之类的硬编码判断。
//!
//! 厂商识别以模型名前缀为准（同一 OpenAI 兼容端点可服务多家厂商的模型，
//! 按 provider 类型无法区分，按模型名识别最可靠）。

use serde::{Deserialize, Serialize};

/// 推理内容在响应中的字段风格。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThinkingField {
    /// OpenAI 兼容风格：`message.reasoning_content`（DeepSeek / Qwen / GLM / 火山）
    ReasoningContent,
    /// Anthropic / MiniMax 风格：`content[].type=="thinking"` 块
    Thinking,
    /// 无推理内容字段
    None,
}

/// 提示缓存策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheStrategy {
    /// 顶层 `prompt_cache_key` 字段（Kimi / Moonshot）
    PromptCacheKey,
    /// 消息块 `cache_control` 标记（Anthropic / MiniMax Anthropic 入口）
    CacheControl,
    /// 按模型名启发式（Kimi/Moonshot 模型自动注入 prompt_cache_key）
    Auto,
    /// 无缓存机制
    None,
}

/// 一家厂商的静态能力描述。
pub struct VendorCapability {
    /// 厂商标识
    pub id: VendorId,
    /// 展示名（中文）
    pub display_name: &'static str,
    /// 推理内容字段风格
    pub thinking_field: ThinkingField,
    /// 提示缓存策略
    pub cache_strategy: CacheStrategy,
    /// 是否支持原生 function calling
    pub supports_tools: bool,
    /// 是否存在推理/思考型模型
    pub supports_thinking: bool,
    /// 默认模型是否支持视觉输入（视觉版模型需按名判断，此处为厂商主力模型口径）
    pub supports_vision: bool,
}

/// 厂商标识（由模型名前缀识别）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VendorId {
    OpenAi,
    Anthropic,
    Deepseek,
    Glm,
    Kimi,
    Qwen,
    Doubao,
    MiniMax,
    Mimo,
    Gemini,
    Wenxin,
    Spark,
    Unknown,
}

/// 厂商能力表。
static VENDOR_CAPABILITIES: &[VendorCapability] = &[
    VendorCapability {
        id: VendorId::OpenAi,
        display_name: "OpenAI",
        thinking_field: ThinkingField::ReasoningContent,
        cache_strategy: CacheStrategy::Auto,
        supports_tools: true,
        supports_thinking: true,
        supports_vision: false,
    },
    VendorCapability {
        id: VendorId::Anthropic,
        display_name: "Anthropic",
        thinking_field: ThinkingField::Thinking,
        cache_strategy: CacheStrategy::CacheControl,
        supports_tools: true,
        supports_thinking: true,
        supports_vision: true,
    },
    VendorCapability {
        id: VendorId::Deepseek,
        display_name: "DeepSeek",
        thinking_field: ThinkingField::ReasoningContent,
        cache_strategy: CacheStrategy::Auto,
        supports_tools: true,
        supports_thinking: true,
        supports_vision: false,
    },
    VendorCapability {
        id: VendorId::Glm,
        display_name: "智谱 GLM",
        thinking_field: ThinkingField::ReasoningContent,
        cache_strategy: CacheStrategy::Auto,
        supports_tools: true,
        supports_thinking: true,
        supports_vision: false,
    },
    VendorCapability {
        id: VendorId::Kimi,
        display_name: "月之暗面 Kimi",
        thinking_field: ThinkingField::ReasoningContent,
        cache_strategy: CacheStrategy::PromptCacheKey,
        supports_tools: true,
        supports_thinking: true,
        supports_vision: true,
    },
    VendorCapability {
        id: VendorId::Qwen,
        display_name: "通义千问",
        thinking_field: ThinkingField::ReasoningContent,
        cache_strategy: CacheStrategy::Auto,
        supports_tools: true,
        supports_thinking: true,
        supports_vision: false,
    },
    VendorCapability {
        id: VendorId::Doubao,
        display_name: "豆包",
        thinking_field: ThinkingField::ReasoningContent,
        cache_strategy: CacheStrategy::None,
        supports_tools: true,
        supports_thinking: true,
        supports_vision: true,
    },
    VendorCapability {
        id: VendorId::MiniMax,
        display_name: "MiniMax",
        thinking_field: ThinkingField::Thinking,
        cache_strategy: CacheStrategy::CacheControl,
        supports_tools: true,
        supports_thinking: true,
        supports_vision: true,
    },
    VendorCapability {
        id: VendorId::Mimo,
        display_name: "小米 MiMo",
        thinking_field: ThinkingField::ReasoningContent,
        cache_strategy: CacheStrategy::Auto,
        supports_tools: true,
        supports_thinking: true,
        supports_vision: true,
    },
    VendorCapability {
        id: VendorId::Gemini,
        display_name: "Google Gemini",
        thinking_field: ThinkingField::None,
        cache_strategy: CacheStrategy::None,
        supports_tools: true,
        supports_thinking: true,
        supports_vision: true,
    },
    VendorCapability {
        id: VendorId::Wenxin,
        display_name: "文心一言",
        thinking_field: ThinkingField::None,
        cache_strategy: CacheStrategy::None,
        supports_tools: true,
        supports_thinking: false,
        supports_vision: true,
    },
    VendorCapability {
        id: VendorId::Spark,
        display_name: "讯飞星火",
        thinking_field: ThinkingField::None,
        cache_strategy: CacheStrategy::None,
        supports_tools: true,
        supports_thinking: false,
        supports_vision: true,
    },
    VendorCapability {
        id: VendorId::Unknown,
        display_name: "未知厂商",
        thinking_field: ThinkingField::ReasoningContent,
        cache_strategy: CacheStrategy::None,
        supports_tools: true,
        supports_thinking: false,
        supports_vision: false,
    },
];

/// 按模型名识别厂商（大小写不敏感，前缀匹配）。
///
/// 同一 OpenAI 兼容端点可服务多家厂商的模型，因此以模型名而非
/// provider 类型作为识别依据。
pub fn detect_vendor(model: &str) -> VendorId {
    let m = model.to_lowercase();
    if m.starts_with("gpt-")
        || m.starts_with("o1")
        || m.starts_with("o3")
        || m.starts_with("o4")
        || m.starts_with("chatgpt")
    {
        VendorId::OpenAi
    } else if m.starts_with("claude-") {
        VendorId::Anthropic
    } else if m.starts_with("deepseek") {
        VendorId::Deepseek
    } else if m.starts_with("glm") || m.starts_with("chatglm") {
        VendorId::Glm
    } else if m.starts_with("kimi") || m.starts_with("moonshot") {
        VendorId::Kimi
    } else if m.starts_with("qwen") || m.starts_with("qwq") {
        VendorId::Qwen
    } else if m.starts_with("doubao") {
        VendorId::Doubao
    } else if m.starts_with("minimax") || m.starts_with("abab") {
        VendorId::MiniMax
    } else if m.starts_with("mimo") {
        VendorId::Mimo
    } else if m.starts_with("gemini") {
        VendorId::Gemini
    } else if m.starts_with("ernie") {
        VendorId::Wenxin
    } else if m.starts_with("spark") {
        VendorId::Spark
    } else {
        VendorId::Unknown
    }
}

/// 查询厂商能力元数据（未知厂商返回保守兜底值）。
pub fn vendor_capability(vendor: VendorId) -> &'static VendorCapability {
    VENDOR_CAPABILITIES
        .iter()
        .find(|c| c.id == vendor)
        .unwrap_or_else(|| VENDOR_CAPABILITIES.last().expect("能力表非空"))
}

/// 按模型名给出该厂商主力模型的上下文窗口默认值（tokens）。
///
/// 用于自动压缩阈值判定：用户未显式配置 context_window 时的兜底，
/// 避免 128k/200k 小窗口模型按 1M 判定导致压缩永不触发。
pub fn default_context_window(model: &str) -> u64 {
    match detect_vendor(model) {
        VendorId::OpenAi => 400_000,
        VendorId::Anthropic => 200_000,
        VendorId::Gemini => 1_000_000,
        VendorId::Deepseek => 128_000,
        VendorId::Glm => 131_072,
        VendorId::Kimi => 131_072,
        VendorId::Qwen => 131_072,
        VendorId::Doubao => 256_000,
        VendorId::MiniMax => 200_000,
        VendorId::Mimo => 131_072,
        VendorId::Wenxin | VendorId::Spark => 8_192,
        VendorId::Unknown => 131_072,
    }
}
