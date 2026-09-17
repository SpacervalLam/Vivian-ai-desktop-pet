use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::config::manager::{AppConfig, ProviderConfig, TaskRouteConfig};
use crate::error::{VivianError, VivianResult};
use crate::network::proxy::{
    build_client_with_proxy, is_domestic_endpoint, ProxyConfig, ProxyMode,
};
use crate::pipeline::prompt_modules;
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::base::BaseProvider;
use crate::providers::chat_completions::ChatCompletionsProvider;
use crate::providers::declarative::DeclarativeProvider;
use crate::providers::doubao::DoubaoProvider;
use crate::providers::gemini::GeminiProvider;
use crate::providers::openai_compat::{CacheStrategy, OpenAiCompatProvider};
use crate::providers::openai_responses::OpenAiResponsesProvider;
use crate::providers::protocol_registry;
use crate::providers::spec::CompiledSpec;
use crate::providers::spark::SparkProvider;
use crate::providers::wenxin::WenxinProvider;
use crate::providers::zhipu::ZhipuProvider;

/// 原生 adapter 覆盖的 provider_type。这些类型始终走专用 native 实现
/// （鲁棒性更全：重试/缓存/工具等），**不**被声明式协议注册表接管。
///
/// 全新协议（插件带来的新 provider_type，如 `interactions`）不在本集合内，
/// 命中注册表时路由到 `DeclarativeProvider`（通用解释器）。
const NATIVE_PROVIDER_TYPES: &[&str] = &[
    "openai",
    "openai_responses",
    "doubao",
    "gemini",
    "anthropic",
    "wenxin",
    "spark",
    "chat_completions",
    "zhipu",
    "custom",
];

/// 声明式协议路由：非原生类型查协议注册表。
///
/// 优先级：JS 插件动态贡献 > 插件 `protocols/*.json` 文件 > 内置兜底
/// （protocol_registry::spec_for 统一裁决）。协议 JSON / JS 贡献均属数据面，
/// 运行时装载插件后由 js_host 触发 invalidate 重载。
fn declarative_spec_for(provider_type: &str) -> Option<Arc<CompiledSpec>> {
    // 原生类型交给专用 adapter（不做声明式接管——重试/缓存/工具链更全）
    if NATIVE_PROVIDER_TYPES.contains(&provider_type) {
        return None;
    }
    protocol_registry::spec_for(provider_type)
}

/// 失效声明式协议注册表缓存（下次创建 provider 时重新读盘装载）。
pub fn invalidate_declarative_registry() {
    protocol_registry::invalidate();
}

/// 客户端缓存类型
///
/// key = `{base_url}|{proxy_url}|{timeout}`，value = 带（或不带）代理的 reqwest 客户端。
/// 用于在配置热重载前复用已建立的连接池，避免重复构建客户端。
pub type ClientCache = Arc<RwLock<HashMap<String, Arc<reqwest::Client>>>>;

fn cached_http_client(
    client_cache: &ClientCache,
    cache_key: String,
    proxy_config: &ProxyConfig,
) -> VivianResult<reqwest::Client> {
    if let Some(client) = client_cache.read().get(&cache_key).cloned() {
        return Ok((*client).clone());
    }
    // 构造无需 await；持有写锁完成二次检查和插入，避免并发 miss 重复创建连接池。
    let mut cache = client_cache.write();
    if let Some(client) = cache.get(&cache_key).cloned() {
        return Ok((*client).clone());
    }
    let client = build_client_with_proxy(proxy_config).map_err(|error| {
        VivianError::Provider(format!("创建代理 HTTP 客户端失败: {}", error))
    })?;
    cache.insert(cache_key, Arc::new(client.clone()));
    Ok(client)
}

