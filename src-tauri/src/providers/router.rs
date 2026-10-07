use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio::sync::Semaphore;

use crate::config::manager::{AppConfig, TaskRouteConfig, WorkModelProfile};
use crate::error::{VivianError, VivianResult};
use crate::providers::base::TASK_WORK_AGENT;
use crate::providers::base::{
    scope_provider_call, BaseProvider, ChatResponse, LLMRequest, ProviderCallOptions, StreamEvent,
    ToolDefinition,
};
use crate::providers::factory::{
    create_task_provider, provider_configuration_complete, ClientCache,
};
use crate::providers::reasoning::ReasoningPreference;
use crate::providers::usage_store;
use crate::resilience::{classify_llm_error_from_str, error_kind_to_message_key, LlmErrorKind};
use crate::types::response::ChatMessage;

enum RouteProvider<'a> {
    Task { route: &'a str, provider: &'a Box<dyn BaseProvider> },
    Main(&'a Box<dyn BaseProvider>),
    Override(Arc<Box<dyn BaseProvider>>),
}

impl RouteProvider<'_> {
    fn provider(&self) -> &Box<dyn BaseProvider> {
        match self {
            Self::Task { provider, .. } | Self::Main(provider) => provider,
            Self::Override(provider) => provider.as_ref(),
        }
    }

    fn source(&self) -> &'static str {
        match self { Self::Task { .. } => "task", Self::Main(_) => "main", Self::Override(_) => "work_override" }
    }

    fn route(&self) -> &str {
        match self { Self::Task { route, .. } => route, Self::Main(_) => "main", Self::Override(_) => TASK_WORK_AGENT }
    }
}

/// 模型路由
///
/// 职责：
/// - 按 `routing_matrix` 将任务分发到对应 provider
/// - 每个任务拥有独立的 provider 实例（独立模型/API Key/端点）
/// - 支持 fallback：任务 provider 失败后自动尝试主 LLM API，失败即报错
/// - 主 LLM API（`config.ai`）是必须配置的；路由矩阵可选启用
/// - 路由矩阵未启用 / 任务未配置 / 任务 provider 出错时回退到主 LLM API
/// - 任务 provider 出错并回退时，通过 `chat:route_fallback` 事件通知前端
/// - 任务 provider 调用结果（成功/失败）通过 `chat:route_status` 事件通知前端，
///   用于路由矩阵 UI 中模型名颜色标记（绿色=最近成功，红色=最近失败）
/// - 支持联网搜索开关（`enable_search`，DeepSeek/GPT-4o/Gemini 三种集成）
/// - 支持代理配置（全局 `network.proxy_mode` + `network.proxy_url`，三模式 direct/system/manual）
/// - 支持客户端缓存 + 热重载（`clear_client_cache` / `reload`）
#[derive(Clone)]
pub struct ModelRouter {
    /// 主 LLM API provider —— 由 `config.ai` 构建，必须配置
    main_provider: Arc<Option<Box<dyn BaseProvider>>>,
    /// 任务专属 provider —— 每个任务独立配置的模型实例
    task_providers: Arc<HashMap<String, Box<dyn BaseProvider>>>,
    /// Native decision endpoint; Jev does not implement chat completions.
    jev_decision: Option<Arc<crate::providers::jev::JevClient>>,
    /// 工作智能体模型热切换覆盖
    ///
    /// 用户为工作智能体选择的 provider 实例。`None` 表示未覆盖。
    ///
    /// 字段名沿用历史命名，但**它已不对应 reasoning 任务**：工作智能体改用
    /// [`TASK_WORK_AGENT`] 后，匹配只认那个类型（见 `override_provider_for`）。
    /// 陪伴对话的 reasoning 碰不到这里——否则角色的每句台词都会被编程模型接管，
    /// 连带套上该 provider 的 omit_temperature，情绪驱动的温度被静默丢弃。
    reasoning_override: Arc<RwLock<Option<Arc<Box<dyn BaseProvider>>>>>,
    /// 是否启用路由矩阵（关闭时所有请求走主 LLM API）
    enable_routing_matrix: bool,
    /// 全局联网搜索开关
    enable_search: Arc<AtomicBool>,
    /// 客户端缓存
    client_cache: ClientCache,
    /// Tauri AppHandle —— 用于在路由回退时 emit `chat:route_fallback` 事件
    /// 启动时由 `lib.rs` 注入；未注入时不发事件（仅日志）
    app_handle: Arc<RwLock<Option<AppHandle>>>,
    /// 是否允许 emit 路由回退事件
    /// 仅在用户主动发消息（`send_message*`）期间为 true，避免主动对话轮询刷屏
    emit_enabled: Arc<AtomicBool>,
    /// 按任务分组的并发限制信号量
    ///
    /// 防止后处理 LLM 调用（记忆巩固 / 内心独白 / 日记等）同时挤占主对话资源。
    /// 分组规则见 `semaphore_for_task`：
    /// - chat / reasoning / vision_describe → 3 并发（用户交互路径，最高优先级）
    /// - memory / reflection / consolidation → 3 并发（记忆/反思/巩固）
    /// - emotion_analysis / inner_monologue / diary / knowledge_acquisition
    ///   / translation / bystander_judge / intent_judge / asr_polish → 2 并发（辅助后台任务）
    /// - 其他 → 3 并发（兜底）
    semaphores: Arc<HashMap<String, Arc<Semaphore>>>,
    /// LLM 错误 toast 冷却追踪：error_kind → 上次发送时间，防止同类型错误反复弹窗
    error_toast_cooldown: Arc<RwLock<HashMap<LlmErrorKind, Instant>>>,
    /// 路由回退事件冷却追踪：task_type → 上次发送时间
    ///
    /// 防止同一任务反复回退（如 inner_monologue 熔断后每次调用都回退）导致 toast 刷屏。
    /// 按 task_type 维度冷却：不同任务的回退各自独立计数。
    route_fallback_cooldown: Arc<RwLock<HashMap<String, Instant>>>,
    /// 是否走代理链路（基于 `config.network.proxy_mode` 判断，非 direct 即视为走代理）
    ///
    /// 让辅助任务能据此调整超时：代理链路通常比直连慢，需要更长等待时间。
    uses_proxy: bool,
    /// Structured Outputs strict 模式熔断标记
    ///
    /// 一旦检测到 strict schema 拒绝（API 400 + schema 相关错误），置为 true，
    /// 后续 `apply_json_schema` 降级为 None（不注入 schema，回退到 json_object / 纯文本路径）。
    /// 熔断后自动重试当前请求（不带 schema），对上层透明。
    /// 熔断持续到进程重启或 `reload`（reload 创建新 ModelRouter 实例，自然重置）。
    strict_broken: Arc<RwLock<HashSet<String>>>,
    /// 全局默认推理偏好（来自 `config.ai.reasoning`）
    ///
    /// 请求未显式设置推理偏好（AUTO）时使用；请求显式设置的偏好优先。
    default_reasoning: ReasoningPreference,
    /// 各任务路由自身的默认推理偏好。
    task_reasoning: Arc<HashMap<String, ReasoningPreference>>,
    /// 当前工作模型的默认推理偏好。
    work_reasoning: Arc<RwLock<Option<ReasoningPreference>>>,
    /// 陪伴对话默认存在惩罚（`config.ai.presence_penalty`）。
    ///
    /// 只对"角色正在说话"的任务类型注入，见 `conversational_penalties`。
    presence_penalty: f64,
    /// 陪伴对话默认频率惩罚（`config.ai.frequency_penalty`）。
    frequency_penalty: f64,
}

/// Only spoken outputs receive character repetition penalties.
fn is_conversational_task(task_type: &str) -> bool {
    super::task_catalog::find(task_type).map(|spec| spec.spoken).unwrap_or(false)
}

/// 任务 → 信号量分组的并发上限
const SEMAPHORE_GROUP_CHAT_REASONING: usize = 3;
const SEMAPHORE_GROUP_MEMORY_REFLECTION: usize = 3;
const SEMAPHORE_GROUP_AUXILIARY: usize = 2;
/// 工作智能体并发额度：单列一组，避免它长时间占用陪伴对话的额度
const SEMAPHORE_GROUP_WORK_AGENT: usize = 2;

/// LLM 错误 toast 冷却时间：同类错误在此时间内不重复弹窗
const ERROR_TOAST_COOLDOWN_SECS: u64 = 60;

/// 路由回退事件冷却时间：同一任务在此时间内的回退不重复发 toast
const ROUTE_FALLBACK_COOLDOWN_SECS: u64 = 120;

/// 解析任务类型对应的信号量分组名
///
/// 返回 (组名, 并发上限)，由 `ModelRouter::new` 在构造时据此创建 Semaphore
fn semaphore_for_task(task_type: &str) -> (&'static str, usize) {
    match super::task_catalog::find(task_type).map(|spec| spec.concurrency.as_str()) {
        Some("memory") => ("memory_reflection", SEMAPHORE_GROUP_MEMORY_REFLECTION),
        Some("work") => ("work_agent", SEMAPHORE_GROUP_WORK_AGENT),
        Some("auxiliary") => ("auxiliary", SEMAPHORE_GROUP_AUXILIARY),
        _ => ("chat_reasoning", SEMAPHORE_GROUP_CHAT_REASONING),
    }
}

/// 持久化 strict 熔断模型名：避免每次重启对不支持 schema 的模型白跑一次 400
fn strict_broken_path() -> std::path::PathBuf {
    crate::utils::path::get_user_data_dir().join("strict_broken_model")
}

fn load_strict_broken_models() -> HashSet<String> {
    let Ok(raw) = std::fs::read_to_string(strict_broken_path()) else {
        return HashSet::new();
    };
    serde_json::from_str(&raw).unwrap_or_else(|_| {
        // 兼容旧版只保存单个模型名的文件。
        let legacy = raw.trim();
        if legacy.is_empty() {
            HashSet::new()
        } else {
            HashSet::from([legacy.to_string()])
        }
    })
}

