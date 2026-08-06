use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{VivianError, VivianResult};
use crate::utils::path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub base: BaseConfig,
    pub window: WindowConfig,
    pub live2d_render: Live2dRenderConfig,
    pub ai: AiConfig,
    pub network: NetworkConfig,
    pub routing_matrix: HashMap<String, TaskRouteConfig>,
    pub memory: MemoryConfig,
    pub speech_recognition: SpeechRecognitionConfig,
    #[serde(default)]
    pub proactive: ProactiveConfig,
    #[serde(default)]
    pub enable_routing_matrix: bool,
    /// 工具系统配置（工具页签）
    #[serde(default)]
    pub tools: ToolConfig,
    /// 真实世界感知配置（让 Vivian 在真实世界中"活着"）
    #[serde(default)]
    pub world: WorldConfig,
    /// 服务商切换缓存：按 preset id 索引各家配置快照，切换预设时自动恢复
    #[serde(default)]
    pub provider_cache: HashMap<String, CachedProviderProfile>,
    /// 豆包端到端实时语音通话配置（SC2.0）
    #[serde(default)]
    pub realtime_voice: RealtimeVoiceConfig,
    /// 多角色配置
    #[serde(default)]
    pub characters: CharactersConfig,
    /// 网络搜索后端配置（provider 切换 + 各家 API Key）
    #[serde(default)]
    pub web_search: WebSearchConfig,
    /// 内联表情/动作标签配置
    #[serde(default)]
    pub inline_expression: InlineExpressionConfig,
}

/// 多角色配置
///
/// 定义应用支持的所有桌面宠物角色。每个角色有独立的人格、记忆、心理状态、对话历史和用户事实画像，
/// 共享 LLM 配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharactersConfig {
    /// 所有角色列表
    #[serde(default = "default_characters")]
    pub list: Vec<CharacterEntry>,
    /// 当前活跃角色 ID（点击选中的对话目标）
    #[serde(default = "default_active_character")]
    pub active_id: String,
}

impl Default for CharactersConfig {
    fn default() -> Self {
        default_characters_config()
    }
}

/// 单个角色配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterEntry {
    pub id: String,
    pub name: String,
    #[serde(default = "default_character_live2d_model")]
    pub live2d_model: String,
    #[serde(default = "default_model_kind")]
    pub model_kind: String,
    #[serde(default = "default_true")]
    pub default_online: bool,
}

fn default_model_kind() -> String {
    "live2d".to_string()
}

fn default_characters_config() -> CharactersConfig {
    CharactersConfig {
        list: default_characters(),
        active_id: default_active_character(),
    }
}

fn default_characters() -> Vec<CharacterEntry> {
    vec![
        CharacterEntry {
            id: "nana".to_string(),
            name: "Nana".to_string(),
            live2d_model: "Nana".to_string(),
            model_kind: "live2d".to_string(),
            default_online: true,
        },
        CharacterEntry {
            id: "vivian".to_string(),
            name: "Vivian".to_string(),
            live2d_model: "Vivian".to_string(),
            model_kind: "live2d".to_string(),
            default_online: true,
        },
    ]
}

fn default_active_character() -> String {
    "nana".to_string()
}

// ============================================================================
// WebSearchConfig — 网络搜索后端配置
// ============================================================================

/// 网络搜索后端配置
///
/// 支持多引擎**混用**：用户可同时启用 duckduckgo / searxng / tavily，
/// 搜索工具会并发调用所有已配置的引擎，合并去重后返回。
///
/// - `providers`：启用的引擎列表（至少包含一个；DuckDuckGo 零配置始终可用）
/// - 未配置必要参数的引擎（如 SearXNG 未填 base_url）会被跳过并记录日志
/// - 若所有引擎都不可用，最终回退到 DuckDuckGo 兜底
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchConfig {
    /// 已弃用：旧版单一引擎选择（保留仅为向后兼容迁移）
    /// 新代码应使用 `providers` 字段
    #[serde(default)]
    pub provider: String,
    /// 启用的搜索引擎列表（混用模式）
    /// 候选值：duckduckgo / searxng / tavily
    /// 同时启用多个引擎时，搜索工具会并发调用并合并去重结果
    #[serde(default = "default_web_search_providers")]
    pub providers: Vec<String>,
    /// 每次搜索返回结果数（1-20）
    #[serde(default = "default_web_search_max_results")]
    pub max_results: u32,
    /// 搜索请求超时（秒）
    #[serde(default = "default_web_search_timeout_secs")]
    pub timeout_secs: u64,
    /// 是否在 Busy 状态下主动检索知识
    #[serde(default = "default_true")]
    pub enable_background_knowledge_fetch: bool,
    /// 语言偏好（如 "zh-CN" / "en" / "ja"），空表示不限定
    #[serde(default)]
    pub language: Option<String>,

    /// SearXNG 配置（自部署元搜索引擎，聚合多源结果）
    #[serde(default)]
    pub searxng: SearXngConfig,
    /// Tavily 配置（专为 LLM Agent 设计的搜索 API）
    #[serde(default)]
    pub tavily: TavilyConfig,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            provider: String::new(),
            providers: default_web_search_providers(),
            max_results: default_web_search_max_results(),
            timeout_secs: default_web_search_timeout_secs(),
            enable_background_knowledge_fetch: true,
            language: None,
            searxng: SearXngConfig::default(),
            tavily: TavilyConfig::default(),
        }
    }
}

fn default_web_search_providers() -> Vec<String> {
    vec!["duckduckgo".to_string()]
}

fn default_web_search_max_results() -> u32 {
    5
}

fn default_web_search_timeout_secs() -> u64 {
    15
}

/// SearXNG 配置（自部署元搜索引擎）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearXngConfig {
    /// SearXNG 实例 base URL（如 `http://localhost:8080` 或公共实例 `https://searx.example.com`）
    /// 末尾不要带 `/`。本地 Docker 部署默认端口 8080。
    #[serde(default)]
    pub base_url: String,
    /// 用于访问受保护实例的 token（若实例配置了 `search.formats` 限制）
    #[serde(default)]
    pub auth_token: String,
}