/// 工作智能体（编程）模型的默认输出预算（token）。
///
/// 聊天主配置的 `ai.max_tokens`（默认 2048）是为日常对话设计的预算，
/// 对生成代码 / 长 diff / 多轮工具调用序列过小。编程模型不要求用户配置
/// max_tokens，由后端按服务商分级给予默认值。
///
/// 分级依据：各家「单次输出上限」（≠ 上下文窗口——1M 上下文是输入+输出合计，
/// 单次生成上限通常远小于此）：
/// - Claude（anthropic）：64000；Gemini：65536；OpenAI：32768；智谱 GLM：65536
/// - DeepSeek V4：16384（V4 已移除 V3/R1 时代的 8K 硬上限，输出可达 384K）；Kimi：32768；豆包：65536
/// - Groq / Together / OpenRouter：8192（Llama 系模型上限）
/// - 未知端点 / 本地服务：保守 8192，避免超过模型输出上限被服务商拒绝（400）
pub fn work_model_default_max_tokens(provider_type: &str, endpoint: &str) -> u32 {
    let e = endpoint.to_lowercase();
    // 1. 已知端点硬上限优先（同一 provider 类型下按端点区分厂商）
    if e.contains("api.deepseek.com") {
        return 16384; // DeepSeek V4（V3/R1 时代的 8K 硬上限已随 V4 移除）
    }
    if e.contains("generativelanguage.googleapis.com") {
        return 65536; // Gemini 2.5 / 3.x
    }
    if e.contains("dashscope") || e.contains("aliyuncs.com") {
        return 32768; // 通义千问 DashScope（Qwen3）
    }
    if e.contains("open.bigmodel.cn") {
        return 65536; // 智谱 GLM-5.x（最大输出 128K）
    }
    if e.contains("api.x.ai") {
        return 32768; // xAI Grok
    }
    if e.contains("api.moonshot.cn") {
        return 32768; // Moonshot Kimi（K2.x 默认 max_tokens 32768）
    }
    if e.contains("ark.cn-beijing.volces.com") {
        return 65536; // 火山方舟豆包（Seed 最大输出 256K）
    }
    if e.contains("api.siliconflow.cn") {
        return 8192; // 硅基流动（托管 DeepSeek-V3 等 8K 上限模型）
    }
    if e.contains("api.groq.com") {
        return 8192; // Groq Llama 系模型输出上限 ≤8K
    }
    if e.contains("openrouter.ai") {
        return 8192; // OpenRouter 代理，按模型上限校验
    }
    if e.contains("api.together.xyz") {
        return 8192; // Together Llama 3.3 70B 上限 8K
    }
    if e.contains("api.mistral.ai") {
        return 16384;
    }
    if e.contains("aip.baidubce.com") {
        return 8192; // 文心 ernie-4.5-8k
    }
    if e.contains("api.hunyuan.cloud.tencent.com") {
        return 16384; // 腾讯混元 hunyuan-turbos（16k）/ hy3（128k）
    }
    if e.contains("spark-api.xf-yun.com") || e.contains("xfyun") {
        return 8192; // 讯飞星火
    }
    if e.contains("localhost") || e.contains("127.0.0.1") || e.contains("11434") {
        return 8192; // Ollama 等本地服务
    }
    // 2. provider 类型兜底
    match provider_type.to_lowercase().as_str() {
        "anthropic" | "claude" => 64000,
        "gemini" | "google" => 65536,
        "openai" | "openai_compat" | "openai-compat" | "openai_responses" | "responses_api" => 32768,
        "zhipu" | "glm" | "chatglm" | "bigmodel" => 32768,
        "doubao" | "doubao_responses" => 16384,
        "wenxin" | "ernie" | "baidu" => 8192,
        "spark" | "xfyun" | "iflytek" => 8192,
        _ => 8192, // chat_completions / custom / 未知类型：保守
    }
}

/// Provider 协议类型
///
/// 替代旧的"contains gemini"字符串启发式判断，按显式类型分发到对应实现：
/// - `OpenAiCompat`：OpenAI Responses API 兼容接口（DeepSeek / Qwen / Moonshot / SiliconFlow / Doubao / GLM 等）
/// - `OpenAiResponses`：OpenAI 官方 Responses API（`/v1/responses`），原生支持 MCP/Tool Calling/多模态，适用于 GPT-4o / o1 / o3 系列
/// - `DoubaoResponses`：火山方舟豆包 Responses API（`/api/v3/responses`），仅支持 250615+ 新模型
/// - `Gemini`：Google Gemini 原生 REST API（含 Google Search grounding）
/// - `Anthropic`：Anthropic Claude 原生 /v1/messages（x-api-key + anthropic-version）
/// - `Wenxin`：百度文心一言原生 OAuth + access_token 鉴权
/// - `Spark`：讯飞星火 WebSocket + HMAC-SHA256 签名
/// - `ChatCompletions`：标准 OpenAI Chat Completions API（`/v1/chat/completions`），适用于 OpenRouter / Groq / Mistral / Together / Ollama / vLLM / LM Studio
/// - `Custom`：用户自定义协议（按 Chat Completions 处理）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAiCompat,
    OpenAiResponses,
    DoubaoResponses,
    Gemini,
    Anthropic,
    Wenxin,
    Spark,
    ChatCompletions,
    Zhipu,
    Custom,
}

