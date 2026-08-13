use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio::sync::Semaphore;

use crate::config::manager::AppConfig;
use crate::error::{VivianError, VivianResult};
use crate::providers::base::{
    BaseProvider, ChatResponse, LLMRequest, StreamEvent, ToolDefinition,
};
use crate::providers::factory::{create_task_provider, ClientCache};
use crate::resilience::{classify_llm_error_from_str, error_kind_to_message_key, LlmErrorKind};
use crate::types::response::ChatMessage;

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
    ///   / translation / bystander_judge / intent_judge → 2 并发（辅助后台任务）
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
    strict_broken: Arc<AtomicBool>,
}

/// 任务 → 信号量分组的并发上限
const SEMAPHORE_GROUP_CHAT_REASONING: usize = 3;
const SEMAPHORE_GROUP_MEMORY_REFLECTION: usize = 3;
const SEMAPHORE_GROUP_AUXILIARY: usize = 2;

/// LLM 错误 toast 冷却时间：同类错误在此时间内不重复弹窗
const ERROR_TOAST_COOLDOWN_SECS: u64 = 60;

/// 路由回退事件冷却时间：同一任务在此时间内的回退不重复发 toast
const ROUTE_FALLBACK_COOLDOWN_SECS: u64 = 120;

/// 解析任务类型对应的信号量分组名
///
/// 返回 (组名, 并发上限)，由 `ModelRouter::new` 在构造时据此创建 Semaphore
fn semaphore_for_task(task_type: &str) -> (&'static str, usize) {
    match task_type {
        "chat" | "reasoning" | "vision_describe" => {
            ("chat_reasoning", SEMAPHORE_GROUP_CHAT_REASONING)
        }
        "memory" | "reflection" | "consolidation" => {
            ("memory_reflection", SEMAPHORE_GROUP_MEMORY_REFLECTION)
        }
        "emotion_analysis"
        | "inner_monologue"
        | "diary"
        | "knowledge_acquisition"
        | "translation"
        | "bystander_judge"
        | "intent_judge" => ("auxiliary", SEMAPHORE_GROUP_AUXILIARY),
        _ => ("chat_reasoning", SEMAPHORE_GROUP_CHAT_REASONING),
    }
}

/// 持久化 strict 熔断模型名：避免每次重启对不支持 schema 的模型白跑一次 400
fn strict_broken_path() -> std::path::PathBuf {
    crate::utils::path::get_user_data_dir().join("strict_broken_model")
}