impl Default for SearXngConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            auth_token: String::new(),
        }
    }
}

/// Tavily API 配置（专为 LLM Agent 设计）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TavilyConfig {
    /// Tavily API Key（https://tavily.com 注册获取，免费 1000 次/月）
    #[serde(default)]
    pub api_key: String,
    /// 是否返回原文摘录（true）还是仅摘要（false）
    #[serde(default = "default_true")]
    pub include_raw_content: bool,
    /// 搜索深度：basic（默认）/ advanced（更全面但更慢，多消耗 1 credit）
    #[serde(default = "default_tavily_search_depth")]
    pub search_depth: String,
}

impl Default for TavilyConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            include_raw_content: true,
            search_depth: default_tavily_search_depth(),
        }
    }
}

fn default_tavily_search_depth() -> String {
    "basic".to_string()
}

/// 真实世界感知配置
///
/// 控制让 Vivian 感知真实世界（时间/节气/节日/日出日落/天气），
/// 以及离线时的内心活动（内心独白、作息、记忆巩固）。
/// 这些功能会加大 token 消耗，可由 `enable` 总开关统一关闭。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldConfig {
    /// 总开关：关闭后所有"活着"功能停止（不感知世界、不内心独白、不记忆巩固）
    #[serde(default = "default_true")]
    pub enable: bool,
    /// 是否注入世界快照到 prompt（让 Vivian 知道外面在下雨/今天是中秋）
    #[serde(default = "default_true")]
    pub inject_into_prompt: bool,
    // ── 天气 ──
    /// 是否启用天气感知
    #[serde(default = "default_true")]
    pub enable_weather: bool,
    /// 天气缓存 TTL（秒），默认 3600（1 小时）
    #[serde(default = "default_weather_cache_ttl")]
    pub weather_cache_ttl_secs: u64,
    /// 纬度（度，北正南负）—— 用于天气获取与日出日落计算
    #[serde(default)]
    pub latitude: Option<f64>,
    /// 经度（度，东正西负）
    #[serde(default)]
    pub longitude: Option<f64>,
    /// 城市名称
    #[serde(default)]
    pub city: Option<String>,
    /// 地区/省份名称
    #[serde(default)]
    pub region: Option<String>,
    /// 国家名称
    #[serde(default)]
    pub country: Option<String>,
    // ── 内心独白 ──
    /// 是否启用内心独白（离线时 LLM 自主思考，写入记忆，不打扰用户）
    #[serde(default = "default_true")]
    pub enable_inner_monologue: bool,
    // ── 记忆巩固 ──
    /// 是否启用夜间记忆巩固（睡眠时整理记忆）
    #[serde(default = "default_true")]
    pub enable_memory_consolidation: bool,
    // ── 作息（让桌宠的睡眠时间可配置，而非写死） ──
    /// 入睡小时（0-23），默认 1（凌晨 1 点开始真正入睡）
    #[serde(default = "default_sleep_start_hour")]
    pub sleep_start_hour: u32,
    /// 醒来小时（0-23），默认 6（凌晨 6 点醒来）
    #[serde(default = "default_sleep_end_hour")]
    pub sleep_end_hour: u32,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            enable: true,
            inject_into_prompt: true,
            enable_weather: true,
            weather_cache_ttl_secs: default_weather_cache_ttl(),
            latitude: None,
            longitude: None,
            city: None,
            region: None,
            country: None,
            enable_inner_monologue: true,
            enable_memory_consolidation: true,
            sleep_start_hour: default_sleep_start_hour(),
            sleep_end_hour: default_sleep_end_hour(),
        }
    }
}

fn default_weather_cache_ttl() -> u64 {
    3600
}

fn default_sleep_start_hour() -> u32 {
    1
}

fn default_sleep_end_hour() -> u32 {
    6
}