impl ProviderKind {
    /// 从字符串解析 ProviderKind（大小写不敏感）
    ///
    /// 兼容旧配置：`openai` / `gemini` / `anthropic` / `wenxin` / `spark` / `custom`
    /// `doubao` / `doubao_responses` 走火山方舟 Responses API 专用路径。
    /// `openai_responses` / `responses_api` 走 OpenAI 官方 Responses API 路径。
    /// `zhipu` / `glm` 走智谱 GLM Chat Completions 专用路径（含联网搜索）。
    /// 未知值统一回退到 `Custom`（按 OpenAI 兼容处理）。
    pub fn from_str(s: &str) -> Self {
        Self::try_from_str(s).unwrap_or(ProviderKind::Custom)
    }

    pub fn try_from_str(s: &str) -> Option<Self> {
        let lower = s.to_lowercase();
        Some(match lower.as_str() {
            "openai" | "openai_compat" | "openai-compat" => ProviderKind::OpenAiCompat,
            "openai_responses" | "openai-responses" | "responses_api" | "responses-api" => {
                ProviderKind::OpenAiResponses
            }
            "doubao" | "doubao_responses" | "doubao-responses" | "responses" => {
                ProviderKind::DoubaoResponses
            }
            "gemini" | "google" => ProviderKind::Gemini,
            "anthropic" | "claude" => ProviderKind::Anthropic,
            "wenxin" | "ernie" | "baidu" => ProviderKind::Wenxin,
            "spark" | "xfyun" | "iflytek" => ProviderKind::Spark,
            "chat_completions" | "chat-completions" | "openai_chat" | "openai-chat" => {
                ProviderKind::ChatCompletions
            }
            "zhipu" | "glm" | "chatglm" | "bigmodel" => ProviderKind::Zhipu,
            "custom" => ProviderKind::Custom,
            _ => return None,
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::OpenAiCompat => "openai",
            ProviderKind::OpenAiResponses => "openai_responses",
            ProviderKind::DoubaoResponses => "doubao",
            ProviderKind::Gemini => "gemini",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Wenxin => "wenxin",
            ProviderKind::Spark => "spark",
            ProviderKind::ChatCompletions => "chat_completions",
            ProviderKind::Zhipu => "zhipu",
            ProviderKind::Custom => "custom",
        }
    }
}

fn is_local_endpoint(endpoint: &str) -> bool {
    reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host == "127.0.0.1"
                || host == "::1"
        })
}

/// 配置是否足以创建 provider。本地 OpenAI-compatible 服务允许无 API key，
/// 声明为无鉴权的插件协议同样允许空 key。
pub fn provider_configuration_complete(config: &TaskRouteConfig) -> bool {
    if config.endpoint.trim().is_empty() || config.model.trim().is_empty() {
        return false;
    }
    if is_local_endpoint(&config.endpoint) {
        return true;
    }
    if let Some(spec) = declarative_spec_for(config.provider_type.trim()) {
        return spec.spec.auth.is_none() || !config.api_key.trim().is_empty();
    }
    !config.api_key.trim().is_empty()
}