fn save_strict_broken_models(models: &HashSet<String>) {
    // Unit tests must not change the user's persistent capability cache.
    if cfg!(test) {
        return;
    }
    if let Ok(bytes) = serde_json::to_vec(models) {
        let _ = crate::utils::fs::atomic_write(&strict_broken_path(), &bytes);
    }
}

impl ModelRouter {
    pub fn new(config: &AppConfig) -> VivianResult<Self> {
        let client_cache: ClientCache = Arc::new(RwLock::new(HashMap::new()));

        // 1. 主 LLM API provider（来自 config.ai）
        //    本地或声明为无鉴权的协议允许 api_key 为空。
        let main_provider: Option<Box<dyn BaseProvider>> = {
            let api_key = config.ai.api_key.as_deref().unwrap_or("").trim();
            let endpoint = config.ai.endpoint.as_deref().unwrap_or("").trim();
            let model = config.ai.model.trim();
            let task_config = crate::config::manager::TaskRouteConfig {
                provider_type: config.ai.provider.clone(),
                model: model.to_string(),
                api_key: api_key.to_string(),
                endpoint: endpoint.to_string(),
                api_secret: config.ai.api_secret.clone().unwrap_or_default(),
                app_id: config.ai.app_id.clone().unwrap_or_default(),
                send_temperature: None,
                send_max_tokens: None,
                reasoning_overrides: config.ai.reasoning_overrides.clone(),
                temperature: None,
                max_tokens: None,
                context_window: None,
                reasoning: None,
            };
            if !provider_configuration_complete(&task_config) {
                tracing::warn!("[ModelRouter] 主 LLM API 未配置完整，跳过创建 main_provider");
                None
            } else {
                match create_task_provider(&task_config, config, &client_cache) {
                    Ok(p) => {
                        tracing::info!(
                            "[ModelRouter] 主 LLM API 已绑定: {} @ {} ({})",
                            model,
                            endpoint,
                            config.ai.provider
                        );
                        Some(p)
                    }
                    Err(e) => {
                        tracing::warn!("[ModelRouter] 创建主 LLM API provider 失败: {}", e);
                        None
                    }
                }
            }
        };

        // 2. 任务专属 provider（来自 routing_matrix）
        //    仅在 enable_routing_matrix=true 时构建；关闭时跳过，所有任务回退到主 API
        let mut task_providers: HashMap<String, Box<dyn BaseProvider>> = HashMap::new();
        let mut jev_decision = None;
        let mut task_reasoning = HashMap::new();
        if config.enable_routing_matrix {
            for (task_type, task_config) in &config.routing_matrix {
                // 跳过空配置（未填写 model 或 endpoint 的任务）→ 由主 API 兜底
                if !provider_configuration_complete(task_config) {
                    tracing::info!(
                        "[ModelRouter] 任务 {} 未配置完整，回退到主 LLM API",
                        task_type
                    );
                    continue;
                }
                if task_type == "simple_judge" && task_config.provider_type == "jev" {
                    match crate::providers::jev::JevClient::new(task_config) {
                        Ok(client) => jev_decision = Some(Arc::new(client)),
                        Err(e) => tracing::warn!("[ModelRouter] Jev route invalid: {e}"),
                    }
                    continue;
                }
                if task_config.provider_type == "jev" {
                    tracing::warn!(
                        "[ModelRouter] Jev only supports simple_judge, skipping {task_type}"
                    );
                    continue;
                }
                // reasoning（编程 / 深度推理）未显式配置 max_tokens 时按服务商分级默认，
                // 避免回退到聊天用的 2048 限制代码生成；显式配置仍优先
                let mut cfg = task_config.clone();
                if task_type == "reasoning" && cfg.max_tokens.is_none() {
                    cfg.max_tokens =
                        Some(crate::providers::factory::work_model_default_max_tokens(
                            &cfg.provider_type,
                            &cfg.endpoint,
                        ));
                }
                match create_task_provider(&cfg, config, &client_cache) {
                    Ok(provider) => {
                        tracing::info!(
                            "[ModelRouter] 任务 {} 绑定模型 {} @ {}",
                            task_type,
                            cfg.model,
                            cfg.endpoint
                        );
                        task_providers.insert(task_type.clone(), provider);
                        if let Some(reasoning) = cfg.reasoning {
                            task_reasoning.insert(task_type.clone(), reasoning);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[ModelRouter] 创建任务 {} 的 provider 失败: {}，回退到主 LLM API",
                            task_type,
                            e
                        );
                    }
                }
            }
        } else {
            tracing::info!(
                "[ModelRouter] 路由矩阵未启用，所有任务走主 LLM API（{} 个任务配置已忽略）",
                config.routing_matrix.len()
            );
        }

        // 3. 无任何可用 provider 时记录警告，以空 provider 构造 router（运行期 query 返回错误）
        if main_provider.is_none() && task_providers.is_empty() {
            tracing::warn!(
                "[ModelRouter] 主 LLM API 与路由矩阵均未配置，以空 provider 启动（等待用户配置）"
            );
        }

        // 4. 恢复工作智能体模型覆盖（若配置了 active_work_model）
        //    使重启 / save 后 reload 的新 router 自动沿用用户选中的工作模型，无需重新切换
        let reasoning_override = Self::build_reasoning_override(config, &client_cache);

        Ok(Self {
            main_provider: Arc::new(main_provider),
            task_providers: Arc::new(task_providers),
            jev_decision,
            reasoning_override: Arc::new(RwLock::new(reasoning_override)),
            enable_routing_matrix: config.enable_routing_matrix,
            enable_search: Arc::new(AtomicBool::new(false)),
            client_cache,
            app_handle: Arc::new(RwLock::new(None)),
            emit_enabled: Arc::new(AtomicBool::new(false)),
            semaphores: Arc::new(Self::build_semaphores()),
            error_toast_cooldown: Arc::new(RwLock::new(HashMap::new())),
            route_fallback_cooldown: Arc::new(RwLock::new(HashMap::new())),
            uses_proxy: config.network.proxy_mode != "direct",
            strict_broken: Arc::new(RwLock::new(load_strict_broken_models())),
            default_reasoning: config.ai.reasoning.unwrap_or(ReasoningPreference::AUTO),
            task_reasoning: Arc::new(task_reasoning),
            work_reasoning: Arc::new(RwLock::new(
                config
                    .active_work_model
                    .as_deref()
                    .and_then(|id| config.work_models.iter().find(|m| m.id == id))
                    .and_then(|profile| profile.route.reasoning),
            )),
            presence_penalty: config.ai.presence_penalty,
            frequency_penalty: config.ai.frequency_penalty,
        })
    }

    /// 当前是否走代理链路（基于 `config.network.proxy_mode`）
    ///
    /// `direct` 为直连，`system`/`custom`/`manual` 均视为走代理。
    /// 辅助任务可据此延长超时（代理链路通常比直连慢）。
    pub fn uses_proxy(&self) -> bool {
        self.uses_proxy
    }

    /// 依据配置构建工作智能体模型覆盖 provider（reasoning 任务）
    ///
    /// 查找 `config.active_work_model` 命中的预置项，配置完整时构建 provider；
    /// 未配置/未命中/配置不完整时返回 None（走默认路由）。
    fn build_reasoning_override(
        config: &AppConfig,
        client_cache: &ClientCache,
    ) -> Option<Arc<Box<dyn BaseProvider>>> {
        let active_id = config.active_work_model.as_deref()?;
        let profile: &WorkModelProfile = config
            .work_models
            .iter()
            .find(|m| m.id.as_str() == active_id)?;
        let cfg = &profile.route;
        if !provider_configuration_complete(cfg) {
            tracing::warn!(
                "[ModelRouter] 工作智能体模型 {} 配置不完整，跳过恢复覆盖",
                profile.name
            );
            return None;
        }
        // 用户填写的预算优先；未填写时使用编程默认预算。发送开关由 factory 应用。
        let mut cfg = cfg.clone();
        cfg.send_max_tokens = Some(cfg.send_max_tokens.unwrap_or(true));
        cfg.max_tokens = cfg.max_tokens.or_else(|| {
            Some(crate::providers::factory::work_model_default_max_tokens(
                &cfg.provider_type,
                &cfg.endpoint,
            ))
        });
        match create_task_provider(&cfg, config, client_cache) {
            Ok(p) => {
                // 工作智能体请求省略 temperature（服务端默认）：
                // 编程任务要确定性，且推理模型对非默认温度敏感
                p.set_omit_temperature(cfg.send_temperature.is_none());
                tracing::info!(
                    "[ModelRouter] 已恢复工作智能体模型覆盖: {} @ {}",
                    cfg.model,
                    cfg.endpoint
                );
                Some(Arc::new(p))
            }
            Err(e) => {
                tracing::warn!("[ModelRouter] 恢复工作智能体模型 provider 失败: {}", e);
                None
            }
        }
    }

    /// 构建按任务分组的并发信号量表
    fn build_semaphores() -> HashMap<String, Arc<Semaphore>> {
        let mut map = HashMap::new();
        map.insert(
            "chat_reasoning".to_string(),
            Arc::new(Semaphore::new(SEMAPHORE_GROUP_CHAT_REASONING)),
        );
        map.insert(
            "memory_reflection".to_string(),
            Arc::new(Semaphore::new(SEMAPHORE_GROUP_MEMORY_REFLECTION)),
        );
        map.insert(
            "auxiliary".to_string(),
            Arc::new(Semaphore::new(SEMAPHORE_GROUP_AUXILIARY)),
        );
        map
    }

    /// 获取任务对应的信号量（已构造的实例，按组名复用）
    fn get_semaphore(&self, task_type: &str) -> Option<Arc<Semaphore>> {
        let (group, _) = semaphore_for_task(task_type);
        self.semaphores.get(group).cloned()
    }

    /// 是否已配置主 LLM API（`config.ai` 三项字段齐全且 provider 构建成功）
    pub fn has_main_provider(&self) -> bool {
        self.main_provider.is_some()
    }

    pub fn has_task_provider(&self, task_type: &str) -> bool {
        self.enable_routing_matrix && self.task_providers.contains_key(task_type)
    }