/// 检测系统语言并映射为应用支持的界面语言代码
///
/// 仅在首次创建 config.yaml 时调用，作为默认界面语言。
/// 映射规则：zh* → zh-CN，ja* → ja，其余 → en（与前端 i18n 支持的三种语言对齐）。
/// 检测失败时回退到 zh-CN。
fn detect_default_language() -> String {
    let locale = sys_locale::get_locale().unwrap_or_default();
    if locale.is_empty() {
        return "zh-CN".to_string();
    }
    let lower = locale.to_lowercase();
    if lower.starts_with("zh") {
        "zh-CN".to_string()
    } else if lower.starts_with("ja") {
        "ja".to_string()
    } else if lower.starts_with("en") {
        "en".to_string()
    } else {
        "zh-CN".to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseConfig {
    pub language: String,
    /// 界面主题：system（跟随系统）/ light（浅色）/ dark（深色）
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_shortcut")]
    pub shortcut: String,
    /// Nana 私聊输入框快捷键
    #[serde(default = "default_shortcut_nana")]
    pub shortcut_nana: String,
    /// 总框快捷键（同时向两个角色发言），弹窗居中显示
    #[serde(default = "default_shortcut_broadcast")]
    pub shortcut_broadcast: String,
    /// 用户自定义头像在本机用户数据目录下的相对路径（如 "avatar.png"）。
    /// 为 None 时不使用自定义头像，前端回退到默认蓝色"我"字图标。
    #[serde(default)]
    pub user_avatar_path: Option<String>,
}

fn default_shortcut() -> String {
    "CommandOrControl+Shift+A".to_string()
}

fn default_theme() -> String {
    "system".to_string()
}

fn default_shortcut_nana() -> String {
    "CommandOrControl+Shift+Q".to_string()
}

fn default_shortcut_broadcast() -> String {
    "CommandOrControl+Shift+Z".to_string()
}

fn default_character_live2d_model() -> String {
    "Vivian".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    /// 智能避让：实时检测屏幕纯色区域，将桌宠移动到不遮挡内容的位置。
    /// 默认启用；用户可在设置面板的通用页签关闭。
    #[serde(default = "default_true")]
    pub smart_positioning_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Live2dRenderConfig {
    pub blink_interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    pub provider: String,
    pub model: String,
    pub api_key: Option<String>,
    pub endpoint: Option<String>,
    pub temperature: f64,
    pub max_tokens: u32,
    /// API Secret（仅文心 / 讯飞等需要 OAuth/HMAC 签名的服务商使用）
    #[serde(default)]
    pub api_secret: Option<String>,
    /// 应用 ID（仅讯飞星火等需要 app_id 的服务商使用）
    #[serde(default)]
    pub app_id: Option<String>,
    /// 是否启用原生 function calling（结构化 tools 字段路径）
    ///
    /// - true：当 provider 支持时走 `bind_tools` + `invoke` 结构化路径
    ///   （schema 走 API 专用通道，不占 prompt token，调用准确率更高）
    /// - false：始终走文本路径（在 system prompt 注入工具列表 + JSON 输出协议）
    ///
    /// 默认 true。不支持原生 fc 的 provider 会自动回退到文本路径。
    /// 旧配置存于 `ai.enable_native_function_calling`；新配置存于 `tools.enable_native_function_calling`，
    /// 加载时若新字段缺失则从旧字段继承。
    #[serde(default = "default_true")]
    pub enable_native_function_calling: bool,
    /// 是否启用图片输入（多模态）
    ///
    /// 开启后 user 消息可携带图片，provider 层按各家协议翻译为
    /// `image_url`（OpenAI 兼容）或 `image` block（Anthropic）。
    /// 不支持视觉的模型会忽略图片或报错，由用户自行确认模型能力。
    #[serde(default)]
    pub enable_vision: bool,
    /// 图片采样精度（OpenAI vision 协议的 `detail` 字段）
    ///
    /// - `auto`：由模型按分辨率自行决定（默认）
    /// - `low`：固定低分辨率（512×512），token 消耗低
    /// - `high`：高分辨率切片，细节更好但 token 消耗高
    #[serde(default = "default_image_detail")]
    pub image_detail: String,
}

/// 工具系统配置（设置窗口「工具」页签）
///
/// 集中管理工具执行/缓存/超时/原生 fc 等运行时参数，
/// 替代分散在 `executor.rs` / `registry.rs` / `tool_call_manager.rs` / `generation.rs` 的硬编码常量。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    // ── 原生 function calling ──
    /// 是否启用原生 function calling（结构化 tools 字段路径）
    ///
    /// 与 `ai.enable_native_function_calling` 含义相同，本字段为新增的主存储位置。
    /// 旧配置中的 `ai.enable_native_function_calling` 在加载时作为 fallback。
    #[serde(default = "default_true")]
    pub enable_native_function_calling: bool,

    // ── 执行轮次 ──
    /// 文本路径工具调用循环最大迭代数（`ToolCallManager::max_iterations`）
    #[serde(default = "default_tool_max_iterations")]
    pub max_iterations: u32,
    /// 原生 function calling 路径单次对话最大 LLM↔工具 往返轮次
    /// （`generation.rs` 的 `MAX_ROUNDS` / `MAX_EXTRA_ROUNDS+1`）
    #[serde(default = "default_tool_max_rounds")]
    pub max_rounds: u32,

    // ── 结果大小预算 ──
    /// 单工具结果字符预算，超出则截断为预览版（`executor.rs` 的 `MAX_RESULT_CHARS`）
    #[serde(default = "default_tool_max_result_chars")]
    pub max_result_chars: u32,
    /// 反馈提示词中工具结果 JSON 的截断长度（`tool_call_manager.rs` 的 `take(2000)`）
    #[serde(default = "default_tool_feedback_history_chars")]
    pub feedback_history_chars: u32,

    // ── 超时 ──
    /// 未登记工具的默认超时（秒）（`executor.rs` 的 `DEFAULT_TOOL_TIMEOUT_SECS`）
    #[serde(default = "default_tool_default_timeout_secs")]
    pub default_tool_timeout_secs: u64,
    /// 用户授权确认弹窗的最长等待时间（秒）（`registry.rs` 的 `Duration::from_secs(600)`）
    #[serde(default = "default_tool_confirmation_timeout_secs")]
    pub confirmation_timeout_secs: u64,

    // ── 缓存 ──
    /// 是否启用工具结果缓存
    #[serde(default = "default_true")]
    pub enable_cache: bool,
    /// 工具结果缓存 TTL（秒）（`registry.rs` 的 `ToolCache::new(300, ...)` 第一参数）
    #[serde(default = "default_tool_cache_ttl_secs")]
    pub cache_ttl_secs: u64,
    /// 工具结果缓存最大条目数（`registry.rs` 的 `ToolCache::new(..., 1000)` 第二参数）
    #[serde(default = "default_tool_cache_max_size")]
    pub cache_max_size: u32,

    // ── 提示缓存策略 ──
    /// LLM 提示缓存策略：`auto` / `prompt_cache_key` / `cache_control` / `none`
    ///
    /// - `auto`：由 provider 按模型启发式选择（默认）
    /// - `prompt_cache_key`：注入顶层字段（Kimi / Moonshot）
    /// - `cache_control`：Anthropic system 块打 ephemeral 标记
    /// - `none`：显式关闭
    #[serde(default = "default_cache_strategy")]
    pub cache_strategy: String,

    // ── 工具权限网关 ──
    /// Agent 文件访问级别：`read-only` / `fs-read` / `fs-write` / `shell` / `full-control`
    ///
    /// 与每个工具的 `risk()` 共同决定 `allow` / `ask` / `deny`：
    /// - 高风险工具在低权限级别下需要用户确认或被拒绝
    /// - `read-only` 仅允许安全工具（无副作用查询）
    /// - `full-control` 允许包括 shell / input-control 在内的全部工具
    #[serde(default = "default_access_level")]
    pub access_level: String,

    // ── 上下文窗口压缩 ──
    /// FC 循环中对话消息总 token 估算值超过此阈值时触发窗口级压缩（默认 20000）
    #[serde(default = "default_compress_threshold_tokens")]
    pub compress_threshold_tokens: usize,
    /// 压缩时保留的最近消息轮数（不会被截断）
    #[serde(default = "default_compress_keep_recent")]
    pub compress_keep_recent: usize,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            enable_native_function_calling: true,
            max_iterations: default_tool_max_iterations(),
            max_rounds: default_tool_max_rounds(),
            max_result_chars: default_tool_max_result_chars(),
            feedback_history_chars: default_tool_feedback_history_chars(),
            default_tool_timeout_secs: default_tool_default_timeout_secs(),
            confirmation_timeout_secs: default_tool_confirmation_timeout_secs(),
            enable_cache: true,
            cache_ttl_secs: default_tool_cache_ttl_secs(),
            cache_max_size: default_tool_cache_max_size(),
            cache_strategy: default_cache_strategy(),
            access_level: default_access_level(),
            compress_threshold_tokens: default_compress_threshold_tokens(),
            compress_keep_recent: default_compress_keep_recent(),
        }
    }
}

