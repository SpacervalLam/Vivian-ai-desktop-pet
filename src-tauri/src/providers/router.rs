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
use crate::providers::base::TASK_WORK_AGENT;
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
    /// 主 LLM API 的 endpoint。LLM 错误 toast 用它把用户引导到对应厂商的控制台
    /// （充值页因厂商而异，前端按 endpoint 反查预设里的 consoleUrl）。
    main_endpoint: String,
    /// 任务专属 provider —— 每个任务独立配置的模型实例
    task_providers: Arc<HashMap<String, Box<dyn BaseProvider>>>,
    /// 任务类型 → 该任务绑定的 provider endpoint。查不到的任务回退 `main_endpoint`
    /// （路由矩阵关闭或任务未配置时全部走主 API）。
    task_endpoints: Arc<HashMap<String, String>>,
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

/// 需要注入惩罚参数的"角色说话"任务类型。
///
/// 与 `generation.rs::build_chat_request` 注入响应 Schema 的集合保持一致——
/// 那三个类型就是角色真正产出台词的路径（`reasoning` 是携带工具定义时
/// 由 `chat` 升级而来，见 `TASK_WORK_AGENT` 的文档）。
///
/// **不含 `work_agent`**：编程任务要的是确定性，惩罚参数只会让代码措辞发散；
/// 也不含 reflection / consolidation / memory 等结构化抽取任务——它们的输出
/// 是 JSON，抑制重复没有意义，反而可能影响字段复现的稳定性。
const CONVERSATIONAL_TASK_TYPES: &[&str] = &["chat", "reasoning", "vision_describe"];