    /// 注入 Tauri AppHandle，启用 `chat:route_fallback` 事件发送能力
    pub fn set_app_handle(&self, handle: AppHandle) {
        *self.app_handle.write() = Some(handle);
    }

    /// 临时开启 / 关闭路由回退事件发送
    ///
    /// 仅在用户主动发消息期间开启，避免主动对话轮询刷屏。
    pub fn set_emit_enabled(&self, enabled: bool) {
        self.emit_enabled.store(enabled, Ordering::Relaxed);
    }

    /// 设置全局联网搜索开关
    ///
    /// 会同步到所有 provider 的 `enable_search` 字段，确保流式/非流式均生效。
    pub fn set_enable_search(&self, enable: bool) {
        self.enable_search.store(enable, Ordering::Relaxed);
        if let Some(p) = self.main_provider.as_ref() {
            p.set_enable_search(enable);
        }
        for provider in self.task_providers.values() {
            provider.set_enable_search(enable);
        }
        tracing::info!("[ModelRouter] 全局联网搜索开关: {}", enable);
    }

    /// 读取全局联网搜索开关
    pub fn is_enable_search(&self) -> bool {
        self.enable_search.load(Ordering::Relaxed)
    }

    /// 全局设置 temperature 运行时覆盖（emotion→temperature 映射在每轮对话前调用）。
    ///
    /// 传播到 main_provider 和 task_providers 中的所有 provider。
    /// 传入 None 清除覆盖（恢复配置默认值）。
    pub fn set_temperature_override(&self, temp: Option<f64>) {
        if let Some(p) = self.main_provider.as_ref() {
            p.set_temperature_override(temp);
        }
        for provider in self.task_providers.values() {
            provider.set_temperature_override(temp);
        }
    }

    /// 解析请求的实际推理偏好：请求为 AUTO（未干预）且配置了全局默认时，
    /// 使用全局默认（`config.ai.reasoning`）；请求显式设置的偏好优先。
    fn effective_request_reasoning(
        &self,
        task_type: &str,
        requested: ReasoningPreference,
    ) -> ReasoningPreference {
        if requested == ReasoningPreference::AUTO {
            if task_type == TASK_WORK_AGENT {
                if let Some(pref) = *self.work_reasoning.read() {
                    return pref;
                }
            }
            super::task_catalog::route_keys(task_type).into_iter()
                .find_map(|key| self.task_reasoning.get(key).copied())
                .unwrap_or(self.default_reasoning)
        } else {
            requested
        }
    }

    /// 本请求应使用的惩罚参数。非"角色说话"的任务返回 `(None, None)`。
    ///
    /// 请求级字段优先（`LLMRequest::with_penalties`），未设置时用配置默认值。
    /// `0.0` 交由 `ProviderBase::sanitize_penalty` 折叠成"不发送"。
    fn conversational_penalties(&self, request: &LLMRequest) -> (Option<f64>, Option<f64>) {
        if !is_conversational_task(&request.task_type) {
            return (None, None);
        }
        (
            Some(request.presence_penalty.unwrap_or(self.presence_penalty)),
            Some(request.frequency_penalty.unwrap_or(self.frequency_penalty)),
        )
    }

    fn call_options(&self, request: &LLMRequest) -> ProviderCallOptions {
        let fingerprint_source = serde_json::json!({
            "task_type": request.task_type,
            "tools": request.tools,
            "json_schema": request.json_schema,
            "stream": request.stream,
            "include_framework_instructions": request.include_framework_instructions,
        })
        .to_string();
        let (presence_penalty, frequency_penalty) = self.conversational_penalties(request);
        ProviderCallOptions {
            include_framework_instructions: request.include_framework_instructions,
            enable_search: Some(request.enable_search),
            temperature: request.temperature_override,
            max_tokens: request.max_tokens_override,
            presence_penalty,
            frequency_penalty,
            reasoning: Some(
                self.effective_request_reasoning(&request.task_type, request.reasoning),
            ),
            json_schema: request.json_schema.clone().map(Arc::new),
            disable_json_schema: false,
            request_fingerprint: Some(crate::utils::fnv1a_64(&fingerprint_source)),
            response_cache_allowed: Some(request.tools.is_empty() && !request.enable_search),
        }
    }

    /// 为一组嵌套 LLM 调用设置请求级默认值，作用域结束时自动恢复。
    pub async fn scope_call_options<F, T>(&self, options: ProviderCallOptions, future: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        scope_provider_call(options, future).await
    }

    /// 清空客户端缓存
    ///
    /// 用于配置变更时重建客户端。下次创建 provider 时会重新构建并缓存。
    pub fn clear_client_cache(&self) {
        let count = self.client_cache.read().len();
        self.client_cache.write().clear();
        tracing::info!("[ModelRouter] 已清空客户端缓存（{} 条）", count);
    }

    /// 热重载
    ///
    /// 基于新配置重建 providers 与客户端缓存，返回新实例。
    /// 调用方（如 state 层）负责替换旧实例。
    pub fn reload(config: &AppConfig) -> VivianResult<Self> {
        tracing::info!("[ModelRouter] 模型路由正在重新加载...");
        let new_router = Self::new(config)?;
        tracing::info!("[ModelRouter] 模型路由已重新加载");
        Ok(new_router)
    }

    /// 内部：发送路由回退事件
    ///
    /// 带冷却机制：同一 task_type 在 ROUTE_FALLBACK_COOLDOWN_SECS 内不重复发送，
    /// 防止熔断状态下（如 inner_monologue 反复回退）toast 刷屏。
    ///
    /// `character_id` 为触发本次调用的角色归属（空串 = 无归属，前端不弹 toast）。
    fn emit_route_fallback(&self, task_type: &str, error: &str, character_id: &str) {
        if !self.emit_enabled.load(Ordering::Relaxed) {
            return;
        }
        {
            let mut cooldowns = self.route_fallback_cooldown.write();
            let now = Instant::now();
            if let Some(last_time) = cooldowns.get(task_type) {
                if last_time.elapsed().as_secs() < ROUTE_FALLBACK_COOLDOWN_SECS {
                    return;
                }
            }
            cooldowns.insert(task_type.to_string(), now);
        }
        let error_kind = classify_llm_error_from_str(error);
        let message_key = error_kind_to_message_key(&error_kind);
        let handle_guard = self.app_handle.read();
        if let Some(handle) = handle_guard.as_ref() {
            let _ = handle.emit(
                "chat:route_fallback",
                json!({
                    "task_type": task_type,
                    "error": error,
                    "error_kind": error_kind,
                    "message_key": message_key,
                    "character_id": character_id,
                    "fallback_to": "main",
                }),
            );
        }
    }

    /// 内部：发送路由状态事件（任务专属 provider 调用结果）
    ///
    /// 用于前端在路由矩阵 UI 中显示模型可用性：
    /// - "ok" → 模型名绿色（最近一次请求成功）
    /// - "error" → 模型名红色（最近一次请求失败，回退到主 LLM API）
    ///
    /// 不受 `emit_enabled` 限制：状态追踪需要覆盖所有调用场景（含后台任务），
    /// 仅用于 UI 颜色标记，不会产生 toast 通知，因此无刷屏问题。
    fn emit_route_status(&self, task_type: &str, status: &str) {
        let handle_guard = self.app_handle.read();
        if let Some(handle) = handle_guard.as_ref() {
            let _ = handle.emit(
                "chat:route_status",
                json!({
                    "task_type": task_type,
                    "status": status,
                }),
            );
        }
    }

    /// 内部：发送 LLM 错误 toast 事件（不区分 emit_enabled，错误应始终通知用户）
    ///
    /// 带冷却机制：同类 error_kind 在 COOLDOWN_SECS 内不重复弹窗，防止 Permanent 错误反复刷屏。
    /// Permanent 类错误（InvalidApiKey/InsufficientBalance/QuotaExceeded/ModelNotFound/
    /// RegionNotSupported/PermissionDenied）冷却时间更长（5 分钟），因为需要用户手动修复。
    ///
    /// 熔断器打开不在此通知：它是本地自我保护，冷却结束会自行半开探测，用户无需介入；
    /// 且「余额不足」引发的连续失败会同时触发熔断，两条 toast 并存时后者会盖掉真正该看的那条。
    ///
    /// `character_id` 为触发本次调用的角色归属（空串 = 无归属，前端不弹 toast）。
    fn emit_llm_error_toast(
        &self,
        task_type: &str,
        error: &str,
        endpoint: &str,
        character_id: &str,
    ) {
        let error_kind = classify_llm_error_from_str(error);

        if matches!(error_kind, LlmErrorKind::CircuitBreakerOpen) {
            return;
        }

        let cooldown = match error_kind {
            LlmErrorKind::InvalidApiKey
            | LlmErrorKind::InsufficientBalance
            | LlmErrorKind::QuotaExceeded
            | LlmErrorKind::ModelNotFound
            | LlmErrorKind::RegionNotSupported
            | LlmErrorKind::PermissionDenied => 300,
            _ => ERROR_TOAST_COOLDOWN_SECS,
        };

        {
            let mut cooldowns = self.error_toast_cooldown.write();
            let now = Instant::now();
            if let Some(last_time) = cooldowns.get(&error_kind) {
                if last_time.elapsed().as_secs() < cooldown {
                    return;
                }
            }
            cooldowns.insert(error_kind.clone(), now);
        }

        let message_key = error_kind_to_message_key(&error_kind);
        // endpoint 由调用方在失败现场传入：本次调用最后实际失败的 provider 的 endpoint
        // （跟随 fallback 链，而非任务最初的优选 endpoint）。前端按它反查厂商预设，
        // 把「余额不足」类错误挂上直达对应厂商控制台的动作。
        let handle_guard = self.app_handle.read();
        if let Some(handle) = handle_guard.as_ref() {
            let _ = handle.emit(
                "llm:error",
                json!({
                    "task_type": task_type,
                    "error": error,
                    "error_kind": error_kind,
                    "message_key": message_key,
                    "character_id": character_id,
                    "endpoint": endpoint,
                }),
            );
        }
    }