fn default_tool_max_iterations() -> u32 {
    10
}
fn default_tool_max_rounds() -> u32 {
    20
}
fn default_tool_max_result_chars() -> u32 {
    4000
}
fn default_tool_feedback_history_chars() -> u32 {
    2000
}
fn default_tool_default_timeout_secs() -> u64 {
    120
}
fn default_tool_confirmation_timeout_secs() -> u64 {
    600
}
fn default_tool_cache_ttl_secs() -> u64 {
    300
}
fn default_tool_cache_max_size() -> u32 {
    1000
}
fn default_cache_strategy() -> String {
    "auto".to_string()
}
fn default_access_level() -> String {
    "full-control".to_string()
}
fn default_compress_threshold_tokens() -> usize {
    20000
}
fn default_compress_keep_recent() -> usize {
    6
}
fn default_image_detail() -> String {
    "auto".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub proxy_mode: String,
    pub proxy_url: String,
    pub timeout: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

/// 服务商切换缓存：按 preset id 索引的 provider 配置快照
///
/// 用户在设置窗口切换服务商预设时，把当前槽位的 provider_type/endpoint/model/api_key/
/// api_secret/app_id 快照存入 `AppConfig.provider_cache`，切回该预设时自动恢复，
/// 避免反复填写。主配置与路由矩阵共享同一份缓存（同一家厂商的凭据应一致）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CachedProviderProfile {
    pub provider_type: String,
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
    #[serde(default)]
    pub api_secret: String,
    #[serde(default)]
    pub app_id: String,
}

/// 单个任务的完整路由配置 —— 字段与 LLM 主配置一致
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskRouteConfig {
    /// 服务商类型：openai / gemini / anthropic / wenxin / spark / custom
    pub provider_type: String,
    /// 模型名称
    pub model: String,
    /// API Key
    pub api_key: String,
    /// 接口端点
    pub endpoint: String,
    /// API Secret（仅文心 / 讯飞等需要 OAuth/HMAC 签名的服务商使用）
    #[serde(default)]
    pub api_secret: String,
    /// 应用 ID（仅讯飞星火等需要 app_id 的服务商使用）
    #[serde(default)]
    pub app_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub max_short_term_memory: usize,
    pub retrieval_strategy: String,
    pub enable_expiration: bool,
    /// 嵌入服务配置（独立于 routing_matrix.memory）
    #[serde(default)]
    pub embedding: EmbeddingConfig,
    /// 巩固流水线配置（ShortTerm→MidTerm→LongTerm 三阶段）
    #[serde(default)]
    pub consolidation: ConsolidationConfig,
    /// 检索五因子加权配置（recency + relevance + importance + hook_boost + need_sim）
    #[serde(default)]
    pub retrieval_weights: RetrievalWeightsConfig,
}

/// 巩固流水线配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationConfig {
    /// Stage 1 触发阈值：ShortTerm 条数 ≥ 此值时触发摘要
    #[serde(default = "default_stage1_threshold")]
    pub stage1_short_term_threshold: usize,
    /// Stage 1 触发空闲超时（秒）：会话空闲超过此值时触发摘要
    #[serde(default = "default_stage1_idle_sec")]
    pub stage1_idle_timeout_sec: f64,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            stage1_short_term_threshold: default_stage1_threshold(),
            stage1_idle_timeout_sec: default_stage1_idle_sec(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_stage1_threshold() -> usize {
    20
}
fn default_stage1_idle_sec() -> f64 {
    1800.0
}

/// 检索五因子加权配置
///
/// 综合 score = α·recency + β·relevance + γ·importance + δ·hook_boost + ε·need_sim
/// - recency = exp(-age_hours / tau)
/// - relevance = RRF fused_score（含 semantic_boost）
/// - importance = memory.importance（写入时 LLM 打分 + 命中反馈 delta）
/// - hook_boost = 含未闭环 open_hooks 的记忆获得加成（承诺/约定/待跟进优先 surfaced）
/// - need_sim = 当前用户输入与记忆内容的 Jaccard 语义相似度（jieba 分词）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalWeightsConfig {
    #[serde(default = "default_w_recency")]
    pub recency: f64,
    #[serde(default = "default_w_relevance")]
    pub relevance: f64,
    #[serde(default = "default_w_importance")]
    pub importance: f64,
    #[serde(default = "default_w_hook_boost")]
    pub hook_boost: f64,
    #[serde(default = "default_w_need_sim")]
    pub need_sim: f64,
    /// recency 衰减时间常数（小时），τ=24 时 24h 后 R≈0.368
    #[serde(default = "default_w_tau")]
    pub recency_tau_hours: f64,
    /// 最终融合分数的下限阈值：低于此值的命中会被过滤，避免噪声记忆污染 prompt 上下文。
    /// 默认 0.15；设为 0 等价于不启用过滤。
    #[serde(default = "default_w_min_score")]
    pub min_score: f64,
}

impl Default for RetrievalWeightsConfig {
    fn default() -> Self {
        Self {
            recency: default_w_recency(),
            relevance: default_w_relevance(),
            importance: default_w_importance(),
            hook_boost: default_w_hook_boost(),
            need_sim: default_w_need_sim(),
            recency_tau_hours: default_w_tau(),
            min_score: default_w_min_score(),
        }
    }
}