fn load_strict_broken_model() -> Option<String> {
    std::fs::read_to_string(strict_broken_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn save_strict_broken_model(model: &str) {
    let _ = std::fs::write(strict_broken_path(), model);
}

impl ModelRouter {
    pub fn new(config: &AppConfig) -> VivianResult<Self> {
        let client_cache: ClientCache = Arc::new(RwLock::new(HashMap::new()));

        // 1. 主 LLM API provider（来自 config.ai）
        //    三项字段（api_key / endpoint / model）任一为空则视为未配置
        let main_provider: Option<Box<dyn BaseProvider>> = {
            let api_key = config.ai.api_key.as_deref().unwrap_or("").trim();
            let endpoint = config.ai.endpoint.as_deref().unwrap_or("").trim();
            let model = config.ai.model.trim();
            if api_key.is_empty() || endpoint.is_empty() || model.is_empty() {
                tracing::warn!(
                    "[ModelRouter] 主 LLM API 未配置完整（api_key/endpoint/model 任一为空），跳过创建 main_provider"
                );
                None
            } else {
                let task_config = crate::config::manager::TaskRouteConfig {
                    provider_type: config.ai.provider.clone(),
                    model: model.to_string(),
                    api_key: api_key.to_string(),
                    endpoint: endpoint.to_string(),
                    api_secret: config.ai.api_secret.clone().unwrap_or_default(),
                    app_id: config.ai.app_id.clone().unwrap_or_default(),
                    temperature: None,
                    max_tokens: None,
                };
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
        if config.enable_routing_matrix {
            for (task_type, task_config) in &config.routing_matrix {
                // 跳过空配置（未填写 model 或 endpoint 的任务）→ 由主 API 兜底
                if task_config.model.trim().is_empty()
                    || task_config.endpoint.trim().is_empty()
                    || task_config.api_key.trim().is_empty()
                {
                    tracing::info!(
                        "[ModelRouter] 任务 {} 未配置完整，回退到主 LLM API",
                        task_type
                    );
                    continue;
                }
                match create_task_provider(task_config, config, &client_cache) {
                    Ok(provider) => {
                        tracing::info!(
                            "[ModelRouter] 任务 {} 绑定模型 {} @ {}",
                            task_type,
                            task_config.model,
                            task_config.endpoint
                        );
                        task_providers.insert(task_type.clone(), provider);
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

        Ok(Self {
            main_provider: Arc::new(main_provider),
            task_providers: Arc::new(task_providers),
            enable_routing_matrix: config.enable_routing_matrix,
            enable_search: Arc::new(AtomicBool::new(false)),
            client_cache,
            app_handle: Arc::new(RwLock::new(None)),
            emit_enabled: Arc::new(AtomicBool::new(false)),
            semaphores: Arc::new(Self::build_semaphores()),
            error_toast_cooldown: Arc::new(RwLock::new(HashMap::new())),
            route_fallback_cooldown: Arc::new(RwLock::new(HashMap::new())),
            uses_proxy: config.network.proxy_mode != "direct",
            strict_broken: Arc::new(AtomicBool::new(
                load_strict_broken_model().as_deref() == Some(config.ai.model.trim()),
            )),
        })
    }

    /// 当前是否走代理链路（基于 `config.network.proxy_mode`）
    ///
    /// `direct` 为直连，`system`/`custom`/`manual` 均视为走代理。
    /// 辅助任务可据此延长超时（代理链路通常比直连慢）。
    pub fn uses_proxy(&self) -> bool {
        self.uses_proxy
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

    /// 对某任务的 provider 设置 max_tokens 运行时覆盖（凝神模式激活时调用）。
    ///
    /// 按路由顺序匹配首个可用 provider（task_providers → main_provider）。
    /// `extra_tokens` 为 0 时等价于清除覆盖。
    pub fn set_focus_boost(&self, task_type: &str, extra_tokens: u32) {
        let target = self.task_providers.get(task_type);
        let provider: &dyn BaseProvider = match target {
            Some(p) => p.as_ref(),
            None => match self.main_provider.as_ref() {
                Some(p) => p.as_ref(),
                None => {
                    tracing::warn!(
                        "[ModelRouter] set_focus_boost 无可用 provider，跳过 (task={})",
                        task_type
                    );
                    return;
                }
            },
        };
        provider.set_max_tokens_override(extra_tokens);
        if extra_tokens > 0 {
            tracing::debug!(
                "[ModelRouter] 凝神模式激活：task={} max_tokens 额外余量={}",
                task_type,
                extra_tokens
            );
        }
    }

    /// 清除某任务 provider 的 max_tokens 覆盖（凝神模式退出后调用）。
    pub fn clear_focus_boost(&self, task_type: &str) {
        self.set_focus_boost(task_type, 0);
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
    fn emit_route_fallback(&self, task_type: &str, error: &str) {
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
    fn emit_llm_error_toast(&self, task_type: &str, error: &str) {
        let error_kind = classify_llm_error_from_str(error);

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
        let handle_guard = self.app_handle.read();
        if let Some(handle) = handle_guard.as_ref() {
            let _ = handle.emit(
                "llm:error",
                json!({
                    "task_type": task_type,
                    "error": error,
                    "error_kind": error_kind,
                    "message_key": message_key,
                }),
            );
        }
    }

    async fn query_with_fallback(
        &self,
        task_type: &str,
        messages: Vec<ChatMessage>,
        json_schema: Option<serde_json::Value>,
    ) -> VivianResult<String> {
        Self::log_llm_request(task_type, &messages, &[]);
        // 按任务分组获取并发信号量，acquire 后才执行（防止后处理 LLM 同时挤占主对话）
        // 信号量在 _permit 作用域结束时自动释放
        let sem_opt = self.get_semaphore(task_type);
        let _permit = if let Some(sem) = &sem_opt {
            Some(
                sem.acquire()
                    .await
                    .map_err(|e| VivianError::Provider(format!("获取并发信号量失败: {}", e)))?,
            )
        } else {
            None
        };

        let mut last_error: Option<VivianError> = None;
        let enable_search = self.is_enable_search();

        // 1. 路由矩阵启用时优先使用任务专属 provider；失败则通知前端并回退到主 API
        if self.enable_routing_matrix {
            if let Some(provider) = self.task_providers.get(task_type) {
                tracing::debug!(
                    "[ModelRouter] 路由任务 {} 到专属 provider ({})",
                    task_type,
                    provider.get_model()
                );
                match provider
                    .call_chat_with_search(messages.clone(), enable_search, json_schema.clone())
                    .await
                {
                    Ok(result) => {
                        tracing::debug!(
                            "[LLM-IO] <<< task={} response={:?}",
                            task_type,
                            if result.len() > 4000 {
                                format!("{}...<truncated {}>", &result[..4000], result.len() - 4000)
                            } else {
                                result.clone()
                            }
                        );
                        self.emit_route_status(task_type, "ok");
                        return Ok(result);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[ModelRouter] 任务 {} 专属 provider 失败，回退到主 LLM API: {}",
                            task_type,
                            e
                        );
                        self.emit_route_status(task_type, "error");
                        self.emit_route_fallback(task_type, &e.to_string());
                        last_error = Some(e);
                    }
                }
            }
        }

        // 2. 主 LLM API
        if let Some(provider) = self.main_provider.as_ref() {
            tracing::debug!(
                "[ModelRouter] 任务 {} 使用主 LLM API ({})",
                task_type,
                provider.get_model()
            );
            match provider
                .call_chat_with_search(messages.clone(), enable_search, json_schema.clone())
                .await
            {
                Ok(result) => {
                    tracing::debug!(
                        "[LLM-IO] <<< task={} response={:?}",
                        task_type,
                        if result.len() > 4000 {
                            format!("{}...<truncated {}>", &result[..4000], result.len() - 4000)
                        } else {
                            result.clone()
                        }
                    );
                    // 回退主 LLM 成功，恢复绿色状态
                    if last_error.is_some() {
                        self.emit_route_status(task_type, "ok");
                    }
                    return Ok(result);
                }
                Err(e) => {
                    tracing::warn!(
                        "[ModelRouter] 主 LLM API 失败: {}",
                        e
                    );
                    self.emit_route_status(task_type, "error");
                    last_error = Some(e);
                }
            }
        }

        let err = last_error
            .unwrap_or_else(|| VivianError::Provider("没有可用的提供商".to_string()));
        self.emit_llm_error_toast(task_type, &err.to_string());
        Err(err)
    }

    async fn query_stream(
        &self,
        task_type: &str,
        messages: Vec<ChatMessage>,
        json_schema: Option<serde_json::Value>,
    ) -> VivianResult<mpsc::Receiver<String>> {
        Self::log_llm_request(task_type, &messages, &[]);
        // 按任务分组获取并发信号量（流式任务在 receiver 创建后即视为"已建立"，
        // permit 在函数返回时释放；后续流式数据由调用方独立消费）
        let sem_opt = self.get_semaphore(task_type);
        let _permit = if let Some(sem) = &sem_opt {
            Some(
                sem.acquire()
                    .await
                    .map_err(|e| VivianError::Provider(format!("获取并发信号量失败: {}", e)))?,
            )
        } else {
            None
        };

        // 1. 路由矩阵启用时优先使用任务专属 provider
        if self.enable_routing_matrix {
            if let Some(provider) = self.task_providers.get(task_type) {
                tracing::debug!(
                    "[ModelRouter] 路由流式任务 {} 到专属 provider ({})",
                    task_type,
                    provider.get_model()
                );
                match provider.call_stream_chat(messages, json_schema).await {
                    Ok(rx) => {
                        self.emit_route_status(task_type, "ok");
                        return Ok(rx);
                    }
                    Err(e) => {
                        self.emit_route_status(task_type, "error");
                        return Err(e);
                    }
                }
            }
        }

        // 2. 回退到主 LLM API
        if let Some(provider) = self.main_provider.as_ref() {
            tracing::debug!(
                "[ModelRouter] 流式任务 {} 使用主 LLM API ({})",
                task_type,
                provider.get_model()
            );
            return provider.call_stream_chat(messages, json_schema).await;
        }

        Err(VivianError::Provider("没有可用的流式提供商".to_string()))
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
        if let Some(p) = self.resolve_provider(task_type) {
            return p.supports_native_function_calling();
        }
        false
    }

    /// 当前 chat 任务的 provider 是否支持原生 JSON Schema 约束
    pub fn supports_structured_output(&self) -> bool {
        if let Some(p) = self.resolve_provider("chat") {
            return p.supports_structured_output();
        }
        false
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
    /// `enable_search` 通过临时切换 router 全局开关实现(调用后恢复)。
    pub async fn generate(&self, mut request: LLMRequest) -> VivianResult<String> {
        loop {
            let LLMRequest {
                task_type,
                messages,
                stream,
                enable_search,
                tools,
                json_schema,
                temperature_override,
                ..
            } = request.clone();
            // tools 非空应走 generate_with_tools,这里防御性检查
            if !tools.is_empty() {
                return Err(VivianError::Provider(
                    "generate() 不支持 tools 非空,请用 generate_with_tools()".to_string(),
                ));
            }
            // 临时切换联网搜索开关
            let original_search = self.is_enable_search();
            if enable_search != original_search {
                self.set_enable_search(enable_search);
            }
            // 应用请求级 temperature 覆盖（调用后恢复，避免污染其他并发调用）
            let temp_applied = temperature_override.is_some();
            if temp_applied {
                self.set_temperature_override(temperature_override);
            }
            // strict 熔断后强制降级为无 schema
            let effective_schema = if self.strict_broken.load(Ordering::Relaxed) {
                None
            } else {
                json_schema.clone()
            };
            let result = if stream {
                // 流式:累积所有 chunk 返回完整文本
                let mut rx = self.query_stream(&task_type, messages, effective_schema).await?;
                let mut buf = String::new();
                while let Some(chunk) = rx.recv().await {
                    buf.push_str(&chunk);
                }
                Ok(buf)
            } else {
                self.query_with_fallback(&task_type, messages, effective_schema).await
            };
            if enable_search != original_search {
                self.set_enable_search(original_search);
            }
            if temp_applied {
                self.set_temperature_override(None);
            }
            // strict 拒绝检测:熔断后重试(不带 schema)
            if let Err(ref e) = result {
                if json_schema.is_some() && self.handle_strict_failure(e) {
                    request.json_schema = None;
                    tracing::info!("[ModelRouter] strict 熔断，重试 generate(无 schema)");
                    continue;
                }
            }
            return result;
        }
    }

    /// 统一流式文本生成入口(无工具)
    ///
    /// 返回 chunk Receiver,调用方自行累积。
    pub async fn generate_stream(
        &self,
        mut request: LLMRequest,
    ) -> VivianResult<tokio::sync::mpsc::Receiver<String>> {
        loop {
            let LLMRequest {
                task_type,
                messages,
                enable_search,
                tools,
                json_schema,
                stream: _,
                ..
            } = request.clone();
            if !tools.is_empty() {
                return Err(VivianError::Provider(
                    "generate_stream() 不支持 tools 非空,请用 generate_stream_with_tools()".to_string(),
                ));
            }
            let original_search = self.is_enable_search();
            if enable_search != original_search {
                self.set_enable_search(enable_search);
            }
            let effective_schema = if self.strict_broken.load(Ordering::Relaxed) {
                None
            } else {
                json_schema.clone()
            };
            let rx = self.query_stream(&task_type, messages, effective_schema).await;
            if enable_search != original_search {
                self.set_enable_search(original_search);
            }
            // strict 拒绝检测:仅在流未开始时(返回 Err)可重试;流已开始则无法重试
            if let Err(ref e) = rx {
                if json_schema.is_some() && self.handle_strict_failure(e) {
                    request.json_schema = None;
                    tracing::info!("[ModelRouter] strict 熔断，重试 generate_stream(无 schema)");
                    continue;
                }
            }
            return rx;
        }
    }

    /// 统一工具调用入口(原生 function calling 非流式)
    ///
    /// 内部转调 `query_with_tools`。调用方应先通过 `supports_native_function_calling`
    /// 确认 provider 支持,否则应回退到文本路径。
    pub async fn generate_with_tools(
        &self,
        mut request: LLMRequest,
    ) -> VivianResult<ChatResponse> {
        loop {
            let LLMRequest {
                task_type,
                messages,
                tools,
                enable_search,
                json_schema,
                stream: _,
                ..
            } = request.clone();
            let original_search = self.is_enable_search();
            if enable_search != original_search {
                self.set_enable_search(enable_search);
            }
            let result = self.query_with_tools(&task_type, messages, tools).await;
            if enable_search != original_search {
                self.set_enable_search(original_search);
            }
            // strict 拒绝检测:熔断后重试(不带 schema)
            if let Err(ref e) = result {
                if json_schema.is_some() && self.handle_strict_failure(e) {
                    request.json_schema = None;
                    tracing::info!("[ModelRouter] strict 熔断，重试 generate_with_tools(无 schema)");
                    continue;
                }
            }
            return result;
        }
    }

    /// 统一工具调用入口(原生 function calling 流式)
    ///
    /// 返回 StreamEvent Receiver,调用方按事件类型累积文本/工具调用。
    pub async fn generate_stream_with_tools(
        &self,
        mut request: LLMRequest,
    ) -> VivianResult<tokio::sync::mpsc::Receiver<crate::providers::base::StreamEvent>> {
        loop {
            let LLMRequest {
                task_type,
                messages,
                tools,
                enable_search,
                json_schema,
                stream: _,
                ..
            } = request.clone();
            let original_search = self.is_enable_search();
            if enable_search != original_search {
                self.set_enable_search(enable_search);
            }
            let rx = self.query_stream_with_tools(&task_type, messages, tools).await;
            if enable_search != original_search {
                self.set_enable_search(original_search);
            }
            // strict 拒绝检测:仅在流未开始时(返回 Err)可重试;流已开始则无法重试
            if let Err(ref e) = rx {
                if json_schema.is_some() && self.handle_strict_failure(e) {
                    request.json_schema = None;
                    tracing::info!("[ModelRouter] strict 熔断，重试 generate_stream_with_tools(无 schema)");
                    continue;
                }
            }
            return rx;
        }
    }

    /// 解析任务的当前 provider（按路由顺序：task_providers → main）
    ///
    /// 返回的 `Option<&Box<dyn BaseProvider>>` 借用自 self，调用方需在调用前
    /// 完成所有路由决策（避免运行时 provider 切换）。
    fn resolve_provider(&self, task_type: &str) -> Option<&Box<dyn BaseProvider>> {
        if self.enable_routing_matrix {
            if let Some(p) = self.task_providers.get(task_type) {
                return Some(p);
            }
        }
        if let Some(p) = self.main_provider.as_ref() {
            return Some(p);
        }
        None
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
    ) -> VivianResult<ChatResponse> {
        Self::log_llm_request(task_type, &messages, &tools);
        // 按任务分组获取并发信号量
        let sem_opt = self.get_semaphore(task_type);
        let _permit = if let Some(sem) = &sem_opt {
            Some(
                sem.acquire()
                    .await
                    .map_err(|e| VivianError::Provider(format!("获取并发信号量失败: {}", e)))?,
            )
        } else {
            None
        };

        let mut last_error: Option<VivianError> = None;

        // 1. 路由矩阵启用时优先用任务专属 provider
        if self.enable_routing_matrix {
            if let Some(provider) = self.task_providers.get(task_type) {
                if provider.supports_native_function_calling() {
                    tracing::debug!(
                        "[ModelRouter] 路由任务 {} (native fc) 到专属 provider ({})",
                        task_type,
                        provider.get_model()
                    );
                    match Self::invoke_with_tools(provider, messages.clone(), tools.clone()).await {
                        Ok(resp) => {
                            Self::log_llm_response(task_type, &resp);
                            self.emit_route_status(task_type, "ok");
                            return Ok(resp);
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[ModelRouter] 任务 {} 专属 provider native fc 失败，回退到主 LLM API: {}",
                                task_type,
                                e
                            );
                            self.emit_route_status(task_type, "error");
                            self.emit_route_fallback(task_type, &e.to_string());
                            last_error = Some(e);
                        }
                    }
                } else {
                    tracing::debug!(
                        "[ModelRouter] 任务 {} 专属 provider 不支持 native fc，跳过",
                        task_type
                    );
                }
            }
        }

        // 2. 主 LLM API
        if let Some(provider) = self.main_provider.as_ref() {
            if provider.supports_native_function_calling() {
                tracing::debug!(
                    "[ModelRouter] 任务 {} (native fc) 使用主 LLM API ({})",
                    task_type,
                    provider.get_model()
                );
                match Self::invoke_with_tools(provider, messages.clone(), tools.clone()).await {
                    Ok(resp) => {
                        Self::log_llm_response(task_type, &resp);
                        // 回退主 LLM 成功，恢复绿色状态
                        if last_error.is_some() {
                            self.emit_route_status(task_type, "ok");
                        }
                        return Ok(resp);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[ModelRouter] 主 LLM API native fc 失败: {}",
                            e
                        );
                        self.emit_route_status(task_type, "error");
                        last_error = Some(e);
                    }
                }
            }
        }

        let err = last_error.unwrap_or_else(|| {
            VivianError::NotImplemented(format!(
                "任务 {} 没有可用的 provider 支持原生 function calling",
                task_type
            ))
        });
        self.emit_llm_error_toast(task_type, &err.to_string());
        Err(err)
    }

    /// 内部辅助：bind_tools + invoke 的两步组合
    async fn invoke_with_tools(
        provider: &Box<dyn BaseProvider>,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
    ) -> VivianResult<ChatResponse> {
        if tools.is_empty() {
            // 无工具时直接调用 invoke（provider 会回退到 call_chat）
            return provider.invoke(messages).await;
        }
        let bound = provider.bind_tools(tools)?;
        bound.invoke(messages).await
    }

    /// 安全截断字符串到指定字节数以内，确保不切在多字节 UTF-8 字符中间
    fn safe_truncate(s: &str, max_bytes: usize) -> &str {
        if s.len() <= max_bytes {
            return s;
        }
        // 从 max_bytes 向前找最近的 char boundary
        let mut end = max_bytes;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }

    /// LLM 请求/响应日志辅助函数
    ///
    /// 在四个 LLM 入口（query_with_fallback / query_stream / query_with_tools /
    /// query_stream_with_tools）调用前埋点，输出 task_type、消息序列（role + 截断后的
    /// content）、tools 名称列表，便于从日志直接复现 LLM 看到的真实 prompt。
    /// 截断阈值：system 8000 字符，其他消息 2000 字符。
    fn log_llm_request(task_type: &str, messages: &[ChatMessage], tools: &[ToolDefinition]) {
        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        let lines: Vec<String> = messages
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let cap = if m.role == "system" { 8000 } else { 2000 };
                let preview = if m.content.len() > cap {
                    let truncated = Self::safe_truncate(&m.content, cap);
                    format!("{}...<truncated {}>", truncated, m.content.len() - truncated.len())
                } else {
                    m.content.clone()
                };
                format!("  [{}] role={} content={:?}", i, m.role, preview)
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
        let cap = 4000;
        let preview = if resp.content.len() > cap {
            let truncated = Self::safe_truncate(&resp.content, cap);
            format!("{}...<truncated {}>", truncated, resp.content.len() - truncated.len())
        } else {
            resp.content.clone()
        };
        let tool_calls_preview: Vec<String> = resp
            .tool_calls
            .iter()
            .map(|tc| {
                let args = serde_json::to_string(&tc.arguments).unwrap_or_default();
                let args_cap = 1000;
                let args_preview = if args.len() > args_cap {
                    let truncated = Self::safe_truncate(&args, args_cap);
                    format!("{}...<truncated>", truncated)
                } else {
                    args
                };
                format!("{{name={}, args={}}}", tc.name, args_preview)
            })
            .collect();
        tracing::debug!(
            "[LLM-IO] <<< task={} finish_reason={:?} content={:?} tool_calls=[{}]",
            task_type,
            resp.finish_reason,
            preview,
            tool_calls_preview.join(", ")
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
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        Self::log_llm_request(task_type, &messages, &tools);
        // 按任务分组获取并发信号量
        let sem_opt = self.get_semaphore(task_type);
        let _permit = if let Some(sem) = &sem_opt {
            Some(
                sem.acquire()
                    .await
                    .map_err(|e| VivianError::Provider(format!("获取并发信号量失败: {}", e)))?,
            )
        } else {
            None
        };

        // 1. 路由矩阵启用时优先用任务专属 provider
        if self.enable_routing_matrix {
            if let Some(provider) = self.task_providers.get(task_type) {
                if provider.supports_native_function_calling() {
                    tracing::debug!(
                        "[ModelRouter] 路由流式任务 {} (native fc) 到专属 provider ({})",
                        task_type,
                        provider.get_model()
                    );
                    match Self::stream_with_tools_provider(provider, messages, tools).await {
                        Ok(rx) => {
                            self.emit_route_status(task_type, "ok");
                            return Ok(rx);
                        }
                        Err(e) => {
                            self.emit_route_status(task_type, "error");
                            return Err(e);
                        }
                    }
                }
                tracing::debug!(
                    "[ModelRouter] 任务 {} 专属 provider 不支持 native fc stream",
                    task_type
                );
            }
        }

        // 2. 主 LLM API
        if let Some(provider) = self.main_provider.as_ref() {
            if provider.supports_native_function_calling() {
                tracing::debug!(
                    "[ModelRouter] 流式任务 {} (native fc) 使用主 LLM API ({})",
                    task_type,
                    provider.get_model()
                );
                return Self::stream_with_tools_provider(provider, messages, tools).await;
            }
        }

        Err(VivianError::NotImplemented(format!(
            "任务 {} 没有可用的 provider 支持流式原生 function calling",
            task_type
        )))
    }

    /// 内部辅助：直接调用 provider 的 stream_with_tools
    ///
    /// 与 `invoke_with_tools` 不同，stream_with_tools 直接接受外部 tools 参数，
    /// 不需要 bind_tools 步骤（避免克隆 provider 实例的开销）。
    async fn stream_with_tools_provider(
        provider: &Box<dyn BaseProvider>,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        provider.stream_with_tools(messages, tools).await
    }

    /// 识别 strict schema 拒绝错误
    ///
    /// 各 provider 返回错误格式为 `"XXX API 请求失败 (400): ..."`，
    /// strict 拒绝的响应文本通常包含 schema / response_format / json_schema /
    /// strict / responseSchema 等关键词。
    ///
    /// 匹配条件（同时满足）：
    /// 1. HTTP 400 状态码（错误字符串包含 "400"）
    /// 2. 响应文本包含 schema 相关关键词之一
    fn is_strict_error(err: &VivianError) -> bool {
        let msg = err.to_string();
        // 必须是 400 错误
        if !msg.contains("400") {
            return false;
        }
        // 检查 schema 相关关键词（覆盖 OpenAI / 豆包 / Gemini 的错误信息）
        const SCHEMA_KEYWORDS: &[&str] = &[
            "json_schema",
            "response_format",
            "responseSchema",
            "response_schema",
            "structured output",
            "structured_output",
            "invalid schema",
            "schema validation",
            "strict",
            "$ref",
            "$defs",
        ];
        let lower = msg.to_lowercase();
        SCHEMA_KEYWORDS.iter().any(|kw| lower.contains(kw))
    }

    /// 处理 strict 拒绝：熔断 + 记录日志
    ///
    /// 返回 true 表示已熔断（调用方应重试），false 表示未识别为 strict 错误。
    fn handle_strict_failure(&self, err: &VivianError) -> bool {
        if !Self::is_strict_error(err) {
            return false;
        }
        if self.strict_broken.swap(true, Ordering::Relaxed) {
            // 已经熔断过，不应该再触发（理论上 apply_json_schema 已降级）
            tracing::debug!("[ModelRouter] strict 熔断已生效，但仍有 strict 错误: {}", err);
        } else {
            tracing::warn!(
                "[ModelRouter] 检测到 strict schema 拒绝，熔断并降级到无 schema 路径: {}",
                err
            );
            // 持久化：下次启动若仍用同一模型，直接跳过 schema
            if let Some(provider) = self.main_provider.as_ref() {
                save_strict_broken_model(provider.get_model());
            }
        }
        true
    }
}