/// 为路由矩阵中的单个任务创建独立 provider 实例
///
/// 此函数从 `TaskRouteConfig`（完整模型配置）创建 provider，使每个任务拥有独立的模型/API Key/端点。
pub fn create_task_provider(
    task_config: &TaskRouteConfig,
    config: &AppConfig,
    client_cache: &ClientCache,
) -> VivianResult<Box<dyn BaseProvider>> {
    let temperature = task_config.temperature.unwrap_or(config.ai.temperature);
    let max_tokens = task_config.max_tokens.unwrap_or(config.ai.max_tokens);

    // 按 endpoint 域名分流：国内厂商强制直连，国外厂商沿用全局代理配置
    let mut proxy_config = ProxyConfig::from_app_config(config);
    let domestic = is_domestic_endpoint(&task_config.endpoint);
    if domestic {
        proxy_config.mode = ProxyMode::Direct;
        proxy_config.url = String::new();
    }
    let effective_proxy_url = proxy_config.effective_proxy_url();
    tracing::info!(
        "[Proxy] task_provider endpoint={} domestic={} proxy={}",
        task_config.endpoint,
        domestic,
        effective_proxy_url.as_deref().unwrap_or("direct")
    );

    let cache_key = format!(
        "{}|{}|{}",
        task_config.endpoint,
        effective_proxy_url.as_deref().unwrap_or(""),
        proxy_config.timeout_secs
    );

    let client = cached_http_client(client_cache, cache_key, &proxy_config)?;

    let kind = ProviderKind::try_from_str(&task_config.provider_type).unwrap_or(ProviderKind::Custom);
    if kind == ProviderKind::Custom
        && !task_config.provider_type.eq_ignore_ascii_case("custom")
        && declarative_spec_for(task_config.provider_type.trim()).is_none()
    {
        return Err(VivianError::Provider(format!(
            "未知 provider_type: {}",
            task_config.provider_type
        )));
    }
    let provider_config = ProviderConfig {
        base_url: task_config.endpoint.clone(),
        api_key: task_config.api_key.clone(),
        model: task_config.model.clone(),
    };

    create_provider_by_kind(
        kind,
        &task_config.provider_type,
        &provider_config,
        &task_config.api_secret,
        &task_config.app_id,
        temperature,
        max_tokens,
        effective_proxy_url,
        Some(client),
        CacheStrategy::from_str(&config.tools.cache_strategy),
        &config.base.language,
        true,
    )
}

/// 为 API 可用性探测（一键检测）创建"裸" provider
///
/// 与 `create_task_provider` 走完全相同的协议分发 / 代理分流 / 客户端缓存链路，
/// 唯一区别是不注入 system instructions —— 探测请求只需验证端点可达、鉴权有效、
/// 模型存在，不携带完整人格框架提示词，最小化 token 开销。
pub fn create_probe_provider(
    task_config: &TaskRouteConfig,
    config: &AppConfig,
    client_cache: &ClientCache,
) -> VivianResult<Box<dyn BaseProvider>> {
    // 探测用最小参数：temperature=0 + 输出预算 16 token，把每次探测成本压到最低
    let probe_config = TaskRouteConfig {
        temperature: Some(0.0),
        max_tokens: Some(16),
        ..task_config.clone()
    };

    let mut proxy_config = ProxyConfig::from_app_config(config);
    if is_domestic_endpoint(&task_config.endpoint) {
        proxy_config.mode = ProxyMode::Direct;
        proxy_config.url = String::new();
    }
    let effective_proxy_url = proxy_config.effective_proxy_url();

    let cache_key = format!(
        "{}|{}|{}",
        task_config.endpoint,
        effective_proxy_url.as_deref().unwrap_or(""),
        proxy_config.timeout_secs
    );

    let client = cached_http_client(client_cache, cache_key, &proxy_config)?;

    let kind = ProviderKind::try_from_str(&task_config.provider_type).unwrap_or(ProviderKind::Custom);
    if kind == ProviderKind::Custom
        && !task_config.provider_type.eq_ignore_ascii_case("custom")
        && declarative_spec_for(task_config.provider_type.trim()).is_none()
    {
        return Err(VivianError::Provider(format!(
            "未知 provider_type: {}",
            task_config.provider_type
        )));
    }
    let provider_config = ProviderConfig {
        base_url: task_config.endpoint.clone(),
        api_key: task_config.api_key.clone(),
        model: task_config.model.clone(),
    };

    create_provider_by_kind(
        kind,
        &task_config.provider_type,
        &provider_config,
        &task_config.api_secret,
        &task_config.app_id,
        probe_config.temperature.unwrap_or(0.0),
        probe_config.max_tokens.unwrap_or(16),
        effective_proxy_url,
        Some(client),
        CacheStrategy::from_str(&config.tools.cache_strategy),
        &config.base.language,
        false,
    )
}