fn default_w_recency() -> f64 {
    0.25
}
fn default_w_relevance() -> f64 {
    0.40
}
fn default_w_importance() -> f64 {
    0.15
}
fn default_w_hook_boost() -> f64 {
    0.10
}
fn default_w_need_sim() -> f64 {
    0.10
}
fn default_w_tau() -> f64 {
    24.0
}
fn default_w_min_score() -> f64 {
    0.15
}

/// 嵌入服务配置
///
/// 独立于 `routing_matrix.memory`（后者用于 LLM 任务），专门控制向量检索的嵌入服务。
/// 启用后，向量索引与查询将使用远程 OpenAI 兼容接口；否则回退到 256 维哈希嵌入。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// 是否启用远程嵌入服务（false 时使用本地哈希嵌入）
    #[serde(default)]
    pub enabled: bool,
    /// API Key
    #[serde(default)]
    pub api_key: String,
    /// 接口端点（OpenAI 兼容，如 `https://api.siliconflow.cn/v1`）
    #[serde(default)]
    pub endpoint: String,
    /// 模型名称（如 `text-embedding-3-small` / `BAAI/bge-large-zh-v1.5`）
    #[serde(default = "default_embedding_model")]
    pub model: String,
    /// 向量维度（须与所选模型一致；用于检测维度变更并清空旧索引）
    #[serde(default = "default_embedding_dim")]
    pub dimension: usize,
    /// 嵌入来源: "cloud"（远程 API）| "local"（本地 Ollama）
    #[serde(default = "default_embedding_source")]
    pub source: String,
    /// 应用启动时自动启动 Ollama（仅 source="local" 时生效）
    #[serde(default)]
    pub ollama_auto_start: bool,
    /// ollama.exe 可执行文件路径
    #[serde(default = "default_ollama_path")]
    pub ollama_path: String,
    /// 本地 Ollama 嵌入模型名（如 bge-m3、nomic-embed-text）
    #[serde(default = "default_ollama_model")]
    pub ollama_model: String,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: String::new(),
            endpoint: String::new(),
            model: default_embedding_model(),
            dimension: default_embedding_dim(),
            source: default_embedding_source(),
            ollama_auto_start: false,
            ollama_path: default_ollama_path(),
            ollama_model: default_ollama_model(),
        }
    }
}

fn default_embedding_model() -> String {
    "BAAI/bge-m3".to_string()
}

fn default_embedding_dim() -> usize {
    1024
}

fn default_embedding_source() -> String {
    "cloud".to_string()
}

fn default_ollama_path() -> String {
    "G:\\ollama\\ollama.exe".to_string()
}

fn default_ollama_model() -> String {
    "bge-m3".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechRecognitionConfig {
    pub engine: String,
    pub language: String,
    pub silence_timeout_ms: u64,
    /// Whisper 后端专属配置（engine=whisper 时使用）
    #[serde(default)]
    pub whisper: crate::speech::WhisperConfig,
    /// Azure Speech 后端专属配置（engine=azure 时使用）
    #[serde(default)]
    pub azure: crate::speech::AzureSpeechConfig,
    /// 阿里云 NLS 后端专属配置（engine=aliyun 时使用）
    #[serde(default)]
    pub aliyun: crate::speech::AliyunAsrConfig,
}

/// 豆包端到端实时语音大模型配置（SC2.0）
///
/// 独立通话模式，绕过 ASR/LLM/TTS 三层 pipeline，走全双工 WebSocket。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealtimeVoiceConfig {
    /// 是否启用实时语音通话功能
    #[serde(default)]
    pub enabled: bool,
    /// 实时语音通话供应商：doubao / gpt_live
    #[serde(default = "default_realtime_provider")]
    pub provider: String,
    /// 火山引擎控制台 App ID
    #[serde(default)]
    pub app_id: String,
    /// 火山引擎控制台 Access Key
    #[serde(default)]
    pub access_key: String,
    /// 模型版本：O 或 SC（默认 SC）
    #[serde(default = "default_realtime_model")]
    pub model: String,
    /// SC 版本角色描述（character_manifest），用于塑造 AI 人设
    #[serde(default = "default_character_manifest")]
    pub character_manifest: String,
    /// O 版本人设字段
    #[serde(default)]
    pub bot_name: String,
    #[serde(default)]
    pub system_role: String,
    #[serde(default)]
    pub speaking_style: String,
    /// 发音人音色 ID（SC 版本用 ICL_ 开头克隆音色，O 版本用精品音色）
    #[serde(default = "default_realtime_speaker")]
    pub speaker: String,
    /// 判断用户停止说话的时间（毫秒），默认 1500，范围 [500, 50000]
    #[serde(default = "default_end_smooth_window_ms")]
    pub end_smooth_window_ms: u64,
    /// 是否开启自定义 VAD
    #[serde(default)]
    pub enable_custom_vad: bool,
    /// 是否开启非流式模型识别能力
    #[serde(default)]
    pub enable_asr_twopass: bool,
    /// 输入模式：audio（麦克风）/ text（纯文本）/ audio_file（录音文件）
    #[serde(default = "default_realtime_input_mod")]
    pub input_mod: String,
    /// 是否严格审核
    #[serde(default = "default_true")]
    pub strict_audit: bool,
    /// 命中审核后的自定义回复话术
    #[serde(default)]
    pub audit_response: String,
    /// 用户位置信息（可选，提升联网搜索精度）
    #[serde(default)]
    pub location: Option<RealtimeLocation>,
    /// 上次通话的 dialog_id（用于恢复最近20轮对话上下文）
    #[serde(default)]
    pub dialog_id: String,
    /// 是否启用回声抑制（AI 说话时静音麦克风采集，防止 AI 听到自己的声音）
    #[serde(default = "default_true")]
    pub echo_suppression: bool,
    /// 回声抑制释放尾长（毫秒）：AI 音频结束后继续静音麦克风的时间
    #[serde(default = "default_echo_release_ms")]
    pub echo_release_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealtimeLocation {
    pub longitude: f64,
    pub latitude: f64,
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub province: String,
    #[serde(default)]
    pub district: String,
    #[serde(default)]
    pub country_code: String,
}

fn default_realtime_model() -> String {
    "SC".to_string()
}

fn default_realtime_provider() -> String {
    "doubao".to_string()
}

fn default_character_manifest() -> String {
    "你是薇薇安，一个温柔体贴的 AI 伙伴。".to_string()
}

fn default_realtime_speaker() -> String {
    "ICL_zh_female_wenrouwenya_tob".to_string()
}

fn default_end_smooth_window_ms() -> u64 {
    1500
}

fn default_echo_release_ms() -> u64 {
    500
}

fn default_realtime_input_mod() -> String {
    "audio".to_string()
}

impl Default for RealtimeVoiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_realtime_provider(),
            app_id: String::new(),
            access_key: String::new(),
            model: default_realtime_model(),
            character_manifest: default_character_manifest(),
            bot_name: String::new(),
            system_role: String::new(),
            speaking_style: String::new(),
            speaker: default_realtime_speaker(),
            end_smooth_window_ms: default_end_smooth_window_ms(),
            enable_custom_vad: false,
            enable_asr_twopass: false,
            input_mod: default_realtime_input_mod(),
            strict_audit: true,
            audit_response: String::new(),
            location: None,
            dialog_id: String::new(),
            echo_suppression: true,
            echo_release_ms: default_echo_release_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactiveConfig {
    pub enabled: bool,
    pub tick_interval: u64,
    pub idle_threshold: u64,
    pub min_trigger_interval: u64,
    pub proactivity: f64,
    pub enable_idle_trigger: bool,
    pub enable_window_change_trigger: bool,
    pub enable_away_reminder: bool,
    /// 用户离开设备的空闲阈值（秒）。超过该值视为用户不在设备前，
    /// 主动对话暂停触发，等待用户回归后由 WelcomeBack 接续。
    #[serde(default = "default_away_threshold_seconds")]
    pub away_threshold_seconds: u64,
    /// 自适应 tick 间隔：根据用户空闲时间动态调整 tick 频率
    /// （空闲越久 tick 越慢，减少空转 IPC），用户交互立即重置到活跃档。
    #[serde(default = "default_true")]
    pub adaptive_tick_enabled: bool,
}

fn default_away_threshold_seconds() -> u64 {
    600
}

impl Default for ProactiveConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            tick_interval: 10,
            idle_threshold: 300,
            min_trigger_interval: 180,
            proactivity: 0.5,
            enable_idle_trigger: true,
            enable_window_change_trigger: false,
            enable_away_reminder: true,
            away_threshold_seconds: default_away_threshold_seconds(),
            adaptive_tick_enabled: true,
        }
    }
}