/// 该任务类型是否属于"角色正在说话"（需要注入采样惩罚）。
///
/// 独立成自由函数是为了可测：`ModelRouter` 的构造需要完整 `AppConfig`
/// 与真实网络客户端，而这里的判据本身是纯字符串匹配。
fn is_conversational_task(task_type: &str) -> bool {
    CONVERSATIONAL_TASK_TYPES.contains(&task_type)
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
    match task_type {
        "chat" | "reasoning" | "vision_describe" => {
            ("chat_reasoning", SEMAPHORE_GROUP_CHAT_REASONING)
        }
        // 工作智能体单列一组：它的单次调用耗时长、并发量与陪伴对话不是一个量级，
        // 此前挤在 chat_reasoning 组里会互相抢占额度
        TASK_WORK_AGENT => ("work_agent", SEMAPHORE_GROUP_WORK_AGENT),
        "memory" | "reflection" | "consolidation" => {
            ("memory_reflection", SEMAPHORE_GROUP_MEMORY_REFLECTION)
        }
        "emotion_analysis"
        | "inner_monologue"
        | "diary"
        | "knowledge_acquisition"
        | "translation"
        | "bystander_judge"
        | "intent_judge"
        | "asr_polish"
        | "text_rewrite" => ("auxiliary", SEMAPHORE_GROUP_AUXILIARY),
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
                temperature: None,
                max_tokens: None,
                context_window: None,
                reasoning: None,
            };
            if !provider_configuration_complete(&task_config) {
                tracing::warn!(
                    "[ModelRouter] 主 LLM API 未配置完整，跳过创建 main_provider"
                );
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
        let mut task_reasoning = HashMap::new();
        // 任务 → endpoint 映射：错误 toast 用它引导用户前往对应厂商控制台
        let mut task_endpoints: HashMap<String, String> = HashMap::new();
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
                        task_endpoints.insert(task_type.clone(), cfg.endpoint.clone());
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
            main_endpoint: config.ai.endpoint.as_deref().unwrap_or("").trim().to_string(),
            task_providers: Arc::new(task_providers),
            task_endpoints: Arc::new(task_endpoints),
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
        // 编程输出预算按服务商分级默认，忽略历史保存的 max_tokens：
        // 该字段已从设置界面移除（用户不应关心单次输出上限），
        // 旧版本保存的 2048 等小预算会限制代码生成。
        let mut cfg = cfg.clone();
        cfg.max_tokens = Some(crate::providers::factory::work_model_default_max_tokens(
            &cfg.provider_type,
            &cfg.endpoint,
        ));
        match create_task_provider(&cfg, config, client_cache) {
            Ok(p) => {
                // 工作智能体请求省略 temperature（服务端默认）：
                // 编程任务要确定性，且推理模型对非默认温度敏感
                p.set_omit_temperature(true);
                tracing::info!(
                    "[ModelRouter] 已恢复工作智能体模型覆盖: {} @ {}",
                    cfg.model,
                    cfg.endpoint
                );
                Some(Arc::new(p))
            }
            Err(e) => {
                tracing::warn!(
                    "[ModelRouter] 恢复工作智能体模型 provider 失败: {}",
                    e
                );
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
            self.task_reasoning
                .get(task_type)
                .copied()
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
            "tools": request.tools,
            "json_schema": request.json_schema,
            "stream": request.stream,
        })
        .to_string();
        let (presence_penalty, frequency_penalty) = self.conversational_penalties(request);
        ProviderCallOptions {
            enable_search: Some(request.enable_search),
            temperature: request.temperature_override,
            max_tokens: request.max_tokens_override,
            max_tokens_extra: request.max_tokens_extra,
            presence_penalty,
            frequency_penalty,
            reasoning: Some(self.effective_request_reasoning(
                &request.task_type,
                request.reasoning,
            )),
            json_schema: request.json_schema.clone().map(Arc::new),
            request_fingerprint: Some(crate::utils::fnv1a_64(&fingerprint_source)),
            response_cache_allowed: Some(request.tools.is_empty()),
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
    fn emit_llm_error_toast(&self, task_type: &str, error: &str, endpoint: &str, character_id: &str) {
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

    async fn query_with_fallback(
        &self,
        task_type: &str,
        messages: Vec<ChatMessage>,
        json_schema: Option<serde_json::Value>,
        character_id: &str,
    ) -> VivianResult<String> {
        Self::log_llm_request(task_type, &messages, &[]);
        // 按任务分组获取并发信号量，acquire 后才执行（防止后处理 LLM 同时挤占主对话）
        // 信号量在 _permit 作用域结束时自动释放
        let sem_opt = self.get_semaphore(task_type);
        let _permit = if let Some(sem) = sem_opt {
            Some(
                sem.acquire_owned()
                    .await
                    .map_err(|e| VivianError::Provider(format!("获取并发信号量失败: {}", e)))?,
            )
        } else {
            None
        };

        let mut last_error: Option<VivianError> = None;
        // 跟随 last_error 一起记录：最后实际失败的 provider 的 endpoint，
        // 供 llm:error toast 反查厂商直达控制台。为 None 时前端查不到就不挂动作。
        let mut last_error_endpoint: Option<String> = None;
        let enable_search = self.is_enable_search();

        // 0. 工作智能体覆盖模型优先（仅 reasoning 任务）——用户显式选择，优先级高于路由矩阵
        if let Some(provider) = self.override_provider_for(task_type) {
            tracing::debug!(
                "[ModelRouter] 路由任务 {} 到工作智能体覆盖模型 ({})",
                task_type,
                provider.get_model()
            );
            match provider
                .call_chat_with_search(messages.clone(), enable_search, json_schema.clone())
                .await
            {
                Ok(result) => {
                    Self::log_text_response(task_type, &result);
                    self.emit_route_status(task_type, "ok");
                    return Ok(result);
                }
                Err(e) => {
                    tracing::warn!(
                        "[ModelRouter] 工作智能体覆盖模型失败，回退到默认路由: {}",
                        e
                    );
                    self.emit_route_status(task_type, "error");
                    self.emit_route_fallback(task_type, &e.to_string(), character_id);
                    last_error = Some(e);
                }
            }
        }

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
                        Self::log_text_response(task_type, &result);
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
                        self.emit_route_fallback(task_type, &e.to_string(), character_id);
                        last_error = Some(e);
                        last_error_endpoint = self.task_endpoints.get(task_type).cloned();
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
                    Self::log_text_response(task_type, &result);
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
                    last_error_endpoint = Some(self.main_endpoint.clone());
                }
            }
        }

        let err = last_error
            .unwrap_or_else(|| VivianError::Provider("没有可用的提供商".to_string()));
        self.emit_llm_error_toast(
            task_type,
            &err.to_string(),
            last_error_endpoint.as_deref().unwrap_or(""),
            character_id,
        );
        Err(err)
    }

    async fn query_stream(
        &self,
        task_type: &str,
        messages: Vec<ChatMessage>,
        json_schema: Option<serde_json::Value>,
        character_id: &str,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        Self::log_llm_request(task_type, &messages, &[]);
        // permit 由返回流的转发任务持有，直到流结束或调用方丢弃 receiver。
        let sem_opt = self.get_semaphore(task_type);
        let permit = if let Some(sem) = sem_opt {
            Some(
                sem.acquire_owned()
                    .await
                    .map_err(|e| VivianError::Provider(format!("获取并发信号量失败: {}", e)))?,
            )
        } else {
            None
        };

        // 0. 工作智能体覆盖模型优先（仅 reasoning 任务）——用户显式选择，优先级高于路由矩阵
        if let Some(provider) = self.override_provider_for(task_type) {
            tracing::debug!(
                "[ModelRouter] 路由流式任务 {} 到工作智能体覆盖模型 ({})",
                task_type,
                provider.get_model()
            );
            match provider.call_stream_chat(messages.clone(), json_schema.clone()).await {
                Ok(rx) => {
                    self.emit_route_status(task_type, "ok");
                    return Ok(Self::hold_text_stream_permit(
                        rx,
                        permit,
                        provider.get_model().to_string(),
                    ));
                }
                Err(e) => {
                    tracing::warn!(
                        "[ModelRouter] 流式工作智能体覆盖模型失败，回退到默认路由: {}",
                        e
                    );
                    self.emit_route_status(task_type, "error");
                    self.emit_route_fallback(task_type, &e.to_string(), character_id);
                }
            }
        }

        // 1. 路由矩阵启用时优先使用任务专属 provider
        if self.enable_routing_matrix {
            if let Some(provider) = self.task_providers.get(task_type) {
                tracing::debug!(
                    "[ModelRouter] 路由流式任务 {} 到专属 provider ({})",
                    task_type,
                    provider.get_model()
                );
                match provider
                    .call_stream_chat(messages.clone(), json_schema.clone())
                    .await
                {
                    Ok(rx) => {
                        self.emit_route_status(task_type, "ok");
                        return Ok(Self::hold_text_stream_permit(
                            rx,
                            permit,
                            provider.get_model().to_string(),
                        ));
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[ModelRouter] 流式任务 {} 专属 provider 失败，回退到主 API: {}",
                            task_type,
                            e
                        );
                        self.emit_route_status(task_type, "error");
                        self.emit_route_fallback(task_type, &e.to_string(), character_id);
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
            match provider.call_stream_chat(messages, json_schema).await {
                Ok(rx) => {
                    self.emit_route_status(task_type, "ok");
                    return Ok(Self::hold_text_stream_permit(
                        rx,
                        permit,
                        provider.get_model().to_string(),
                    ));
                }
                Err(e) => {
                    self.emit_route_status(task_type, "error");
                    self.emit_llm_error_toast(task_type, &e.to_string(), &self.main_endpoint, character_id);
                    return Err(e);
                }
            }
        }

        Err(VivianError::Provider("没有可用的流式提供商".to_string()))
    }

    fn hold_text_stream_permit(
        mut source: mpsc::Receiver<StreamEvent>,
        permit: Option<tokio::sync::OwnedSemaphorePermit>,
        model: String,
    ) -> mpsc::Receiver<StreamEvent> {
        let (tx, rx) = mpsc::channel(32);
        tokio::spawn(async move {
            let _permit = permit;
            while let Some(event) = source.recv().await {
                if let StreamEvent::Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                } = &event
                {
                    usage_store::record_usage(
                        &model,
                        *input_tokens,
                        *output_tokens,
                        *cache_read_tokens,
                        *cache_write_tokens,
                    );
                }
                if tx.send(event).await.is_err() {
                    break;
                }
            }
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
    pub async fn generate(&self, mut request: LLMRequest) -> VivianResult<String> {
        loop {
            let options = self.call_options(&request);
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
            // strict 熔断后强制降级为无 schema
            let effective_schema = if self.strict_is_broken(&task_type) {
                None
            } else {
                json_schema.clone()
            };
            let char_id = character_id.as_deref().unwrap_or("");
            let result = scope_provider_call(options, async {
                if stream {
                    // 流式:累积所有 chunk 返回完整文本
                    let mut rx = self.query_stream(&task_type, messages, effective_schema, char_id).await?;
                    let mut buf = String::new();
                    while let Some(event) = rx.recv().await {
                        match event {
                            StreamEvent::Text { content } => buf.push_str(&content),
                            StreamEvent::Error { message } => {
                                return Err(VivianError::Provider(format!(
                                    "流式响应中断: {}",
                                    message
                                )));
                            }
                            _ => {}
                        }
                    }
                    Ok(buf)
                } else {
                    self.query_with_fallback(&task_type, messages, effective_schema, char_id).await
                }
            })
            .await;
            // strict 拒绝检测:熔断后重试(不带 schema)
            if let Err(ref e) = result {
                if json_schema.is_some() && self.handle_strict_failure(&task_type, e) {
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
    ) -> VivianResult<tokio::sync::mpsc::Receiver<StreamEvent>> {
        loop {
            let options = self.call_options(&request);
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
            let effective_schema = if self.strict_is_broken(&task_type) {
                None
            } else {
                json_schema.clone()
            };
            let rx = scope_provider_call(
                options,
                self.query_stream(&task_type, messages, effective_schema, character_id.as_deref().unwrap_or("")),
            )
            .await;
            // strict 拒绝检测:仅在流未开始时(返回 Err)可重试;流已开始则无法重试
            if let Err(ref e) = rx {
                if json_schema.is_some() && self.handle_strict_failure(&task_type, e) {
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
            let options = self.call_options(&request);
            let LLMRequest {
                task_type,
                messages,
                tools,
                json_schema,
                stream: _,
                character_id,
                ..
            } = request.clone();
            let result = scope_provider_call(
                options,
                self.query_with_tools(&task_type, messages, tools, character_id.as_deref().unwrap_or("")),
            )
            .await;
            // strict 拒绝检测:熔断后重试(不带 schema)
            if let Err(ref e) = result {
                if json_schema.is_some() && self.handle_strict_failure(&task_type, e) {
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
            let options = self.call_options(&request);
            let LLMRequest {
                task_type,
                messages,
                tools,
                json_schema,
                stream: _,
                character_id,
                ..
            } = request.clone();
            let rx = scope_provider_call(
                options,
                self.query_stream_with_tools(&task_type, messages, tools, character_id.as_deref().unwrap_or("")),
            )
            .await;
            // strict 拒绝检测:仅在流未开始时(返回 Err)可重试;流已开始则无法重试
            if let Err(ref e) = rx {
                if json_schema.is_some() && self.handle_strict_failure(&task_type, e) {
                    request.json_schema = None;
                    tracing::info!("[ModelRouter] strict 熔断，重试 generate_stream_with_tools(无 schema)");
                    continue;
                }
            }
            return rx;
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
        // 编程输出预算按服务商分级默认（忽略传入的 max_tokens，理由见
        // build_reasoning_override）：该字段已从设置界面移除，用户不应关心。
        let mut cfg = task_config.clone();
        cfg.max_tokens = Some(crate::providers::factory::work_model_default_max_tokens(
            &cfg.provider_type,
            &cfg.endpoint,
        ));
        match create_task_provider(&cfg, config, &self.client_cache) {
            Ok(provider) => {
                // 工作智能体请求省略 temperature（服务端默认）：
                // 编程任务要确定性，且推理模型对非默认温度敏感
                provider.set_omit_temperature(true);
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
                tracing::warn!(
                    "[ModelRouter] 构建工作智能体 provider 失败: {}",
                    e
                );
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

    fn strict_identity_for_task(&self, task_type: &str) -> Option<String> {
        if let Some(provider) = self.override_provider_for(task_type) {
            return Some(provider.provider_identity());
        }
        self.resolve_provider(task_type)
            .map(|provider| provider.provider_identity())
    }

    fn strict_is_broken(&self, task_type: &str) -> bool {
        self.strict_identity_for_task(task_type)
            .is_some_and(|identity| self.strict_broken.read().contains(&identity))
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
        // 按任务分组获取并发信号量
        let sem_opt = self.get_semaphore(task_type);
        let _permit = if let Some(sem) = sem_opt {
            Some(
                sem.acquire_owned()
                    .await
                    .map_err(|e| VivianError::Provider(format!("获取并发信号量失败: {}", e)))?,
            )
        } else {
            None
        };

        let mut last_error: Option<VivianError> = None;
        // 跟随 last_error 一起记录：最后实际失败的 provider 的 endpoint
        let mut last_error_endpoint: Option<String> = None;

        // 0. 工作智能体覆盖模型优先（仅 reasoning 任务）——用户显式选择，优先级高于路由矩阵
        if let Some(provider) = self.override_provider_for(task_type) {
            if provider.supports_native_function_calling() {
                tracing::debug!(
                    "[ModelRouter] 路由任务 {} (native fc) 到工作智能体覆盖模型 ({})",
                    task_type,
                    provider.get_model()
                );
                match Self::invoke_with_tools(&provider, messages.clone(), tools.clone()).await {
                    Ok(resp) => {
                        Self::log_llm_response(task_type, &resp);
                        self.emit_route_status(task_type, "ok");
                        return Ok(resp);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[ModelRouter] 工作智能体覆盖模型 native fc 失败，回退到默认路由: {}",
                            e
                        );
                        self.emit_route_status(task_type, "error");
                        self.emit_route_fallback(task_type, &e.to_string(), character_id);
                        last_error = Some(e);
                    }
                }
            } else {
                tracing::debug!(
                    "[ModelRouter] 工作智能体覆盖模型不支持 native fc，跳过",
                );
            }
        }

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
                            self.emit_route_fallback(task_type, &e.to_string(), character_id);
                            last_error = Some(e);
                            last_error_endpoint = self.task_endpoints.get(task_type).cloned();
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
                        last_error_endpoint = Some(self.main_endpoint.clone());
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
        self.emit_llm_error_toast(
            task_type,
            &err.to_string(),
            last_error_endpoint.as_deref().unwrap_or(""),
            character_id,
        );
        Err(err)
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
        let tool_names: Vec<&str> = resp
            .tool_calls
            .iter()
            .map(|tc| tc.name.as_str())
            .collect();
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
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        Self::log_llm_request(task_type, &messages, &tools);
        // 按任务分组获取并发信号量
        let sem_opt = self.get_semaphore(task_type);
        let permit = if let Some(sem) = sem_opt {
            Some(
                sem.acquire_owned()
                    .await
                    .map_err(|e| VivianError::Provider(format!("获取并发信号量失败: {}", e)))?,
            )
        } else {
            None
        };

        // 0. 工作智能体覆盖模型优先（仅 reasoning 任务）——用户显式选择，优先级高于路由矩阵
        if let Some(provider) = self.override_provider_for(task_type) {
            if provider.supports_native_function_calling() {
                tracing::debug!(
                    "[ModelRouter] 路由流式任务 {} (native fc) 到工作智能体覆盖模型 ({})",
                    task_type,
                    provider.get_model()
                );
                match Self::stream_with_tools_provider(
                    &provider,
                    messages.clone(),
                    tools.clone(),
                )
                .await
                {
                    Ok(rx) => {
                        self.emit_route_status(task_type, "ok");
                        return Ok(Self::hold_event_stream_permit(rx, permit));
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[ModelRouter] 工作模型流式 FC 失败，回退默认路由: {}",
                            e
                        );
                        self.emit_route_status(task_type, "error");
                        self.emit_route_fallback(task_type, &e.to_string(), character_id);
                    }
                }
            } else {
                tracing::debug!(
                    "[ModelRouter] 工作智能体覆盖模型不支持 native fc stream，跳过",
                );
            }
        }

        // 1. 路由矩阵启用时优先用任务专属 provider
        if self.enable_routing_matrix {
            if let Some(provider) = self.task_providers.get(task_type) {
                if provider.supports_native_function_calling() {
                    tracing::debug!(
                        "[ModelRouter] 路由流式任务 {} (native fc) 到专属 provider ({})",
                        task_type,
                        provider.get_model()
                    );
                    match Self::stream_with_tools_provider(
                        provider,
                        messages.clone(),
                        tools.clone(),
                    )
                    .await
                    {
                        Ok(rx) => {
                            self.emit_route_status(task_type, "ok");
                            return Ok(Self::hold_event_stream_permit(rx, permit));
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[ModelRouter] 任务 {} 专属 provider 流式 FC 失败，回退主 API: {}",
                                task_type,
                                e
                            );
                            self.emit_route_status(task_type, "error");
                            self.emit_route_fallback(task_type, &e.to_string(), character_id);
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
                match Self::stream_with_tools_provider(provider, messages, tools).await {
                    Ok(rx) => {
                        self.emit_route_status(task_type, "ok");
                        return Ok(Self::hold_event_stream_permit(rx, permit));
                    }
                    Err(e) => {
                        self.emit_route_status(task_type, "error");
                        self.emit_llm_error_toast(task_type, &e.to_string(), &self.main_endpoint, character_id);
                        return Err(e);
                    }
                }
            }
        }

        Err(VivianError::NotImplemented(format!(
            "任务 {} 没有可用的 provider 支持流式原生 function calling",
            task_type
        )))
    }

    fn hold_event_stream_permit(
        mut source: mpsc::Receiver<StreamEvent>,
        permit: Option<tokio::sync::OwnedSemaphorePermit>,
    ) -> mpsc::Receiver<StreamEvent> {
        let (tx, rx) = mpsc::channel(32);
        tokio::spawn(async move {
            let _permit = permit;
            while let Some(event) = source.recv().await {
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });
        rx
    }

    /// 内部辅助：直接调用 provider 的 stream_with_tools
    ///
    /// 与 `invoke_with_tools` 不同，stream_with_tools 直接接受外部 tools 参数，
    /// 不需要 bind_tools 步骤（避免克隆 provider 实例的开销）。
    /// 流式事件原样转发，同时采集 Usage 事件计入全局用量存储。
    async fn stream_with_tools_provider(
        provider: &Box<dyn BaseProvider>,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
    ) -> VivianResult<mpsc::Receiver<StreamEvent>> {
        let mut rx = provider.stream_with_tools(messages, tools).await?;
        let model = provider.get_model().to_string();
        let (tx, out_rx) = mpsc::channel::<StreamEvent>(32);
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if let StreamEvent::Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                } = &event
                {
                    usage_store::record_usage(
                        &model,
                        *input_tokens,
                        *output_tokens,
                        *cache_read_tokens,
                        *cache_write_tokens,
                    );
                }
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });
        Ok(out_rx)
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
    fn handle_strict_failure(&self, task_type: &str, err: &VivianError) -> bool {
        if !Self::is_strict_error(err) {
            return false;
        }
        let identity = self
            .strict_identity_for_task(task_type)
            .unwrap_or_else(|| format!("task:{}", task_type));
        let mut broken = self.strict_broken.write();
        if !broken.insert(identity.clone()) {
            // 已经熔断过，不应该再触发（理论上 apply_json_schema 已降级）
            tracing::debug!("[ModelRouter] strict 熔断已生效，但仍有 strict 错误: {}", err);
        } else {
            tracing::warn!(
                "[ModelRouter] 检测到 strict schema 拒绝，按 provider 熔断并降级: identity={} error={}",
                identity,
                err,
            );
            save_strict_broken_models(&broken);
        }
        true
    }
}

#[cfg(test)]
mod conversational_penalty_tests {
    use super::is_conversational_task;

    /// 惩罚参数只应注入"角色正在说话"的任务。
    ///
    /// 三个正例与 `generation.rs::build_chat_request` 注入响应 Schema 的集合一致
    /// ——`reasoning` 是携带工具定义时由 `chat` 升级而来。
    #[test]
    fn only_spoken_output_tasks_get_penalties() {
        for task in ["chat", "reasoning", "vision_describe"] {
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
