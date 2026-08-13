use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::config::manager::{AppConfig, ProviderConfig, TaskRouteConfig};
use crate::error::VivianResult;
use crate::network::proxy::{
    build_client_with_proxy, is_domestic_endpoint, ProxyConfig, ProxyMode,
};
use crate::pipeline::prompt_modules;
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::base::BaseProvider;
use crate::providers::chat_completions::ChatCompletionsProvider;
use crate::providers::doubao::DoubaoProvider;
use crate::providers::gemini::GeminiProvider;
use crate::providers::openai_compat::{CacheStrategy, OpenAiCompatProvider};
use crate::providers::openai_responses::OpenAiResponsesProvider;
use crate::providers::spark::SparkProvider;
use crate::providers::wenxin::WenxinProvider;
use crate::providers::zhipu::ZhipuProvider;

/// 客户端缓存类型
///
/// key = `{base_url}|{proxy_url}|{timeout}`，value = 带（或不带）代理的 reqwest 客户端。
/// 用于在配置热重载前复用已建立的连接池，避免重复构建客户端。
pub type ClientCache = Arc<RwLock<HashMap<String, Arc<reqwest::Client>>>>;

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
        let lower = s.to_lowercase();
        match lower.as_str() {
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
            _ => ProviderKind::Custom,
        }
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

    let client: reqwest::Client = {
        let cached = {
            let cache = client_cache.read();
            cache.get(&cache_key).cloned()
        };
        match cached {
            Some(c) => (*c).clone(),
            None => match build_client_with_proxy(&proxy_config) {
                Ok(c) => {
                    client_cache
                        .write()
                        .insert(cache_key, Arc::new(c.clone()));
                    c
                }
                Err(_) => crate::network::http_client::get_global_client(),
            },
        }
    };

    let kind = ProviderKind::from_str(&task_config.provider_type);
    let provider_config = ProviderConfig {
        base_url: task_config.endpoint.clone(),
        api_key: task_config.api_key.clone(),
        model: task_config.model.clone(),
    };

    create_provider_by_kind(
        kind,
        &provider_config,
        &task_config.api_secret,
        &task_config.app_id,
        temperature,
        max_tokens,
        effective_proxy_url,
        Some(client),
        CacheStrategy::from_str(&config.tools.cache_strategy),
        &config.base.language,
    )
}

/// 按协议类型分发到具体 Provider 实现
#[allow(clippy::too_many_arguments)]
fn create_provider_by_kind(
    kind: ProviderKind,
    provider_config: &ProviderConfig,
    api_secret: &str,
    app_id: &str,
    temperature: f64,
    max_tokens: u32,
    effective_proxy_url: Option<String>,
    client: Option<reqwest::Client>,
    cache_strategy: CacheStrategy,
    lang: &str,
) -> VivianResult<Box<dyn BaseProvider>> {
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
            .with_instructions(Some(prompt_modules::build_instructions(lang)));
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
            .with_instructions(Some(prompt_modules::build_instructions(lang)));
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
            .with_instructions(Some(prompt_modules::build_instructions(lang)));
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
            .with_instructions(Some(prompt_modules::build_instructions(lang)));
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
            .with_instructions(Some(prompt_modules::build_instructions(lang)));
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
            .with_instructions(Some(prompt_modules::build_instructions(lang)));
            Ok(Box::new(provider))
        }
    }
}