/// 内联表情/动作标签配置。
///
/// 注意：默认关闭。内联模式会在主 prompt 中注入标签格式说明 + 表情/动作列表，
/// 稀释主 LLM 注意力，影响对话沉浸感。默认使用独立后处理 LLM（ExpressionMotionRunnable）
/// 在文本生成完成后选择表情/动作/贴纸，主 prompt 保持干净。
///
/// 启用后，LLM 在流式输出文本中直接嵌入 `<e name="happy"/>` `<m name="wave"/>` 等标签，
/// 流式扫描器实时剥离并 emit `chat:inline_meta` 事件，
/// 消除 ExpressionMotionRunnable 的第二次 LLM 调用延迟（500ms-2s）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineExpressionConfig {
    /// 是否启用内联标签模式（true = 内联模式，false = 独立后处理 LLM 模式）
    #[serde(default)]
    pub enabled: bool,
}

impl Default for InlineExpressionConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            base: BaseConfig {
                language: "zh-CN".to_string(),
                theme: default_theme(),
                shortcut: default_shortcut(),
                shortcut_nana: default_shortcut_nana(),
                shortcut_broadcast: default_shortcut_broadcast(),
                user_avatar_path: None,
            },
            window: WindowConfig {
                smart_positioning_enabled: true,
            },
            live2d_render: Live2dRenderConfig {
                blink_interval: 4000,
            },
            ai: AiConfig {
                provider: "openai".to_string(),
                model: "deepseek-ai/DeepSeek-V3.1".to_string(),
                api_key: None,
                endpoint: Some("https://api.siliconflow.cn/v1".to_string()),
                temperature: 0.7,
                max_tokens: 2000,
                api_secret: None,
                app_id: None,
                enable_native_function_calling: true,
                enable_vision: false,
                image_detail: default_image_detail(),
            },
            network: NetworkConfig {
                proxy_mode: "direct".to_string(),
                proxy_url: String::new(),
                timeout: 30.0,
            },
            routing_matrix: {
                let mut m = HashMap::new();
                // 日常对话：核心对话任务，DeepSeek-V3.1 中文对话能力强、速度快、价格低
                m.insert(
                    "chat".to_string(),
                    TaskRouteConfig {
                        provider_type: "openai".to_string(),
                        model: "deepseek-ai/DeepSeek-V3.1".to_string(),
                        api_key: String::new(),
                        endpoint: "https://api.siliconflow.cn/v1".to_string(),
                        api_secret: String::new(),
                        app_id: String::new(),
                    },
                );
                // 深度推理：长文本/工具调用/复杂问题，自动从chat升级
                m.insert(
                    "reasoning".to_string(),
                    TaskRouteConfig {
                        provider_type: "openai".to_string(),
                        model: "deepseek-ai/DeepSeek-V3.1".to_string(),
                        api_key: String::new(),
                        endpoint: "https://api.siliconflow.cn/v1".to_string(),
                        api_secret: String::new(),
                        app_id: String::new(),
                    },
                );
                // 智能日记：每天总结当天互动，第一人称叙事
                m.insert(
                    "diary".to_string(),
                    TaskRouteConfig {
                        provider_type: "openai".to_string(),
                        model: "deepseek-ai/DeepSeek-V3.1".to_string(),
                        api_key: String::new(),
                        endpoint: "https://api.siliconflow.cn/v1".to_string(),
                        api_secret: String::new(),
                        app_id: String::new(),
                    },
                );
                // 记忆抽取：高频后台，关键词/重要性/标签抽取
                m.insert(
                    "memory".to_string(),
                    TaskRouteConfig {
                        provider_type: "openai".to_string(),
                        model: "deepseek-ai/DeepSeek-V3.1".to_string(),
                        api_key: String::new(),
                        endpoint: "https://api.siliconflow.cn/v1".to_string(),
                        api_secret: String::new(),
                        app_id: String::new(),
                    },
                );
                // 记忆巩固：夜间离线整理，短期→长期摘要、画像抽取、洞察生成
                m.insert(
                    "consolidation".to_string(),
                    TaskRouteConfig {
                        provider_type: "openai".to_string(),
                        model: "deepseek-ai/DeepSeek-V3.1".to_string(),
                        api_key: String::new(),
                        endpoint: "https://api.siliconflow.cn/v1".to_string(),
                        api_secret: String::new(),
                        app_id: String::new(),
                    },
                );
                // 内心独白：用户离开后自主思考，自然口语化，约1小时1次
                m.insert(
                    "inner_monologue".to_string(),
                    TaskRouteConfig {
                        provider_type: "openai".to_string(),
                        model: "deepseek-ai/DeepSeek-V3.1".to_string(),
                        api_key: String::new(),
                        endpoint: "https://api.siliconflow.cn/v1".to_string(),
                        api_secret: String::new(),
                        app_id: String::new(),
                    },
                );
                // 活动提取：极高频，每轮对话后分类当前活动类型
                m.insert(
                    "activity_extraction".to_string(),
                    TaskRouteConfig {
                        provider_type: "openai".to_string(),
                        model: "deepseek-ai/DeepSeek-V3.1".to_string(),
                        api_key: String::new(),
                        endpoint: "https://api.siliconflow.cn/v1".to_string(),
                        api_secret: String::new(),
                        app_id: String::new(),
                    },
                );
                // 情绪分析：极高频，每轮对话后情绪效价/唤醒度打分
                m.insert(
                    "emotion_analysis".to_string(),
                    TaskRouteConfig {
                        provider_type: "openai".to_string(),
                        model: "deepseek-ai/DeepSeek-V3.1".to_string(),
                        api_key: String::new(),
                        endpoint: "https://api.siliconflow.cn/v1".to_string(),
                        api_secret: String::new(),
                        app_id: String::new(),
                    },
                );
                // 图片理解：用户发图片时描述内容，使用Qwen2.5-VL多模态模型
                m.insert(
                    "vision_describe".to_string(),
                    TaskRouteConfig {
                        provider_type: "openai".to_string(),
                        model: "Qwen/Qwen2.5-VL-72B-Instruct".to_string(),
                        api_key: String::new(),
                        endpoint: "https://api.siliconflow.cn/v1".to_string(),
                        api_secret: String::new(),
                        app_id: String::new(),
                    },
                );
                // 知识采集：空闲时后台搜索学习，低频
                m.insert(
                    "knowledge_acquisition".to_string(),
                    TaskRouteConfig {
                        provider_type: "openai".to_string(),
                        model: "deepseek-ai/DeepSeek-V3.1".to_string(),
                        api_key: String::new(),
                        endpoint: "https://api.siliconflow.cn/v1".to_string(),
                        api_secret: String::new(),
                        app_id: String::new(),
                    },
                );
                // 兴趣搜索：内心独白中联网搜索兴趣话题，低频
                m.insert(
                    "interest_search".to_string(),
                    TaskRouteConfig {
                        provider_type: "openai".to_string(),
                        model: "deepseek-ai/DeepSeek-V3.1".to_string(),
                        api_key: String::new(),
                        endpoint: "https://api.siliconflow.cn/v1".to_string(),
                        api_secret: String::new(),
                        app_id: String::new(),
                    },
                );
                // 翻译服务：跨语言 TTS 时将文本从显示语言翻译为 TTS 语言
                m.insert(
                    "translation".to_string(),
                    TaskRouteConfig {
                        provider_type: "openai".to_string(),
                        model: "deepseek-ai/DeepSeek-V3.1".to_string(),
                        api_key: String::new(),
                        endpoint: "https://api.siliconflow.cn/v1".to_string(),
                        api_secret: String::new(),
                        app_id: String::new(),
                    },
                );
                m
            },
            memory: MemoryConfig {
                max_short_term_memory: 20,
                retrieval_strategy: "auto".to_string(),
                enable_expiration: true,
                embedding: EmbeddingConfig::default(),
                consolidation: ConsolidationConfig::default(),
                retrieval_weights: RetrievalWeightsConfig::default(),
            },
            speech_recognition: SpeechRecognitionConfig {
                engine: "winrt".to_string(),
                language: "zh-CN".to_string(),
                silence_timeout_ms: 1500,
                whisper: crate::speech::WhisperConfig::default(),
                azure: crate::speech::AzureSpeechConfig::default(),
                aliyun: crate::speech::AliyunAsrConfig::default(),
            },
            proactive: ProactiveConfig::default(),
            enable_routing_matrix: true,
            tools: ToolConfig::default(),
            world: WorldConfig::default(),
            provider_cache: HashMap::new(),
            realtime_voice: RealtimeVoiceConfig::default(),
            characters: CharactersConfig::default(),
            web_search: WebSearchConfig::default(),
            inline_expression: InlineExpressionConfig::default(),
        }
    }
}