    /// One ordered candidate list for all four invocation modes.
    fn route_candidates(&self, task_type: &str, require_tools: bool) -> Vec<RouteProvider<'_>> {
        let mut candidates = Vec::new();
        if let Some(provider) = self.override_provider_for(task_type) {
            candidates.push(RouteProvider::Override(provider));
        }
        if self.enable_routing_matrix {
            for key in super::task_catalog::route_keys(task_type) {
                if let Some((route, provider)) = self.task_providers.get_key_value(key) {
                    candidates.push(RouteProvider::Task { route, provider });
                }
            }
        }
        if let Some(provider) = self.main_provider.as_ref() {
            candidates.push(RouteProvider::Main(provider));
        }
        candidates.retain(|candidate| {
            let supported = !require_tools || candidate.provider().supports_native_function_calling();
            if !supported {
                tracing::info!(task = task_type, route = candidate.route(), model = candidate.provider().get_model(),
                    "Route candidate does not support native tools; text transport remains available");
            }
            supported
        });
        candidates
    }

    fn record_route_attempt(&self, task_type: &str, candidate: &RouteProvider<'_>) {
        tracing::info!(task = task_type, resolved_route = candidate.route(), source = candidate.source(),
            model = candidate.provider().get_model(), prompt_contract = task_type, "Model route resolved");
        if let Some(handle) = self.app_handle.read().as_ref() {
            let _ = handle.emit("chat:route_resolved", json!({ "task_type": task_type,
                "resolved_route": candidate.route(), "source": candidate.source(),
                "model": candidate.provider().get_model(), "prompt_contract": task_type }));
        }
    }

    fn route_success(&self, candidate: &RouteProvider<'_>) {
        // A successful main fallback must not paint a failed task provider green.
        if !matches!(candidate, RouteProvider::Main(_)) {
            self.emit_route_status(candidate.route(), "ok");
        }
    }

    async fn acquire_route_permit(
        &self,
        task_type: &str,
    ) -> VivianResult<Option<tokio::sync::OwnedSemaphorePermit>> {
        match self.get_semaphore(task_type) {
            Some(semaphore) => Ok(Some(semaphore.acquire_owned().await.map_err(|_| {
                VivianError::Provider("Provider concurrency queue closed".into())
            })?)),
            None => Ok(None),
        }
    }

    async fn with_candidate_schema<T, F, Fut>(
        &self,
        provider: &Box<dyn BaseProvider>,
        schema: Option<serde_json::Value>,
        operation: F,
    ) -> VivianResult<T>
    where
        F: FnMut(Option<serde_json::Value>) -> Fut,
        Fut: std::future::Future<Output = VivianResult<T>>,
    {
        super::routing::with_candidate_schema(
            provider.as_ref(),
            &self.strict_broken,
            save_strict_broken_models,
            schema,
            operation,
        )
        .await
    }

    fn route_failure(
        &self,
        task_type: &str,
        candidate: &RouteProvider<'_>,
        error: &VivianError,
        character_id: &str,
        has_next: bool,
    ) {
        self.emit_route_status(task_type, "error");
        if candidate.route() != task_type && !matches!(candidate, RouteProvider::Main(_)) {
            self.emit_route_status(candidate.route(), "error");
        }
        if has_next {
            self.emit_route_fallback(task_type, &error.to_string(), character_id);
        }
    }

    /// Commit a route only after a useful event. Leading usage/citations are replayed
    /// in order; errors and empty streams before output still allow failover.
    async fn prime_stream(
        source: mpsc::Receiver<StreamEvent>,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        super::routing::prime_stream(source).await
    }

    async fn query_with_fallback(
        &self,
        task_type: &str,
        messages: Vec<ChatMessage>,
        json_schema: Option<serde_json::Value>,
        character_id: &str,
    ) -> VivianResult<String> {
        Self::log_llm_request(task_type, &messages, &[]);
        let _permit = self.acquire_route_permit(task_type).await?;
        let candidates = self.route_candidates(task_type, false);
        let mut last_error = VivianError::Provider("没有可用的提供商".into());
        let mut endpoint = String::new();
        for (index, candidate) in candidates.iter().enumerate() {
            let provider = candidate.provider();
            self.record_route_attempt(task_type, candidate);
            endpoint = provider.get_endpoint().to_string();
            let result = self
                .with_candidate_schema(provider, json_schema.clone(), |schema| {
                    let messages = messages.clone();
                    async move {
                        provider
                            .call_chat_with_search(
                                messages,
                                ProviderCallOptions::current()
                                    .enable_search
                                    .unwrap_or_else(|| self.is_enable_search()),
                                schema,
                            )
                            .await
                    }
                })
                .await;
            match result {
                Ok(content) => {
                    Self::log_text_response(task_type, &content);
                    self.route_success(candidate);
                    return Ok(content);
                }
                Err(error) => {
                    let failover = crate::providers::transport::may_failover(&error);
                    self.route_failure(
                        task_type,
                        candidate,
                        &error,
                        character_id,
                        failover && index + 1 < candidates.len(),
                    );
                    last_error = error;
                    if !failover {
                        break;
                    }
                }
            }
        }
        self.emit_llm_error_toast(task_type, &last_error.to_string(), &endpoint, character_id);
        Err(last_error)
    }

    async fn query_stream(
        &self,
        task_type: &str,
        messages: Vec<ChatMessage>,
        json_schema: Option<serde_json::Value>,
        character_id: &str,
        usage_tag: &str,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        Self::log_llm_request(task_type, &messages, &[]);
        let permit = self.acquire_route_permit(task_type).await?;
        let candidates = self.route_candidates(task_type, false);
        let mut last_error = VivianError::Provider("没有可用的流式提供商".into());
        let mut endpoint = String::new();
        for (index, candidate) in candidates.iter().enumerate() {
            let provider = candidate.provider();
            self.record_route_attempt(task_type, candidate);
            endpoint = provider.get_endpoint().to_string();
            let result = self
                .with_candidate_schema(provider, json_schema.clone(), |schema| {
                    let messages = messages.clone();
                    async move {
                        let source = provider.call_stream_chat(messages, schema).await?;
                        Self::prime_stream(source).await
                    }
                })
                .await;
            match result {
                Ok(source) => {
                    self.route_success(candidate);
                    return Ok(Self::hold_text_stream_permit(
                        source,
                        permit,
                        provider.get_model().to_string(),
                        usage_tag.to_string(),
                        task_type.to_string(),
                    ));
                }
                Err(error) => {
                    let failover = crate::providers::transport::may_failover(&error);
                    self.route_failure(
                        task_type,
                        candidate,
                        &error,
                        character_id,
                        failover && index + 1 < candidates.len(),
                    );
                    last_error = error;
                    if !failover {
                        break;
                    }
                }
            }
        }
        self.emit_llm_error_toast(task_type, &last_error.to_string(), &endpoint, character_id);
        Err(last_error)
    }

    fn hold_text_stream_permit(
        mut source: mpsc::Receiver<StreamEvent>,
        permit: Option<tokio::sync::OwnedSemaphorePermit>,
        model: String,
        usage_tag: String,
        route: String,
    ) -> mpsc::Receiver<StreamEvent> {
        let (tx, rx) = mpsc::channel(32);
        tokio::spawn(async move {
            let _permit = permit;
            let mut usage = usage_store::StreamUsageAccumulator::default();
            let mut content_bytes = 0usize;
            let mut content_hash = 0xcbf29ce484222325u64;
            let mut outcome = "completed";
            loop {
                let event = tokio::select! {
                    _ = tx.closed() => { outcome = "consumer_closed"; break; },
                    event = source.recv() => event,
                };
                let Some(event) = event else {
                    break;
                };
                if let StreamEvent::Text { content } = &event {
                    content_bytes += content.len();
                    for byte in content.bytes() {
                        content_hash ^= byte as u64;
                        content_hash = content_hash.wrapping_mul(0x100000001b3);
                    }
                }
                if matches!(&event, StreamEvent::Error { .. }) {
                    outcome = "provider_error";
                }
                if let StreamEvent::Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                } = &event
                {
                    usage.observe(
                        *input_tokens,
                        *output_tokens,
                        *cache_read_tokens,
                        *cache_write_tokens,
                    );
                }
                if tx.send(event).await.is_err() {
                    outcome = "consumer_closed";
                    break;
                }
            }
            tracing::debug!("[LLM-IO] <<< stream task={} usage_tag={} model={} outcome={} content_bytes={} content_hash={:016x}",
                route, usage_tag, model, outcome, content_bytes, content_hash);
            usage.record(Some(&usage_tag), Some(&route), &model);
        });
        rx
    }

    // ========================================================================
    // 原生 function calling 路径
    // ========================================================================
    //
    // 当 provider 支持 `bind_tools` + `invoke`（覆盖了 trait 默认实现）时，
    // 调用方可走结构化路径，避免在 prompt 里注入工具列表 + 解析 JSON 字符串。
    //
    // 路由顺序与 `query_with_fallback` 一致：task_providers → main_provider
    // 失败时按 fallback 链回退到主 API（带 toast 通知）。

    /// 当前任务路由目标是否支持原生 function calling
    ///
    /// 调用方应在调用 `query_with_tools` 前先检测，以决定走原生路径还是文本路径。
    /// 注意：返回 true 仅表示 provider 实现了 `bind_tools` / `invoke`，
    /// 实际是否走原生路径还需 config 开关 `enable_native_function_calling` 配合。
    pub fn supports_native_function_calling(&self, task_type: &str) -> bool {
        if self
            .override_provider_for(task_type)
            .is_some_and(|p| p.supports_native_function_calling())
        {
            return true;
        }
        if self.enable_routing_matrix
            && self
                .task_providers
                .get(task_type)
                .is_some_and(|p| p.supports_native_function_calling())
        {
            return true;
        }
        self.main_provider
            .as_ref()
            .as_ref()
            .is_some_and(|p| p.supports_native_function_calling())
    }

    /// 当前 chat 任务的 provider 是否支持原生 JSON Schema 约束
    pub fn supports_structured_output(&self) -> bool {
        if self.enable_routing_matrix
            && self
                .task_providers
                .get("chat")
                .is_some_and(|p| p.supports_structured_output() || p.supports_json_mode())
        {
            return true;
        }
        self.main_provider
            .as_ref()
            .as_ref()
            .is_some_and(|p| p.supports_structured_output() || p.supports_json_mode())
    }

    // ========================================================================
    // 统一 LLMRequest 接口（推荐新代码使用）
    // -----------------------------------------------------------------------
    // 把原本散落的 task_type / messages / tools / stream / enable_search
    // 参数打包成单一 LLMRequest 结构,Brain 无需关心底层是 Responses API /
    // Chat Completions / Anthropic Messages / Gemini GenerateContent。
    //
    // 内部转调现有 query_with_fallback / query_stream / query_with_tools /
    // query_stream_with_tools 方法,保持向后兼容。后续可逐步废弃旧方法。
    // ========================================================================

    /// 统一文本生成入口(无工具)
    ///
    /// 根据 `request.stream` 转调 `query_with_fallback` 或 `query_stream`。
    /// 所有请求参数通过 task-local 作用域传递，并发调用之间互不污染。
    /// Optional small decision route. No configured route means callers use their
    /// existing rule or task-specific LLM path; we do not spend a main-chat call.
    pub async fn choose_simple(
        &self,
        state: serde_json::Value,
        instructions: &str,
        choices: &[(&str, &str)],
        character_id: &str,
    ) -> Option<String> {
        if let Some(jev) = &self.jev_decision {
            match jev.choose(state, instructions, choices).await {
                Ok(choice) => {
                    self.emit_route_status("simple_judge", "ok");
                    tracing::debug!(
                        "[simple_judge] Jev choice={} confidence={:.3}",
                        choice.choice,
                        choice.confidence,
                    );
                    return Some(choice.choice);
                }
                Err(e) => {
                    tracing::warn!("[simple_judge] Jev failed: {e}");
                    self.emit_route_status("simple_judge", "error");
                    return None;
                }
            }
        }
        if !self.enable_routing_matrix || !self.task_providers.contains_key("simple_judge") {
            return None;
        }
        let options: serde_json::Map<String, serde_json::Value> = choices
            .iter()
            .map(|(name, description)| ((*name).to_owned(), json!(description)))
            .collect();
        let messages = vec![
            ChatMessage::system("You make one small decision. Return only JSON with one key, choice. The choice must exactly match an option key."),
            ChatMessage::user(format!("Instructions: {instructions}\nOptions: {}\nState: {state}", json!(options))),
        ];
        match self
            .generate(
                LLMRequest::new("simple_judge", messages)
                    .with_character_id(character_id.to_owned())
                    .with_temperature(0.0)
                    .with_max_tokens(64),
            )
            .await
        {
            Ok(reply) => {
                let value = reply.find('{').and_then(|start| {
                    reply
                        .rfind('}')
                        .filter(|end| *end >= start)
                        .and_then(|end| {
                            serde_json::from_str::<serde_json::Value>(&reply[start..=end]).ok()
                        })
                });
                let choice = value
                    .as_ref()
                    .and_then(|v| v.get("choice"))
                    .and_then(|v| v.as_str())?;
                choices
                    .iter()
                    .any(|(name, _)| *name == choice)
                    .then(|| choice.to_owned())
            }
            Err(e) => {
                tracing::warn!("[simple_judge] LLM failed: {e}");
                None
            }
        }
    }

    pub async fn choose_simple_for(
        &self, task_type: &str, state: serde_json::Value, instructions: &str,
        choices: &[(&str, &str)], character_id: &str,
    ) -> Option<String> {
        if self.has_task_provider(task_type) { return None; }
        let contract = super::task_catalog::find(task_type).map(super::task_catalog::prompt).unwrap_or_default();
        self.choose_simple(state, &format!("{contract}\n{instructions}"), choices, character_id).await
    }

    /// Batch independent yes/no judgments. Values are probabilities of `true`.
    /// Returns None when the route is unconfigured or the response is invalid.
    pub async fn judge_noul_batch(
        &self,
        state: serde_json::Value,
        questions: &[(&str, &str, &str, &str)],
        character_id: &str,
    ) -> Option<HashMap<String, f64>> {
        if questions.is_empty() {
            return Some(HashMap::new());
        }
        if let Some(jev) = &self.jev_decision {
            return match jev.noul_batch(state, questions).await {
                Ok(answers) => {
                    self.emit_route_status("simple_judge", "ok");
                    Some(answers)
                }
                Err(e) => {
                    tracing::warn!("[simple_judge] Jev batch failed: {e}");
                    self.emit_route_status("simple_judge", "error");
                    None
                }
            };
        }
        if !self.enable_routing_matrix || !self.task_providers.contains_key("simple_judge") {
            return None;
        }
        let definitions: serde_json::Map<String, serde_json::Value> = questions
            .iter()
            .map(|(key, instruction, yes, no)| {
                (
                    (*key).to_owned(),
                    json!({
                        "question": instruction, "true": yes, "false": no
                    }),
                )
            })
            .collect();
        let messages = vec![
            ChatMessage::system("Answer independent yes/no questions. Return only a JSON object mapping every question key to the probability (0 to 1) that its true criterion holds. No prose."),
            ChatMessage::user(format!("Questions: {}\nState: {state}", json!(definitions))),
        ];
        let reply = self
            .generate(
                LLMRequest::new("simple_judge", messages)
                    .with_character_id(character_id.to_owned())
                    .with_temperature(0.0)
                    .with_max_tokens((questions.len() as u32 * 12 + 32).min(512)),
            )
            .await
            .ok()?;
        let start = reply.find('{')?;
        let end = reply.rfind('}')?;
        let values: serde_json::Value = serde_json::from_str(reply.get(start..=end)?).ok()?;
        let mut answers = HashMap::new();
        for (key, _, _, _) in questions {
            let probability = values.get(*key)?.as_f64()?;
            if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
                return None;
            }
            answers.insert((*key).to_owned(), probability);
        }
        Some(answers)
    }

    pub async fn judge_noul_batch_for(
        &self, task_type: &str, state: serde_json::Value,
        questions: &[(&str, &str, &str, &str)], character_id: &str,
    ) -> Option<HashMap<String, f64>> {
        if self.has_task_provider(task_type) { return None; }
        self.judge_noul_batch(state, questions, character_id).await
    }

    /// Mixed Choice + Score route for emotion classification. A normal chat
    /// provider keeps using the existing emotion_analysis prompt as fallback.
    pub async fn classify_emotion_simple(
        &self,
        text: &str,
        labels: &[&str],
    ) -> Option<(String, f64)> {
        if self.has_task_provider("emotion_analysis") { return None; }
        let jev = self.jev_decision.as_ref()?;
        match jev.classify_emotion(text, labels).await {
            Ok(result) => {
                self.emit_route_status("simple_judge", "ok");
                Some(result)
            }
            Err(e) => {
                tracing::warn!("[simple_judge] Jev emotion classification failed: {e}");
                self.emit_route_status("simple_judge", "error");
                None
            }
        }
    }

    pub async fn generate(&self, mut request: LLMRequest) -> VivianResult<String> {
        self.prepare_request(&mut request)?;
        Self::repair_request_history(&mut request);
        let options = self.call_options(&request);
        let usage_tag = request
            .usage_tag
            .clone()
            .unwrap_or_else(|| request.task_type.clone());
        let LLMRequest {
            task_type,
            messages,
            stream,
            tools,
            json_schema,
            character_id,
            ..
        } = request.clone();
        // tools 非空应走 generate_with_tools,这里防御性检查
        if !tools.is_empty() {
            return Err(VivianError::Provider(
                "generate() 不支持 tools 非空,请用 generate_with_tools()".to_string(),
            ));
        }
        let effective_schema = json_schema.clone();
        let char_id = character_id.as_deref().unwrap_or("");
        let result = usage_store::with_context(
            &usage_tag,
            &task_type,
            scope_provider_call(options, async {
                if stream {
                    // 流式:累积所有 chunk 返回完整文本
                    let mut rx = self
                        .query_stream(&task_type, messages, effective_schema, char_id, &usage_tag)
                        .await?;
                    let mut buf = String::new();
                    let mut web_sources = Vec::new();
                    while let Some(event) = rx.recv().await {
                        match event {
                            StreamEvent::Text { content } => buf.push_str(&content),
                            StreamEvent::WebSources { sources } => web_sources.extend(sources),
                            StreamEvent::Error { message } => {
                                return Err(VivianError::Provider(format!(
                                    "流式响应中断: {}",
                                    message
                                )));
                            }
                            _ => {}
                        }
                    }
                    Ok(crate::providers::web_citations::attach(&buf, &web_sources))
                } else {
                    self.query_with_fallback(&task_type, messages, effective_schema, char_id)
                        .await
                }
            }),
        )
        .await;
        result
    }

    /// 统一流式文本生成入口(无工具)
    ///
    /// 返回 chunk Receiver,调用方自行累积。
    pub async fn generate_stream(
        &self,
        mut request: LLMRequest,
    ) -> VivianResult<tokio::sync::mpsc::Receiver<StreamEvent>> {
        self.prepare_request(&mut request)?;
        Self::repair_request_history(&mut request);
        let options = self.call_options(&request);
        let usage_tag = request
            .usage_tag
            .clone()
            .unwrap_or_else(|| request.task_type.clone());
        let LLMRequest {
            task_type,
            messages,
            tools,
            json_schema,
            stream: _,
            character_id,
            ..
        } = request.clone();
        if !tools.is_empty() {
            return Err(VivianError::Provider(
                "generate_stream() 不支持 tools 非空,请用 generate_stream_with_tools()".to_string(),
            ));
        }
        let effective_schema = json_schema.clone();
        let rx = scope_provider_call(
            options,
            self.query_stream(
                &task_type,
                messages,
                effective_schema,
                character_id.as_deref().unwrap_or(""),
                &usage_tag,
            ),
        )
        .await;
        rx
    }

    /// 统一工具调用入口(原生 function calling 非流式)
    ///
    /// 内部转调 `query_with_tools`。调用方应先通过 `supports_native_function_calling`
    /// 确认 provider 支持,否则应回退到文本路径。
    pub async fn generate_with_tools(&self, mut request: LLMRequest) -> VivianResult<ChatResponse> {
        self.prepare_request(&mut request)?;
        Self::repair_request_history(&mut request);
        let options = self.call_options(&request);
        let usage_tag = request
            .usage_tag
            .clone()
            .unwrap_or_else(|| request.task_type.clone());
        let LLMRequest {
            task_type,
            messages,
            tools,
            json_schema: _,
            stream: _,
            character_id,
            ..
        } = request.clone();
        let result = usage_store::with_context(
            &usage_tag,
            &task_type,
            scope_provider_call(
                options,
                self.query_with_tools(
                    &task_type,
                    messages,
                    tools,
                    character_id.as_deref().unwrap_or(""),
                ),
            ),
        )
        .await;
        result
    }

    /// 统一工具调用入口(原生 function calling 流式)
    ///
    /// 返回 StreamEvent Receiver,调用方按事件类型累积文本/工具调用。
    pub async fn generate_stream_with_tools(
        &self,
        mut request: LLMRequest,
    ) -> VivianResult<tokio::sync::mpsc::Receiver<crate::providers::base::StreamEvent>> {
        self.prepare_request(&mut request)?;
        Self::repair_request_history(&mut request);
        let options = self.call_options(&request);
        let usage_tag = request
            .usage_tag
            .clone()
            .unwrap_or_else(|| request.task_type.clone());
        let LLMRequest {
            task_type,
            messages,
            tools,
            json_schema: _,
            stream: _,
            character_id,
            ..
        } = request.clone();
        let rx = scope_provider_call(
            options,
            self.query_stream_with_tools(
                &task_type,
                messages,
                tools,
                character_id.as_deref().unwrap_or(""),
                &usage_tag,
            ),
        )
        .await;
        rx
    }

    fn prepare_request(&self, request: &mut LLMRequest) -> VivianResult<()> {
        if let Some(spec) = super::task_catalog::find(&request.task_type) {
            let prompt = super::task_catalog::prompt(spec);
            if !request.messages.iter().any(|message| message.role == "system" && message.content == prompt) {
                request.messages.insert(0, ChatMessage::system(prompt));
            }
        } else if !self.has_task_provider(&request.task_type)
            || !request.messages.iter().any(|message| message.role == "system" && !message.content.trim().is_empty()) {
            return Err(VivianError::Config(format!(
                "Unknown task route '{}': use a registered task, or configure a custom route with explicit system instructions",
                request.task_type)));
        }
        // Provider fallback must not inject the main desktop-pet framework into a private task.
        request.include_framework_instructions = Some(false);
        Ok(())
    }

    /// 四个请求入口统一治理历史，文本收尾请求也可能携带此前的工具调用。
    fn repair_request_history(request: &mut LLMRequest) {
        let repairs = super::tool_history::repair_tool_history(&mut request.messages);
        if repairs != super::tool_history::RepairStats::default() {
            tracing::warn!(task = %request.task_type, ?repairs, "已修复请求中的工具历史结构");
        }
    }

    /// 设置工作智能体模型覆盖（reasoning 热切换）
    ///
    /// 由命令层用 `create_task_provider` 构建 provider 后传入；`None` 清除覆盖。
    pub fn set_reasoning_override(&self, provider: Option<Box<dyn BaseProvider>>) {
        if provider.is_none() {
            *self.work_reasoning.write() = None;
        }
        *self.reasoning_override.write() = provider.map(Arc::new);
        tracing::info!("[ModelRouter] 工作智能体模型覆盖已更新");
    }

    /// 根据工作智能体模型配置构建 provider 并设为覆盖（reasoning 热切换）
    ///
    /// 由命令层在用户切换工作模型时调用，复用路由矩阵的 provider 构建链路
    /// （共享 client_cache，确保国内/代理分流与缓存命中一致）。
    pub fn set_work_model_override(
        &self,
        task_config: &TaskRouteConfig,
        config: &AppConfig,
    ) -> VivianResult<()> {
        // 用户填写的预算优先；未填写时使用编程默认预算。
        let mut cfg = task_config.clone();
        cfg.send_max_tokens = Some(cfg.send_max_tokens.unwrap_or(true));
        cfg.max_tokens = cfg.max_tokens.or_else(|| {
            Some(crate::providers::factory::work_model_default_max_tokens(
                &cfg.provider_type,
                &cfg.endpoint,
            ))
        });
        match create_task_provider(&cfg, config, &self.client_cache) {
            Ok(provider) => {
                // 工作智能体请求省略 temperature（服务端默认）：
                // 编程任务要确定性，且推理模型对非默认温度敏感
                provider.set_omit_temperature(cfg.send_temperature.is_none());
                tracing::info!(
                    "[ModelRouter] 工作智能体模型 {} 已切换 {} @ {}",
                    task_config.model,
                    task_config.provider_type,
                    task_config.endpoint
                );
                *self.work_reasoning.write() = task_config.reasoning;
                self.set_reasoning_override(Some(provider));
                Ok(())
            }
            Err(e) => {
                tracing::warn!("[ModelRouter] 构建工作智能体 provider 失败: {}", e);
                Err(e)
            }
        }
    }

    /// 取 reasoning 任务的运行时覆盖 provider（owned Arc）
    ///
    /// 仅返回用户显式切换的工作模型；未覆盖时返回 None（调用方回退默认路由）。
    pub fn reasoning_override_provider(&self) -> Option<Arc<Box<dyn BaseProvider>>> {
        self.reasoning_override.read().clone()
    }

    /// 解析指定任务是否命中工作智能体覆盖
    ///
    /// 仅 reasoning 任务存在覆盖时返回 Some，其他任务一律 None（保持默认路由）。
    /// 工作智能体覆盖模型：只对工作智能体自己的任务类型生效
    ///
    /// 此前按 `task_type == "reasoning"` 匹配，而陪伴对话携带工具定义时会
    /// 从 `chat` 升级成 `reasoning`——于是角色的每句台词都被编程模型接管，
    /// 连带套上该 provider 的 `omit_temperature`，情绪驱动的温度被静默丢弃。
    ///
    /// 现在工作智能体走 `TASK_WORK_AGENT`，陪伴对话无论升级成什么都碰不到这里，
    /// 不需要任何"绕过"标志。
    fn override_provider_for(&self, task_type: &str) -> Option<Arc<Box<dyn BaseProvider>>> {
        if task_type == TASK_WORK_AGENT {
            self.reasoning_override.read().clone()
        } else {
            None
        }
    }

    /// 解析任务的当前 provider（按路由顺序：task_providers → main）
    /// Configured model label; fallback may use a different model.
    pub fn dialogue_model_name(&self, task_type: &str) -> String {
        self.resolve_provider(task_type)
            .map(|p| p.get_model().to_string())
            .unwrap_or_default()
    }

    fn resolve_provider(&self, task_type: &str) -> Option<&Box<dyn BaseProvider>> {
        if self.enable_routing_matrix {
            for key in super::task_catalog::route_keys(task_type) {
                if let Some(provider) = self.task_providers.get(key) { return Some(provider); }
            }
        }
        self.main_provider.as_ref().as_ref()
    }

    /// 带原生 function calling 的对话查询
    ///
    /// 内部流程：
    /// 1. 解析任务 provider（task / main）
    /// 2. 检测能力：若 provider 不支持原生 function calling，返回 `NotImplemented` 错误
    /// 3. 调用 `bind_tools` 得到一个携带工具 schema 的 provider 实例
    /// 4. 调用 `invoke(messages)` 返回 `ChatResponse`（含结构化 tool_calls）
    /// 5. 失败时按 fallback 链回退到主 API（带 toast 通知）
    ///
    /// 调用方应在 `supports_native_function_calling` 返回 true 时调用此方法。
    /// 否则应回退到 `query_with_fallback`（文本路径）。
    async fn query_with_tools(
        &self,
        task_type: &str,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
        character_id: &str,
    ) -> VivianResult<ChatResponse> {
        Self::log_llm_request(task_type, &messages, &tools);
        let _permit = self.acquire_route_permit(task_type).await?;
        let candidates = self.route_candidates(task_type, true);
        let mut last_error =
            VivianError::NotImplemented(format!("任务 {task_type} 没有支持工具调用的 provider"));
        let mut endpoint = String::new();
        for (index, candidate) in candidates.iter().enumerate() {
            let provider = candidate.provider();
            self.record_route_attempt(task_type, candidate);
            endpoint = provider.get_endpoint().to_string();
            let result = self
                .with_candidate_schema(provider, ProviderCallOptions::current_json_schema(), |_| {
                    let messages = messages.clone();
                    let tools = tools.clone();
                    async move { Self::invoke_with_tools(provider, messages, tools).await }
                })
                .await;
            match result {
                Ok(response) => {
                    Self::log_llm_response(task_type, &response);
                    self.route_success(candidate);
                    return Ok(response);
                }
                Err(error) => {
                    let failover = crate::providers::transport::may_failover(&error);
                    self.route_failure(
                        task_type,
                        candidate,
                        &error,
                        character_id,
                        failover && index + 1 < candidates.len(),
                    );
                    last_error = error;
                    if !failover {
                        break;
                    }
                }
            }
        }
        self.emit_llm_error_toast(task_type, &last_error.to_string(), &endpoint, character_id);
        Err(last_error)
    }

    /// 内部辅助：bind_tools + invoke 的两步组合
    async fn invoke_with_tools(
        provider: &Box<dyn BaseProvider>,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
    ) -> VivianResult<ChatResponse> {
        let result = if tools.is_empty() {
            // 无工具时直接调用 invoke（provider 会回退到 call_chat）
            provider.invoke(messages).await
        } else {
            let bound = provider.bind_tools(tools)?;
            bound.invoke(messages).await
        };
        // 用量采集：从原始响应解析 usage 并按模型累计
        if let Ok(resp) = &result {
            Self::record_usage_from_raw(provider.get_model(), &resp.raw);
        }
        result
    }

    /// 从非流式原始响应解析 usage 并计入全局用量存储。
    ///
    /// 兼容 OpenAI（prompt_tokens/completion_tokens）、Anthropic
    /// （input_tokens/output_tokens）与缓存字段；无 usage 的响应跳过。
    fn record_usage_from_raw(model: &str, raw: &serde_json::Value) {
        crate::providers::base::record_response_usage(model, raw);
    }

    fn log_text_response(task_type: &str, content: &str) {
        tracing::debug!(
            "[LLM-IO] <<< task={} content_bytes={} content_hash={:016x}",
            task_type,
            content.len(),
            crate::utils::fnv1a_64(content),
        );
    }

    /// LLM 请求/响应日志辅助函数
    ///
    /// 在四个 LLM 入口（query_with_fallback / query_stream / query_with_tools /
    /// query_stream_with_tools）调用前埋点，输出 task_type、消息序列（role + 截断后的
    /// content 长度/摘要、tools 名称列表。默认日志永不写入 prompt、响应或工具参数。
    fn log_llm_request(task_type: &str, messages: &[ChatMessage], tools: &[ToolDefinition]) {
        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        let lines: Vec<String> = messages
            .iter()
            .enumerate()
            .map(|(i, m)| {
                format!(
                    "  [{}] role={} bytes={} hash={:016x} images={} tool_calls={}",
                    i,
                    m.role,
                    m.content.len(),
                    crate::utils::fnv1a_64(&m.content),
                    m.images.as_ref().map_or(0, Vec::len),
                    m.tool_calls.as_ref().map_or(0, Vec::len),
                )
            })
            .collect();
        tracing::debug!(
            "[LLM-IO] >>> task={} msg_count={} tools=[{}]\n{}",
            task_type,
            messages.len(),
            tool_names.join(", "),
            lines.join("\n")
        );
    }

    /// LLM 非流式响应日志辅助函数
    fn log_llm_response(task_type: &str, resp: &ChatResponse) {
        let tool_names: Vec<&str> = resp.tool_calls.iter().map(|tc| tc.name.as_str()).collect();
        tracing::debug!(
            "[LLM-IO] <<< task={} finish_reason={:?} content_bytes={} content_hash={:016x} tools=[{}]",
            task_type,
            resp.finish_reason,
            resp.content.len(),
            crate::utils::fnv1a_64(&resp.content),
            tool_names.join(", ")
        );
    }

    /// 带原生 function calling 的流式对话查询
    ///
    /// 与 `query_with_tools` 的区别：返回 `StreamEvent` 流而非一次性 `ChatResponse`。
    /// 调用方需在接收端累积 `ToolCallDelta` 事件得到完整的工具调用列表。
    ///
    /// 路由顺序与 fallback 链：task_providers → main_provider。
    /// 仅 `supports_native_function_calling` 返回 true 的 provider 才被尝试。
    /// 所有候选 provider 都不支持时返回 `NotImplemented`。
    async fn query_stream_with_tools(
        &self,
        task_type: &str,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
        character_id: &str,
        usage_tag: &str,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        Self::log_llm_request(task_type, &messages, &tools);
        let permit = self.acquire_route_permit(task_type).await?;
        let candidates = self.route_candidates(task_type, true);
        let mut last_error = VivianError::NotImplemented(format!(
            "任务 {task_type} 没有支持流式工具调用的 provider"
        ));
        let mut endpoint = String::new();
        for (index, candidate) in candidates.iter().enumerate() {
            let provider = candidate.provider();
            self.record_route_attempt(task_type, candidate);
            endpoint = provider.get_endpoint().to_string();
            let result = self
                .with_candidate_schema(provider, ProviderCallOptions::current_json_schema(), |_| {
                    let messages = messages.clone();
                    let tools = tools.clone();
                    async move {
                        let source = provider.stream_with_tools(messages, tools).await?;
                        Self::prime_stream(source).await
                    }
                })
                .await;
            match result {
                Ok(source) => {
                    self.route_success(candidate);
                    return Ok(Self::hold_text_stream_permit(
                        source,
                        permit,
                        provider.get_model().to_string(),
                        usage_tag.to_string(),
                        task_type.to_string(),
                    ));
                }
                Err(error) => {
                    let failover = crate::providers::transport::may_failover(&error);
                    self.route_failure(
                        task_type,
                        candidate,
                        &error,
                        character_id,
                        failover && index + 1 < candidates.len(),
                    );
                    last_error = error;
                    if !failover {
                        break;
                    }
                }
            }
        }
        self.emit_llm_error_toast(task_type, &last_error.to_string(), &endpoint, character_id);
        Err(last_error)
    }
}