/// 按协议类型分发到具体 Provider 实现
///
/// `include_instructions=false` 时创建不带 system instructions 的裸实例（API 探测用）。
#[allow(clippy::too_many_arguments)]
fn create_provider_by_kind(
    kind: ProviderKind,
    provider_type_raw: &str,
    provider_config: &ProviderConfig,
    api_secret: &str,
    app_id: &str,
    temperature: f64,
    max_tokens: u32,
    effective_proxy_url: Option<String>,
    client: Option<reqwest::Client>,
    cache_strategy: CacheStrategy,
    lang: &str,
    include_instructions: bool,
) -> VivianResult<Box<dyn BaseProvider>> {
    let instructions = if include_instructions {
        Some(prompt_modules::build_instructions(lang))
    } else {
        None
    };

    // 声明式协议路径：绑定插件注册表的新协议（provider_type_raw 不在 NATIVE 集合内）
    if let Some(spec) = declarative_spec_for(provider_type_raw.trim()) {
        tracing::info!(
            "[Provider] provider_type=`{}` 走声明式协议引擎（{id}）",
            provider_type_raw,
            id = spec.spec.id
        );
        let provider = DeclarativeProvider::new(
            provider_config,
            temperature,
            max_tokens,
            effective_proxy_url,
            client,
            spec,
            api_secret.to_string(),
        )
        .with_instructions(instructions);
        return Ok(Box::new(provider));
    }

    match kind {
        ProviderKind::Gemini => {
            let provider = GeminiProvider::new(
                &provider_config.api_key,
                &provider_config.model,
                temperature,
                max_tokens,
                effective_proxy_url,
                client,
            );
            Ok(Box::new(provider))
        }
        ProviderKind::Anthropic => {
            let provider = AnthropicProvider::new(
                provider_config,
                temperature,
                max_tokens,
                effective_proxy_url,
                client,
            )
            .with_cache_strategy(cache_strategy)
            .with_instructions(instructions);
            Ok(Box::new(provider))
        }
        ProviderKind::Wenxin => {
            let provider = WenxinProvider::new(
                provider_config,
                api_secret,
                temperature,
                max_tokens,
                effective_proxy_url,
                client,
            );
            Ok(Box::new(provider))
        }
        ProviderKind::Spark => {
            let provider = SparkProvider::new(
                provider_config,
                api_secret,
                app_id,
                temperature,
                max_tokens,
                effective_proxy_url,
                client,
            );
            Ok(Box::new(provider))
        }
        ProviderKind::OpenAiCompat => {
            let provider = OpenAiCompatProvider::new(
                provider_config,
                temperature,
                max_tokens,
                effective_proxy_url,
                client,
            )
            .with_cache_strategy(cache_strategy)
            .with_instructions(instructions);
            Ok(Box::new(provider))
        }
        ProviderKind::ChatCompletions | ProviderKind::Custom => {
            let provider = ChatCompletionsProvider::new(
                provider_config,
                temperature,
                max_tokens,
                effective_proxy_url,
                client,
            )
            .with_instructions(instructions);
            Ok(Box::new(provider))
        }
        ProviderKind::Zhipu => {
            let provider = ZhipuProvider::new(
                provider_config,
                temperature,
                max_tokens,
                effective_proxy_url,
                client,
            )
            .with_instructions(instructions);
            Ok(Box::new(provider))
        }
        ProviderKind::OpenAiResponses => {
            let provider = OpenAiResponsesProvider::new(
                provider_config,
                temperature,
                max_tokens,
                effective_proxy_url,
                client,
            )
            .with_instructions(instructions);
            Ok(Box::new(provider))
        }
        ProviderKind::DoubaoResponses => {
            let provider = DoubaoProvider::new(
                provider_config,
                temperature,
                max_tokens,
                effective_proxy_url,
                client,
            )
            .with_instructions(instructions);
            Ok(Box::new(provider))
        }
    }
}