#[derive(Clone)]
pub struct ConfigManager {
    config: Arc<RwLock<AppConfig>>,
    config_file: PathBuf,
}

impl ConfigManager {
    pub fn new() -> Self {
        let app_dir = path::get_user_data_dir();
        let config_file = app_dir.join("config.yaml");

        let config = if config_file.exists() {
            Self::load_from_file(&config_file).unwrap_or_else(|e| {
                tracing::warn!("加载配置失败: {}, 使用默认配置", e);
                AppConfig::default()
            })
        } else {
            // 首次启动：根据系统语言设置默认界面语言，避免用户来不及改设置就触发英文问候
            let mut default = AppConfig::default();
            default.base.language = detect_default_language();
            tracing::info!("首次启动，检测到系统语言并设置为默认界面语言: {}", default.base.language);
            let _ = Self::save_to_file(&config_file, &default);
            default
        };

        Self {
            config: Arc::new(RwLock::new(config)),
            config_file,
        }
    }

    pub fn get(&self, key: &str) -> Value {
        let config = self.config.read();
        let value = serde_json::to_value(&*config).unwrap_or(Value::Null);
        Self::get_nested(&value, key)
    }

    pub fn get_typed<T: serde::de::DeserializeOwned>(&self, key: &str, default: T) -> T {
        let value = self.get(key);
        serde_json::from_value(value).unwrap_or(default)
    }