#[cfg(test)]
mod conversational_penalty_tests {
    use super::is_conversational_task;

    /// 惩罚参数只应注入"角色正在说话"的任务。
    ///
    /// 正例与 `generation.rs::build_chat_request` 注入响应 Schema 的集合一致
    /// ——`reasoning` 是携带工具定义时由 `chat` 升级而来。
    #[test]
    fn only_spoken_output_tasks_get_penalties() {
        for task in [crate::providers::base::TASK_COMPANION, "chat", "pet_reaction"] {
            assert!(is_conversational_task(task), "{task} 应注入采样惩罚");
        }
    }

    /// 反例守卫：结构化抽取与工作智能体任务绝不能拿到惩罚参数。
    ///
    /// - `work_agent`：编程任务要确定性，惩罚只会让代码措辞发散
    /// - `reflection` / `consolidation` / `memory` / `auto_extract`：输出是 JSON，
    ///   抑制重复没有收益，反而可能扰动字段复现的稳定性
    /// - `emotion_analysis` / `inner_monologue`：短分类输出，同样无收益
    #[test]
    fn structured_and_work_tasks_are_excluded() {
        for task in [
            "work_agent",
            "reasoning",
            "vision_describe",
            "context_compress",
            "reflection",
            "consolidation",
            "memory",
            "auto_extract",
            "inner_monologue",
            "emotion_analysis",
            "translation",
            "query_rewrite",
            "farewell",
        ] {
            assert!(!is_conversational_task(task), "{task} 不应注入采样惩罚");
        }
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct MockProvider {
        model: &'static str,
        events: Vec<StreamEvent>,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl BaseProvider for MockProvider {
        async fn call_chat(&self, _: Vec<ChatMessage>) -> VivianResult<String> {
            Ok(self.model.into())
        }
        async fn call_stream_chat(
            &self,
            _: Vec<ChatMessage>,
            _: Option<serde_json::Value>,
        ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = mpsc::channel(16);
            for event in &self.events {
                tx.send(event.clone()).await.unwrap();
            }
            Ok(rx)
        }
        fn get_model(&self) -> &str {
            self.model
        }
        fn get_circuit_breaker_stats(&self) -> serde_json::Value {
            json!({})
        }
    }

    fn router(
        primary: Vec<StreamEvent>,
        fallback: Vec<StreamEvent>,
    ) -> (ModelRouter, Arc<AtomicUsize>) {
        let mut router = ModelRouter::new(&AppConfig::default()).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        router.main_provider = Arc::new(Some(Box::new(MockProvider {
            model: "fallback",
            events: fallback,
            calls: calls.clone(),
        })));
        router.task_providers = Arc::new(HashMap::from([(
            "chat".into(),
            Box::new(MockProvider {
                model: "primary",
                events: primary,
                calls: Arc::new(AtomicUsize::new(0)),
            }) as Box<dyn BaseProvider>,
        )]));
        router.enable_routing_matrix = true;
        router.strict_broken = Arc::new(RwLock::new(HashSet::new()));
        (router, calls)
    }

    #[test]
    fn companion_missing_route_uses_main_and_explicit_route_wins() {
        let (mut router, _) = router(vec![], vec![]);
        let task = crate::providers::base::TASK_COMPANION;
        assert_eq!(router.resolve_provider(task).unwrap().get_model(), "fallback");
        router.task_providers = Arc::new(HashMap::from([(task.into(), Box::new(MockProvider {
            model: "character", events: vec![], calls: Arc::new(AtomicUsize::new(0)),
        }) as Box<dyn BaseProvider>)]));
        assert_eq!(router.resolve_provider(task).unwrap().get_model(), "character");
    }

    #[tokio::test]
    async fn early_stream_error_falls_back_but_late_error_never_replays_generation() {
        let error = StreamEvent::Error {
            message: "upstream unavailable".into(),
        };
        let text = StreamEvent::Text {
            content: "你好😀".into(),
        };
        let (router, calls) = router(vec![error.clone()], vec![text.clone()]);
        let mut rx = router
            .query_stream("chat", vec![], None, "", "test")
            .await
            .unwrap();
        assert!(
            matches!(rx.recv().await, Some(StreamEvent::Text { content }) if content == "你好😀")
        );
        assert!(rx.recv().await.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let (router, calls) = self::router(vec![text, error], vec![]);
        let mut rx = router
            .query_stream("chat", vec![], None, "", "test")
            .await
            .unwrap();
        assert!(matches!(rx.recv().await, Some(StreamEvent::Text { .. })));
        assert!(matches!(rx.recv().await, Some(StreamEvent::Error { .. })));
        assert!(rx.recv().await.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn empty_done_is_not_success_and_leading_usage_is_replayed_once() {
        let (tx, rx) = mpsc::channel(4);
        tx.send(StreamEvent::Done {
            finish_reason: None,
        })
        .await
        .unwrap();
        drop(tx);
        assert!(ModelRouter::prime_stream(rx).await.is_err());
        let (tx, rx) = mpsc::channel(4);
        tx.send(StreamEvent::Usage {
            input_tokens: 2,
            output_tokens: 1,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        })
        .await
        .unwrap();
        tx.send(StreamEvent::Text {
            content: "ok".into(),
        })
        .await
        .unwrap();
        drop(tx);
        let mut rx = ModelRouter::prime_stream(rx).await.unwrap();
        assert!(matches!(rx.recv().await, Some(StreamEvent::Usage { .. })));
        assert!(matches!(rx.recv().await, Some(StreamEvent::Text { .. })));
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn strict_downgrade_is_local_to_actual_candidate_and_clears_outer_schema() {
        let (router, _) = router(vec![], vec![]);
        let provider = router.main_provider.as_ref().as_ref().unwrap();
        let schema = json!({"type":"object"});
        let mut attempts = 0;
        scope_provider_call(
            ProviderCallOptions {
                json_schema: Some(Arc::new(schema.clone())),
                ..Default::default()
            },
            router.with_candidate_schema(provider, Some(schema), |_| {
                attempts += 1;
                let attempt = attempts;
                async move {
                    if attempt == 1 {
                        assert!(ProviderCallOptions::current_json_schema().is_some());
                        Err(VivianError::ProviderHttp {
                            status: 400,
                            message: "unsupported responseSchema".into(),
                            retry_after_secs: None,
                        })
                    } else {
                        assert!(ProviderCallOptions::current_json_schema().is_none());
                        Ok(())
                    }
                }
            }),
        )
        .await
        .unwrap();
        assert_eq!(attempts, 2);
        assert!(router.strict_broken.read().contains("fallback"));
        assert!(!router.strict_broken.read().contains("primary"));
    }

    #[tokio::test]
    async fn stalled_stream_cancellation_releases_concurrency_slot_immediately() {
        let semaphore = Arc::new(Semaphore::new(1));
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let (tx, source) = mpsc::channel(1);
        let rx = ModelRouter::hold_text_stream_permit(
            source,
            Some(permit),
            "test".into(),
            "test".into(),
            "test".into(),
        );
        drop(rx);
        tokio::time::timeout(std::time::Duration::from_secs(1), tx.closed())
            .await
            .unwrap();
        let _permit = tokio::time::timeout(std::time::Duration::from_secs(1), semaphore.acquire())
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn invalid_schema_does_not_poison_model_capability_cache() {
        let (router, _) = router(vec![], vec![]);
        let provider = router.main_provider.as_ref().as_ref().unwrap();
        let mut attempts = 0;
        router
            .with_candidate_schema(provider, Some(json!({"type":"object"})), |_| {
                attempts += 1;
                std::future::ready(if attempts == 1 {
                    Err(VivianError::ProviderHttp {
                        status: 400,
                        message: "invalid schema: missing required field".into(),
                        retry_after_secs: None,
                    })
                } else {
                    Ok(())
                })
            })
            .await
            .unwrap();
        assert_eq!(attempts, 2);
        assert!(router.strict_broken.read().is_empty());
    }
}


#[cfg(test)]
mod task_contract_tests {
    use super::*;
    use parking_lot::Mutex;

    #[derive(Clone)]
    struct Probe {
        model: String,
        failed: bool,
        calls: Arc<Mutex<Vec<(String, Vec<ChatMessage>, Option<bool>)>>>,
    }
    impl Probe {
        fn record(&self, messages: Vec<ChatMessage>) -> VivianResult<String> {
            self.calls.lock().push((self.model.clone(), messages,
                ProviderCallOptions::current().include_framework_instructions));
            if self.failed { return Err(VivianError::Provider("upstream unavailable".into())); }
            Ok(self.model.clone())
        }
        fn stream(&self, messages: Vec<ChatMessage>) -> VivianResult<mpsc::Receiver<StreamEvent>> {
            let content = self.record(messages)?;
            let (tx, rx) = mpsc::channel(2);
            tx.try_send(StreamEvent::Text { content }).unwrap();
            tx.try_send(StreamEvent::Done { finish_reason: None }).unwrap();
            Ok(rx)
        }
    }
    #[async_trait::async_trait]
    impl BaseProvider for Probe {
        async fn call_chat(&self, messages: Vec<ChatMessage>) -> VivianResult<String> { self.record(messages) }
        async fn call_stream_chat(&self, messages: Vec<ChatMessage>, _: Option<serde_json::Value>) -> VivianResult<mpsc::Receiver<StreamEvent>> { self.stream(messages) }
        async fn invoke(&self, messages: Vec<ChatMessage>) -> VivianResult<ChatResponse> { self.record(messages).map(ChatResponse::from_text) }
        async fn stream_with_tools(&self, messages: Vec<ChatMessage>, _: Vec<ToolDefinition>) -> VivianResult<mpsc::Receiver<StreamEvent>> { self.stream(messages) }
        fn bind_tools(&self, _: Vec<ToolDefinition>) -> VivianResult<Box<dyn BaseProvider>> { Ok(Box::new(self.clone())) }
        fn supports_native_function_calling(&self) -> bool { true }
        fn get_model(&self) -> &str { &self.model }
        fn get_circuit_breaker_stats(&self) -> serde_json::Value { json!({}) }
    }
    fn fixture() -> (ModelRouter, Probe) {
        let mut router = ModelRouter::new(&AppConfig::default()).unwrap();
        let probe = Probe { model: "main".into(), failed: false, calls: Arc::new(Mutex::new(vec![])) };
        router.main_provider = Arc::new(Some(Box::new(probe.clone())));
        router.task_providers = Arc::new(super::super::task_catalog::TASK_ROUTES.iter().map(|spec| {
            let mut provider = probe.clone(); provider.model = spec.id.clone();
            (spec.id.clone(), Box::new(provider) as Box<dyn BaseProvider>)
        }).collect());
        router.enable_routing_matrix = true;
        (router, probe)
    }
    fn request(task: &str) -> LLMRequest {
        LLMRequest::new(task, vec![ChatMessage::system("Caller-specific output protocol"), ChatMessage::user("input")])
    }
    async fn drain(mut rx: mpsc::Receiver<StreamEvent>, expected: &str) {
        let mut text = String::new();
        while let Some(event) = rx.recv().await { if let StreamEvent::Text { content } = event { text.push_str(&content); } }
        assert_eq!(text, expected);
    }
    #[tokio::test]
    async fn every_task_reaches_its_provider_with_a_distinct_prompt_in_all_four_modes() {
        let (router, probe) = fixture();
        for spec in super::super::task_catalog::TASK_ROUTES.iter() {
            assert_eq!(router.generate(request(&spec.id)).await.unwrap(), spec.id);
            drain(router.generate_stream(request(&spec.id)).await.unwrap(), &spec.id).await;
            let tools = vec![ToolDefinition { name: "probe".into(), description: "test".into(), parameters: json!({"type":"object","properties":{}}) }];
            let mut req = request(&spec.id); req.tools = tools.clone();
            assert_eq!(router.generate_with_tools(req).await.unwrap().content, spec.id);
            let mut req = request(&spec.id); req.tools = tools;
            drain(router.generate_stream_with_tools(req).await.unwrap(), &spec.id).await;
        }
        let captures = probe.calls.lock();
        assert_eq!(captures.len(), super::super::task_catalog::TASK_ROUTES.len() * 4);
        for (model, messages, framework) in captures.iter() {
            assert_eq!(framework, &Some(false));
            let contract = super::super::task_catalog::prompt(super::super::task_catalog::find(model).unwrap());
            assert_eq!(messages[0].content, contract);
            assert_eq!(messages[1].content, "Caller-specific output protocol");
            assert_eq!(messages[2].content, "input");
        }
    }
    #[tokio::test]
    async fn inherited_routes_and_failed_providers_preserve_the_logical_task_prompt() {
        let (mut router, probe) = fixture();
        let mut memory = probe.clone(); memory.model = "memory".into();
        let mut companion = probe.clone(); companion.model = "companion".into(); companion.failed = true;
        router.task_providers = Arc::new(HashMap::from([
            ("memory".into(), Box::new(memory) as Box<dyn BaseProvider>),
            ("companion".into(), Box::new(companion) as Box<dyn BaseProvider>),
        ]));
        assert_eq!(router.generate(request("query_rewrite")).await.unwrap(), "memory");
        assert_eq!(router.generate(request("context_compress")).await.unwrap(), "memory");
        assert_eq!(router.generate(request("pet_reaction")).await.unwrap(), "main");
        let calls = probe.calls.lock();
        assert_eq!(calls.len(), 4);
        for (i, task) in ["query_rewrite", "context_compress", "pet_reaction", "pet_reaction"].iter().enumerate() {
            assert_eq!(calls[i].1[0].content, super::super::task_catalog::prompt(super::super::task_catalog::find(task).unwrap()));
            assert_eq!(calls[i].2, Some(false));
        }
    }
    #[tokio::test]
    async fn disabled_matrix_uses_main_and_unknown_task_is_rejected() {
        let (mut router, probe) = fixture(); router.enable_routing_matrix = false;
        assert_eq!(router.generate(request("reflection")).await.unwrap(), "main");
        assert!(router.generate(request("refelction")).await.is_err());
        assert_eq!(probe.calls.lock().len(), 1);
    }
    #[tokio::test]
    async fn work_override_never_intercepts_companion_or_read_only_planning() {
        let (router, probe) = fixture();
        let mut work = probe.clone(); work.model = "work_override".into();
        router.set_reasoning_override(Some(Box::new(work)));
        assert_eq!(router.generate(request("work_agent")).await.unwrap(), "work_override");
        assert_eq!(router.generate(request("reasoning")).await.unwrap(), "reasoning");
        assert_eq!(router.generate(request("companion")).await.unwrap(), "companion");
        assert_eq!(router.generate(request("tool_execution")).await.unwrap(), "tool_execution");
        let captures = probe.calls.lock();
        for (i, task) in ["work_agent", "reasoning", "companion", "tool_execution"].iter().enumerate() {
            assert_eq!(captures[i].1[0].content, super::super::task_catalog::prompt(super::super::task_catalog::find(task).unwrap()));
        }
    }

    #[tokio::test]
    async fn explicit_specialist_configuration_prevents_generic_judge_shortcuts() {
        let (router, probe) = fixture();
        assert!(router.choose_simple_for("intent_judge", json!({}), "question", &[("yes", "yes"), ("no", "no")], "").await.is_none());
        assert!(router.classify_emotion_simple("input", &["neutral"]).await.is_none());
        assert!(probe.calls.lock().is_empty());
    }
}