    pub fn set(&self, key: &str, value: Value) -> VivianResult<()> {
        self.set_no_save(key, value)?;
        self.save()
    }

    /// 修改内存中的配置但不写入磁盘 —— 用于批量修改后统一保存
    pub fn set_no_save(&self, key: &str, value: Value) -> VivianResult<()> {
        let mut config = self.config.write();
        let mut config_value = serde_json::to_value(&*config)
            .map_err(|e| VivianError::Serialization(e.to_string()))?;
        Self::set_nested(&mut config_value, key, value)?;
        *config = serde_json::from_value(config_value)
            .map_err(|e| VivianError::Serialization(e.to_string()))?;
        Ok(())
    }

    /// 获取完整配置的深拷贝。
    ///
    /// 调用方较多（23+ 处），多数为命令处理或后台任务的一次性读取，不在每条消息的热路径上，
    /// 因此保留整份 clone 可接受。若未来出现高频热路径调用，应改为按字段访问的方法
    /// （如 `get_field<T>(&self, key: &str) -> T`），避免整份 AppConfig 克隆。
    pub fn get_all(&self) -> AppConfig {
        self.config.read().clone()
    }

    pub fn save(&self) -> VivianResult<()> {
        Self::save_to_file(&self.config_file, &*self.config.read())
    }

    pub fn reload(&self) -> VivianResult<()> {
        let config = Self::load_from_file(&self.config_file)?;
        *self.config.write() = config;
        Ok(())
    }

    fn load_from_file(path: &Path) -> VivianResult<AppConfig> {
        let content = fs::read_to_string(path)?;
        let content = Self::replace_env_vars(&content);
        let mut config: AppConfig = serde_yaml::from_str(&content)
            .map_err(|e| VivianError::Config(format!("YAML 解析失败: {}", e)))?;

        // ── 配置迁移：ai.enable_native_function_calling → tools.enable_native_function_calling ──
        // 旧版本中开关存于 ai 下；新版本统一到 tools 下。
        // 若用户旧配置 ai.enable_native_function_calling=false 且 tools 用了默认值 true，
        // 则继承旧值 false，避免升级后行为变化。
        if !config.ai.enable_native_function_calling
            && config.tools.enable_native_function_calling
        {
            config.tools.enable_native_function_calling = false;
        }

        // ── 配置迁移：web_search.provider (String) → web_search.providers (Vec<String>) ──
        // 旧版本中只支持单一引擎选择（provider 字段）；新版本改为多引擎混用（providers 列表）。
        // 若旧配置中 provider 有值且 providers 仍是默认值（仅 duckduckgo），
        // 则将旧 provider 合并进 providers，保留用户原选择。
        if !config.web_search.provider.is_empty() {
            let old_provider = config.web_search.provider.clone();
            let providers = &mut config.web_search.providers;
            // 去重插入旧 provider
            if !providers.iter().any(|p| p == &old_provider) {
                providers.insert(0, old_provider);
            }
            // 清空旧字段，避免下次再迁移（保存后 yaml 中 provider 变为空字符串）
            config.web_search.provider.clear();
        }
        // 兜底：providers 为空时回退到 duckduckgo
        if config.web_search.providers.is_empty() {
            config.web_search.providers = default_web_search_providers();
        }

        Ok(config)
    }

    fn save_to_file(path: &Path, config: &AppConfig) -> VivianResult<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                VivianError::Io(std::io::Error::new(
                    e.kind(),
                    format!("创建配置目录失败 [{}]: {}", parent.display(), e),
                ))
            })?;
        }
        let yaml = serde_yaml::to_string(config)
            .map_err(|e| VivianError::Serialization(e.to_string()))?;
        fs::write(path, &yaml).map_err(|e| {
            VivianError::Io(std::io::Error::new(
                e.kind(),
                format!("写入配置文件失败 [{}]: {}", path.display(), e),
            ))
        })?;
        // 配置文件含 API key 等敏感信息，Unix 下收紧权限为 0o600（仅属主可读写）
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    fn replace_env_vars(content: &str) -> String {
        regex::Regex::new(r"\$\{([^}]+)\}")
            .map(|re| {
                re.replace_all(content, |caps: &regex::Captures| {
                    std::env::var(&caps[1]).unwrap_or_else(|_| caps[0].to_string())
                })
                .to_string()
            })
            .unwrap_or_else(|_| content.to_string())
    }

    fn get_nested(value: &Value, key: &str) -> Value {
        let mut current = value.clone();
        for part in key.split('.') {
            current = current.get(part).cloned().unwrap_or(Value::Null);
        }
        current
    }

    fn set_nested(value: &mut Value, key: &str, new_value: Value) -> VivianResult<()> {
        let parts: Vec<&str> = key.split('.').collect();
        let mut current = value;
        for part in &parts[..parts.len() - 1] {
            if !current.is_object() {
                *current = Value::Object(serde_json::Map::new());
            }
            current = current
                .as_object_mut()
                .unwrap()
                .entry(part.to_string())
                .or_insert(Value::Object(serde_json::Map::new()));
        }
        if let Some(obj) = current.as_object_mut() {
            obj.insert(parts.last().unwrap().to_string(), new_value);
        }
        Ok(())
    }
}

impl Default for ConfigManager {
    fn default() -> Self {
        Self::new()
    }
}
