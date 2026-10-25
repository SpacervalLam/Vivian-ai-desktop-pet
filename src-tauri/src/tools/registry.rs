//! 工具注册表 - 管理工具的注册与查找
//!
//! `ToolSystem` 是工具系统的统一入口，整合注册表、缓存、沙箱、
//! 可观测性等组件，并提供线程安全的工具查找接口。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use super::cache::ToolCache;
use super::catalog::ToolCatalog;
pub use super::catalog::normalize_tool_name;
use super::confirmation::{
    ConfirmationRequest, ConfirmationResponse, ConfirmationRisk, ToolConfirmationRegistry,
};
use super::observability::ToolObservability;
use super::sandbox::{ProtectionMode, ToolSandbox};
use super::types::{Tool, ToolCategory, ToolDefinition, ToolScene};

/// 仅工作智能体（`agent_kind == "work"`）可用的工具。
///
/// `create_tool` 与 `create_plugin` 是重进化事件：涉及脚本落地 / 插件打包 +
/// 用户预览卡片授权，执行主体统一收口到工作智能体。陪伴侧所有经
/// `ToolScene` 暴露的工具面（`list_tools_for_scene` 的两个消费方：API tools
/// 字段与 system prompt 工具清单）以及 `tool_search` 的检索域都不包含它们；
/// 执行层另有硬门兜底（防幻觉盲调 / 历史重放）。轻量沉淀（create_skill）
/// 不在名单内，陪伴侧可直接使用。
pub const WORK_AGENT_ONLY_TOOLS: &[&str] = &["create_tool", "create_plugin", "delete_plugin"];

/// 判断工具是否为工作智能体专属工具
pub fn is_work_agent_only(name: &str) -> bool {
    WORK_AGENT_ONLY_TOOLS.contains(&name)
}

/// 智能体侧别 —— 两条独立产品线（陪伴 / 工作）的区分维度。
///
/// 用于工具禁用状态的**分侧隔离**：同一工具在一侧被禁用不影响另一侧。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentSide {
    /// 陪伴侧（agent_kind != "work"：vivian / nana / Vivian）
    Companion,
    /// 工作侧（agent_kind == "work"：编程智能体）
    Work,
}

impl AgentSide {
    /// 由 `ToolUseContext.agent_kind` 映射：`"work"` → 工作侧，其余 → 陪伴侧。
    pub fn from_agent_kind(kind: &str) -> Self {
        if kind == "work" {
            AgentSide::Work
        } else {
            AgentSide::Companion
        }
    }
}

/// 工具归属的侧别 —— 单一真相源。
///
/// 由两张既有清单推导（[`WORK_AGENT_ONLY_TOOLS`] 与
/// [`crate::brain::coding_agent::CODING_TOOLS`]），取代此前散落在三处的隐式判断：
/// 本文件的 `is_work_agent_only` 过滤、工作侧白名单、设置页直接 dump 全量。
/// 设置页展示、陪伴侧工具面、工作侧工具面现在都从这一个函数取侧别，
/// 新增工具不会再出现"忘记同步某处"的不一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolScope {
    /// 仅陪伴侧可用（不在工作侧白名单）
    Companion,
    /// 仅工作侧可用（能力进化事件：create_tool / create_plugin / delete_plugin）
    Work,
    /// 两侧都可用
    Both,
}

impl ToolScope {
    /// 序列化给前端的标识（`list_tools` 输出与设置页分区依据）
    pub fn as_str(self) -> &'static str {
        match self {
            ToolScope::Companion => "companion",
            ToolScope::Work => "work",
            ToolScope::Both => "both",
        }
    }

    /// 该工具是否出现在指定侧的工具面上
    pub fn includes(self, side: AgentSide) -> bool {
        matches!(
            (self, side),
            (ToolScope::Both, _)
                | (ToolScope::Companion, AgentSide::Companion)
                | (ToolScope::Work, AgentSide::Work)
        )
    }
}

/// 推导工具归属侧别（单一真相源）。
pub fn tool_scope(name: &str) -> ToolScope {
    if WORK_AGENT_ONLY_TOOLS.contains(&name) {
        ToolScope::Work
    } else if crate::brain::coding_agent::CODING_TOOLS.contains(&name) || crate::brain::coding_agent::WORK_WEB_BROWSER_TOOLS.contains(&name) {
        ToolScope::Both
    } else {
        ToolScope::Companion
    }
}

/// 工作侧不可禁用的只读基座工具。
///
/// 禁用后工作智能体连"看看项目结构 / 读一个文件"都做不到，任何编程任务都会
/// 立即失败——这几乎不会是用户的真实意图，故锁定。**可变更类**工具
/// （run_command / write_file / edit_file）不在此列：出于安全考虑关掉它们是
/// 用户的合法开关，不做限制。
pub const WORK_LOCKED_TOOLS: &[&str] = &["read_file", "list_dir", "grep_search"];

/// 判断某侧的工具是否被锁定（锁定 = 不可禁用，`is_tool_disabled` 恒为 false）。
pub fn is_tool_locked(name: &str, side: AgentSide) -> bool {
    matches!(side, AgentSide::Work) && WORK_LOCKED_TOOLS.contains(&name)
}

/// 工具系统 - 整合所有工具组件的统一入口
pub struct ToolSystem {
    /// Names, aliases, ownership and revision change atomically.
    catalog: RwLock<ToolCatalog<Arc<dyn Tool>>>,
    schema_cache: RwLock<(u64, HashMap<String, ToolDefinition>)>,
    /// 工具结果缓存
    pub cache: Arc<ToolCache>,
    /// 沙箱
    pub sandbox: Arc<ToolSandbox>,
    /// 可观测性
    pub observability: Arc<ToolObservability>,
    /// 工具执行确认注册表（用于 toast 弹窗请求用户同意）
    pub confirmation: Arc<ToolConfirmationRegistry>,
    /// Tauri AppHandle（运行时注入，用于 emit 事件给前端）
    app_handle: RwLock<Option<AppHandle>>,
    /// 用户授权确认弹窗的最长等待时间（秒，来自 `config.tools.confirmation_timeout_secs`）
    ///
    /// 运行时可通过 `update_confirmation_timeout` 热更新。
    confirmation_timeout_secs: RwLock<u64>,
    /// 最近一次工具调用的时间戳（用于 `has_recent_tool_call` 判定场景为 Chat/Task）
    ///
    /// 由 `execute_tool_use` 在每次工具执行前更新。
    /// `ToolScene::from_full_context` 据此判断 `has_recent_tool_use`，
    /// 进而决定注入 `Task` 还是 `Chat` 场景的工具集。
    last_tool_call_at: RwLock<Option<Instant>>,
    /// 用户禁用的工具名，按侧别隔离（来自 `config.tools.disabled_tools.companion` / `.work`）
    ///
    /// 分侧的意义：陪伴侧与工作侧是两条独立产品线，同一工具（如 web_search）
    /// 在一侧禁用不应影响另一侧。禁用的工具不注入**该侧**的 LLM 工具列表
    /// （陪伴侧 `list_tools_for_scene` / 工作侧 `get_tool_schemas`），执行入口按
    /// `agent_kind` 映射出的侧别直接拒绝。`list_tools`（设置界面用）不受影响。
    disabled_tools: RwLock<HashMap<AgentSide, HashSet<String>>>,
    /// Cordis 运行时上下文引用（策略缝 guard / post-execute 的分发目标）。
    ///
    /// 由 AppState 初始化时注入；用于 `execute_tool_use` 在 pre/post 阶段
    /// 派发可插桩的策略瀑布。为 `None` 时策略缝静默跳过，保持兼容旧调用。
    policy_ctx: RwLock<Option<Arc<crate::cordis::RuntimeContext>>>,
}

impl ToolSystem {
    pub fn can_parallelize_read(&self, name: &str) -> bool {
        crate::brain::work_harness::parallel_read(name) && self.find_tool(name).is_some_and(|tool| tool.is_read_only())
    }
    pub fn work_request_limits(&self) -> (u64, usize) {
        if let Some(app) = self.app_handle.read().clone() {
            if let Some(state) = app.try_state::<Arc<crate::state::AppState>>() {
                let tools = state.config.read().get_all().tools;
                return (tools.work_request_timeout_secs.max(1), tools.work_request_max_attempts.max(1));
            }
        }
        (crate::brain::work_harness::STEP_TIMEOUT_SECS, crate::brain::work_harness::MAX_PROVIDER_ATTEMPTS)
    }
    /// 创建新的工具系统（使用默认配置：缓存 TTL=300s / max=1000 / 确认超时=600s）
    pub fn new() -> Self {
        Self {
            catalog: RwLock::new(ToolCatalog::default()),
            schema_cache: RwLock::new((u64::MAX, HashMap::new())),
            cache: Arc::new(ToolCache::new(300, 1000)),
            sandbox: Arc::new(ToolSandbox::new(ProtectionMode::Cautious, 600)),
            observability: Arc::new(ToolObservability::new(1000)),
            confirmation: Arc::new(ToolConfirmationRegistry::new()),
            app_handle: RwLock::new(None),
            confirmation_timeout_secs: RwLock::new(600),
            last_tool_call_at: RwLock::new(None),
            disabled_tools: RwLock::new(HashMap::new()),
            policy_ctx: RwLock::new(None),
        }
    }

    /// 使用 ToolConfig 创建工具系统（缓存 TTL/max_size 与确认超时从配置读取）
    ///
    /// 当 `enable_cache=false` 时，TTL=0 禁用缓存读写。
    pub fn with_tool_config(
        cache_ttl_secs: u64,
        cache_max_size: u32,
        enable_cache: bool,
        confirmation_timeout_secs: u64,
    ) -> Self {
        let effective_ttl = if enable_cache { cache_ttl_secs } else { 0 };
        Self {
            catalog: RwLock::new(ToolCatalog::default()),
            schema_cache: RwLock::new((u64::MAX, HashMap::new())),
            cache: Arc::new(ToolCache::new(effective_ttl, cache_max_size as usize)),
            sandbox: Arc::new(ToolSandbox::new(ProtectionMode::Cautious, 600)),
            observability: Arc::new(ToolObservability::new(1000)),
            confirmation: Arc::new(ToolConfirmationRegistry::new()),
            app_handle: RwLock::new(None),
            confirmation_timeout_secs: RwLock::new(confirmation_timeout_secs.max(10)),
            last_tool_call_at: RwLock::new(None),
            disabled_tools: RwLock::new(HashMap::new()),
            policy_ctx: RwLock::new(None),
        }
    }

    /// 更新用户禁用的工具名（设置保存 / 启动加载时调用）
    ///
    /// 直接整体替换：设置界面按 `config.tools.disabled_tools` 的
    /// `companion` / `work` 两个列表全量写入对应侧。
    pub fn set_disabled_tools(&self, companion: Vec<String>, work: Vec<String>) {
        let mut map = HashMap::new();
        map.insert(
            AgentSide::Companion,
            companion.into_iter().collect::<HashSet<_>>(),
        );
        map.insert(AgentSide::Work, work.into_iter().collect::<HashSet<_>>());
        *self.disabled_tools.write() = map;
    }

    /// 查询工具在指定侧是否被用户禁用。
    ///
    /// 锁定工具（如工作侧 read_file）恒为 false ——
    /// 它们不可被禁用，配置里即使残留对应条目也不生效。
    pub fn is_tool_disabled(&self, name: &str, side: AgentSide) -> bool {
        let catalog = self.catalog.read();
        let canonical = catalog.resolve(name).or_else(|| super::builtin::web_tool_compat::target(name)).unwrap_or(name);
        if is_tool_locked(canonical, side) {
            return false;
        }
        self.disabled_tools
            .read()
            .get(&side)
            .map(|set| {
                set.iter().any(|disabled| {
                    catalog.resolve(disabled).or_else(|| super::builtin::web_tool_compat::target(disabled)).unwrap_or(disabled) == canonical
                })
            })
            .unwrap_or(false)
    }

    /// 注入 Cordis 运行时上下文（启用 guard / post-execute 策略缝）。
    pub fn set_policy_ctx(&self, ctx: Arc<crate::cordis::RuntimeContext>) {
        *self.policy_ctx.write() = Some(ctx);
    }

    /// 读取已注入的策略上下文（`None` = 未注入，策略缝跳过）。
    pub fn policy_ctx(&self) -> Option<Arc<crate::cordis::RuntimeContext>> {
        self.policy_ctx.read().clone()
    }

    /// 热更新确认超时（秒）—— 由 ChatChain 在 reinitialize 时调用
    pub fn update_confirmation_timeout(&self, secs: u64) {
        *self.confirmation_timeout_secs.write() = secs.max(10);
    }

    /// One path for save, reload and reinitialization of tool runtime settings.
    pub fn apply_config(&self, config: &crate::config::manager::ToolConfig) {
        self.set_disabled_tools(config.disabled_tools.companion.clone(), config.disabled_tools.work.clone());
        self.update_confirmation_timeout(config.confirmation_timeout_secs);
        self.cache.reconfigure(if config.enable_cache { config.cache_ttl_secs } else { 0 }, config.cache_max_size as usize);
        // Saved app settings can change tool dependencies (location, providers, etc.).
        self.cache.clear();
        super::executor::update_runtime_config(&super::executor::ToolRuntimeConfig {
            default_tool_timeout_secs: config.default_tool_timeout_secs,
            max_result_chars: config.max_result_chars as usize,
            access_level: super::types::AgentAccessLevel::from_str(&config.access_level),
        });
    }

    /// 记录一次工具调用（由 `execute_tool_use` 在工具找到后立即调用）
    ///
    /// 用于 `has_recent_tool_call` 判定，进而驱动 `ToolScene::from_full_context`
    /// 将场景判定为 `Task`（近期调过工具）而非 `Chat`。
    pub fn record_tool_call(&self) {
        *self.last_tool_call_at.write() = Some(Instant::now());
    }

    /// 是否在最近 `within_secs` 秒内调过工具
    ///
    /// `within_secs` 推荐 300（5 分钟）—— 覆盖一轮多步工具调用 + 后续 1-2 轮追问。
    pub fn has_recent_tool_call(&self, within_secs: u64) -> bool {
        let last = self.last_tool_call_at.read();
        match *last {
            Some(t) => t.elapsed().as_secs() <= within_secs,
            None => false,
        }
    }

    /// 注入 Tauri AppHandle，启用工具执行确认的 toast 弹窗流程
    ///
    /// 应在 app.setup 中调用，注入后 execute_tool_use 检测到 ask 状态时
    /// 会 emit `tool:confirmation_request` 事件给前端，前端弹 Modal 询问用户。
    pub fn set_app_handle(&self, handle: AppHandle) {
        *self.app_handle.write() = Some(handle);
    }

    /// 请求用户确认工具执行（emit 事件 + await oneshot）
    ///
    /// 返回 `Some(ConfirmationResponse)` 表示用户的三态选择（Deny/AllowOnce/AllowAlways），
    /// `None` 表示无 AppHandle（未初始化）或用户未响应。
    pub async fn request_confirmation(
        &self,
        tool: &str,
        arguments: &Value,
        reason: String,
        risk_level: ConfirmationRisk,
        char_id: &str,
        allow_always_scope: &str,
    ) -> Option<ConfirmationResponse> {
        let handle = self.app_handle.read().clone()?;

        let request = ConfirmationRequest {
            request_id: 0, // create_request 会分配真实 id，稍后回填
            tool: tool.to_string(),
            arguments: arguments.clone(),
            reason,
            risk_level,
            char_id: char_id.to_string(),
            allow_always_scope: allow_always_scope.to_string(),
        };
        // 注册表清理和 await 使用同一窗口期，避免固定 5 分钟 TTL 提前丢弃请求。
        let timeout_secs = *self.confirmation_timeout_secs.read();
        let (id, rx) = self.confirmation.create_request_with_timeout(
            request.clone(), std::time::Duration::from_secs(timeout_secs));
        let _pending = self.confirmation.cancel_on_drop(id);
        struct ConfirmationUiCleanup(AppHandle, u64);
        impl Drop for ConfirmationUiCleanup {
            fn drop(&mut self) {
                let _ = self.0.emit("toast:confirm_done", serde_json::json!({"request_id":self.1}));
            }
        }
        let _ui_cleanup = ConfirmationUiCleanup(handle.clone(), id);
        let request = ConfirmationRequest { request_id: id, ..request };

        tracing::info!(request_id = id, tool = tool, char_id = char_id,
            path = ?arguments.get("path").and_then(|value| value.as_str()),
            "[ToolSystem] waiting for confirmation");

        // emit 给发起角色对应的主窗口（label = char_id），避免广播到其他角色窗口
        // 导致多角色同时使用工具时 suspend/resume 计数器失配
        let emit_result = if request.char_id.is_empty() {
            handle.emit("tool:confirmation_request", &request)
        } else {
            handle.emit_to(request.char_id.as_str(), "tool:confirmation_request", &request)
        };
        if let Err(e) = emit_result {
            tracing::warn!("[ToolSystem] emit confirmation_request 失败: {}", e);
            self.confirmation.cancel_request(id);
            return None;
        }

        // 同步压入远程通知队列，供手机端确认 toast 轮询展示（标题标注发起角色）
        let toast_title = if request.char_id.is_empty() {
            "智能体请求确认".to_string()
        } else {
            format!(
                "{} 请求确认",
                crate::cross_character::display_name(&request.char_id)
            )
        };
        crate::remote::push_toast(
            "confirmation",
            &toast_title,
            &request.reason,
            &request.char_id,
            serde_json::json!({
                "request_id": request.request_id,
                "tool": request.tool,
                "risk_level": request.risk_level,
                "allow_always_scope": request.allow_always_scope,
            }),
        );

        // 等待用户响应（超时由 config.tools.confirmation_timeout_secs 控制，避免永久阻塞）
        match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), rx).await {
            Ok(Ok(response)) => Some(response),
            Ok(Err(_)) => {
                tracing::warn!("[ToolSystem] 确认请求 {} 的 sender 被 drop", id);
                None
            }
            Err(_) => {
                tracing::warn!(
                    "[ToolSystem] 确认请求 {} 超时（{} 秒）",
                    id,
                    timeout_secs
                );
                self.confirmation.cancel_request(id);
                None
            }
        }
    }

    /// 注册工具（普通注册项没有插件 owner）。
    pub fn register_tool(&self, tool: Arc<dyn Tool>) {
        self.register_tool_with_owner(tool, None);
    }

    /// 注册带 owner 的工具，供插件/MCP 等可卸载来源使用。
    pub fn register_tool_with_owner(&self, tool: Arc<dyn Tool>, owner: Option<String>) {
        let name = tool.name().to_string();
        let category = tool.category();
        tracing::debug!("注册工具: {} (类别: {:?}, owner: {:?})", name, category, owner);

        self.catalog.write().register(name.clone(), tool, owner);
        self.cache.invalidate(&name);
    }

    /// 注册工具别名：`from` 会解析到 `to`。
    ///
    /// 用途是**兼容历史名字**——工具改名后，已经沉淀在 few-shot 示例、用户配置、
    /// 历史对话里的旧名字仍然能命中（否则模型照示例发旧名会直接查不到工具）。
    ///
    /// ⚠️ 别名只在 `find_tool` 的**第二级**匹配生效，且**不做词序重排**：
    /// `normalize_tool_name` 只去分隔符，`set_wallpaper`→`setwallpaper` 与
    /// `wallpaper_set`→`wallpaperset` 是两个不同的键，指望规范化兜底是不成立的。
    ///
    /// 返回是否注册成功（`to` 未注册时返回 false，避免留下指向空目标的悬空别名）。
    pub fn register_alias(&self, from: &str, to: &str) -> bool {
        self.catalog.write().register_alias(from, to)
    }

    /// 注册一批别名，返回成功注册的数量。
    ///
    /// 传 `&[("旧名", "真名")]`。单项失败只 warn 不中断——别名是兼容性兜底，
    /// 不该因为一个目标工具没注册就把其余别名一起丢掉。
    pub fn register_aliases(&self, pairs: &[(&str, &str)]) -> usize {
        pairs
            .iter()
            .filter(|(from, to)| self.register_alias(from, to))
            .count()
    }

    /// 仅当当前注册项仍归指定 owner 时注销工具。
    pub fn unregister_tool_if_owner(&self, name: &str, owner: &str) -> bool {
        let removed = self.catalog.write().remove(name, Some(owner));
        if removed { self.cache.invalidate(name); }
        removed
    }

    /// 注销工具
    pub fn unregister_tool(&self, name: &str) -> bool {
        let removed = self.catalog.write().remove(name, None);
        if removed { self.cache.invalidate(name); }
        removed
    }

    /// 查找工具
    ///
    /// 匹配顺序：
    /// 1. 精确匹配工具名
    /// 2. 别名匹配
    /// 3. 规范化匹配（lowercase + 去除 `_`/`-`/`.` 分隔符），容错 LLM 输出大小写或分隔符偏差
    pub fn find_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.find_tool_versioned(name).map(|(tool,_)| tool)
    }

    /// The implementation and revision must come from the same registry snapshot.
    pub(crate) fn find_tool_versioned(&self, name: &str) -> Option<(Arc<dyn Tool>, u64)> {
        let catalog = self.catalog.read();
        if let Some(tool) = catalog.find(name) { return Some((tool.clone(),catalog.revision())); }
        let target = super::builtin::web_tool_compat::target(name)?;
        Some((super::builtin::web_tool_compat::wrap(name,catalog.find(target)?.clone()),catalog.revision()))
    }

    /// 是否存在指定工具
    pub fn has_tool(&self, name: &str) -> bool {
        self.find_tool(name).is_some()
    }

    /// 列出所有工具
    pub fn list_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.catalog.read().values().cloned().collect()
    }

    /// 按场景列出工具
    ///
    /// 设计变更：不再做场景黑名单硬过滤（旧 `blocked_tools()` 已废弃）。
    /// 所有注册工具均返回，由 LLM 自主判断是否调用，危险操作通过
    /// `check_permissions` 在执行时确认。场景信息通过 `ToolScene::soft_hint()`
    /// 注入 prompt 作为软提示，引导但不强制 LLM 的工具选择。
    /// 延迟加载由 `should_defer` 控制（在 `tool_call_manager` 中分流）。
    ///
    /// 陪伴侧工具面：只返回归属陪伴侧的 [`ToolScope`]（`Companion` / `Both`），
    /// 并按**陪伴侧**的禁用集合过滤（`config.tools.disabled_tools.companion`）。
    ///
    /// 侧别判定统一走 [`tool_scope`]（单一真相源），不再在本方法内硬编码
    /// "排除 WORK_AGENT_ONLY_TOOLS"。工作智能体不走本方法（用 `CODING_TOOLS`
    /// 白名单 ∩ `get_tool_schemas`），其禁用集合是独立的 `...work`，
    /// 两侧互不影响。
    pub fn list_tools_for_scene(&self, _scene: ToolScene) -> Vec<Arc<dyn Tool>> {
        self.list_tools_for_side(AgentSide::Companion)
    }

    /// Shared discovery boundary, including live per-side user switches.
    pub fn list_tools_for_side(&self, side: AgentSide) -> Vec<Arc<dyn Tool>> {
        self.list_tools().into_iter()
            .filter(|t| side == AgentSide::Work || tool_scope(t.name()).includes(side))
            .filter(|t| !self.is_tool_disabled(t.name(), side)).collect()
    }

    /// 列出所有工具名
    pub fn list_tool_names(&self) -> Vec<String> {
        self.catalog.read().names().cloned().collect()
    }

    /// 按类别列出工具
    pub fn list_tools_by_category(&self, category: ToolCategory) -> Vec<Arc<dyn Tool>> {
        self.list_tools().into_iter().filter(|t| t.category() == category).collect()
    }

    /// 获取所有类别
    pub fn get_categories(&self) -> Vec<ToolCategory> {
        let mut categories: Vec<_> = self.list_tools().iter().map(|t| t.category()).collect();
        categories.sort_by_key(|category| category.as_str());
        categories.dedup();
        categories
    }

    /// 获取所有工具的 schema 定义（**工作侧**视角）。
    ///
    /// 按工作侧的禁用集合（`config.tools.disabled_tools.work`）过滤；
    /// 编程智能体的 FC tools 字段由此生成（再与 `CODING_TOOLS` 白名单取交集）。
    /// 陪伴侧的过滤在 [`Self::list_tools_for_scene`] 中按陪伴侧集合独立进行。
    pub fn get_tool_schemas(&self) -> Vec<ToolDefinition> {
        let names = self.list_tool_names();
        self.work_tool_schemas(&names.iter().map(String::as_str).collect::<Vec<_>>())
    }

    /// Render only requested, currently enabled schemas. Registry replacement
    /// invalidates cached definitions; per-side disables are checked every time.
    pub fn work_tool_schemas(&self, names: &[&str]) -> Vec<ToolDefinition> {
        let (revision, tools) = {
            let catalog = self.catalog.read();
            (catalog.revision(), names.iter().filter_map(|name| catalog.find(name).cloned()).collect::<Vec<_>>())
        };
        let mut cache = self.schema_cache.write();
        if cache.0 != revision { *cache = (revision, HashMap::new()); }
        tools.into_iter().filter(|tool| !self.is_tool_disabled(tool.name(), AgentSide::Work))
            .map(|tool| cache.1.entry(tool.name().into()).or_insert_with(|| tool.to_definition()).clone()).collect()
    }

    /// 获取所有工具的 OpenAI 格式定义
    pub fn work_extension_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.list_tools_for_side(AgentSide::Work).iter()
            .filter(|t| t.is_custom() || t.category() == ToolCategory::Mcp)
            .filter(|t| !t.name().starts_with("mcp__browser__"))
            .map(|t| t.name().to_string()).collect();
        names.sort();
        names
    }

    /// 获取所有工具的 OpenAI 格式定义
    pub fn get_openai_tools(&self) -> Vec<Value> {
        self.list_tools().iter().map(|t| t.to_openai_format()).collect()
    }

    /// 简单搜索工具（按名称/描述匹配）
    pub fn search(&self, query: &str) -> Vec<Arc<dyn Tool>> {
        let query_lower = query.to_lowercase();
        self.list_tools()
            .into_iter()
            .filter(|t| {
                t.name().to_lowercase().contains(&query_lower)
                    || t.description().to_lowercase().contains(&query_lower)
            })
            .collect()
    }

    /// 清空所有工具
    pub fn clear(&self) {
        self.catalog.write().clear();
        self.cache.clear();
    }

    /// 清空缓存
    pub fn invalidate_cache(&self) {
        self.cache.clear();
    }

    /// 获取缓存统计
    pub fn get_cache_stats(&self) -> serde_json::Value {
        self.cache.stats()
    }

    /// 获取可观测性摘要
    pub fn get_observability_summary(&self) -> serde_json::Value {
        self.observability.summary()
    }
}

impl Default for ToolSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    // 注意：`super::*` 只带来 registry 模块自己 `use` 进来的名字
    // （Tool / ToolCategory / ToolDefinition / ToolScene），`tools/mod.rs` 的
    // 重导出**不在**其中——这几个必须显式按 `types` 路径引入。
    use crate::tools::types::{PermissionResult, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult};

    /// 夹具：只有名字重要，其余全走最小实现。
    struct NamedTool(&'static str);

    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }

        fn description(&self) -> &str {
            "registry test tool"
        }

        fn parameters_schema(&self) -> Value {
            json!({"type": "object"})
        }

        async fn validate_input(
            &self,
            _input: &Value,
            _context: &ToolUseContext,
        ) -> ValidationResult {
            ValidationResult::success(None)
        }

        async fn check_permissions(
            &self,
            _input: &Value,
            _context: &ToolUseContext,
        ) -> PermissionResult {
            PermissionResult::allow()
        }

        async fn call(&self, _args: Value, _context: &ToolUseContext) -> ToolResult {
            ToolResult::success(json!({}))
        }

        fn is_read_only(&self) -> bool {
            false
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::System
        }

        fn risk(&self) -> ToolRiskTier {
            ToolRiskTier::FsRead
        }
    }

    fn system_with(names: &[&'static str]) -> ToolSystem {
        let ts = ToolSystem::new();
        for n in names {
            ts.register_tool(Arc::new(NamedTool(n)));
        }
        ts
    }

    #[test]
    fn cached_schemas_refresh_after_replacement_and_respect_live_disables() {
        use crate::tools::custom_tools::{CustomToolDef, DynamicTool};
        let system = ToolSystem::new();
        let register = |description: &str| system.register_tool(Arc::new(DynamicTool::new(CustomToolDef {
            name: "fixture_schema".into(), description: description.into(),
            parameters: json!({"type":"object"}), script: "Write-Output 'fixture'".into(),
            deferred: true, created_at: 0.,
        })));
        register("first");
        assert_eq!(system.work_tool_schemas(&["fixture_schema"])[0].description, "first");
        assert_eq!(system.work_tool_schemas(&["fixture_schema"])[0].description, "first");
        register("replacement");
        assert_eq!(system.work_tool_schemas(&["fixture_schema"])[0].description, "replacement");
        system.set_disabled_tools(Vec::new(), vec!["fixture_schema".into()]);
        assert!(system.work_tool_schemas(&["fixture_schema"]).is_empty());
        system.set_disabled_tools(Vec::new(), Vec::new());
        assert_eq!(system.work_tool_schemas(&["fixture_schema"])[0].description, "replacement");
    }

    /// 别名表必须能将改名后的旧名解析到新名——这是它存在的唯一理由。
    /// 若仅有 `retain`/`clear` 而无注册调用点，整张表是死的，旧名查不到工具，
    /// 且该失效编译器不报、无测试覆盖。
    #[test]
    fn alias_resolves_a_renamed_tool() {
        let ts = system_with(&["wallpaper_set"]);
        assert!(ts.find_tool("set_wallpaper").is_none(), "未注册别名前旧名不该可解析");

        assert!(ts.register_alias("set_wallpaper", "wallpaper_set"));
        let tool = ts.find_tool("set_wallpaper").expect("别名应能解析到真实工具");
        assert_eq!(tool.name(), "wallpaper_set");
        assert!(ts.has_tool("set_wallpaper"));
    }

    #[test]
    fn historical_web_names_resolve_without_appearing_in_the_catalog() {
        let system = system_with(&["web_search","read_content"]);
        assert_eq!(system.find_tool("platform_search").unwrap().name(),"web_search");
        assert_eq!(system.find_tool("read_bilibili_danmaku").unwrap().name(),"read_content");
        assert_eq!(system.find_tool("web_fetch").unwrap().name(),"read_content");
        assert_eq!(system.list_tools().len(),2);
        system.set_disabled_tools(vec!["read_platform_content".into()],vec![]);
        assert!(system.is_tool_disabled("read_content",AgentSide::Companion));
    }

    /// 规范化匹配**不能**替代别名表：它只去分隔符、不重排词序。
    ///
    /// 这条是上一个测试的前提说明——`set_wallpaper` → `setwallpaper`，
    /// `wallpaper_set` → `wallpaperset`，两者不等。所以不能指望规范化兜底。
    #[test]
    fn normalization_cannot_substitute_for_an_alias() {
        assert_ne!(
            normalize_tool_name("set_wallpaper"),
            normalize_tool_name("wallpaper_set")
        );
        // 但纯分隔符差异仍由规范化兜住
        assert_eq!(
            normalize_tool_name("Wallpaper-Set"),
            normalize_tool_name("wallpaper_set")
        );
    }

    /// 目标没注册时必须**拒绝**注册别名，否则会埋下一个"看着有映射、
    /// 实际解析不到"的哑弹。
    #[test]
    fn alias_to_unregistered_target_is_refused() {
        let ts = system_with(&["wallpaper_set"]);
        assert!(!ts.register_alias("set_wallpaper", "no_such_tool"));
        assert!(ts.find_tool("set_wallpaper").is_none());
    }

    #[test]
    fn alias_rejects_empty_and_self_reference() {
        let ts = system_with(&["wallpaper_set"]);
        assert!(!ts.register_alias("", "wallpaper_set"));
        assert!(!ts.register_alias("wallpaper_set", ""));
        assert!(!ts.register_alias("wallpaper_set", "wallpaper_set"));
        assert!(!ts.register_alias("  ", "wallpaper_set"));
    }

    /// 批量注册只统计成功项：一个目标没注册不该把其余别名一起丢掉。
    #[test]
    fn batch_alias_registration_counts_only_successes() {
        let ts = system_with(&["wallpaper_set", "list_dir"]);
        let n = ts.register_aliases(&[
            ("set_wallpaper", "wallpaper_set"),
            ("list_directory", "list_dir"),
            ("grep", "grep_search"), // 目标未注册 → 应被跳过
        ]);
        assert_eq!(n, 2);
        assert!(ts.find_tool("set_wallpaper").is_some());
        assert!(ts.find_tool("list_directory").is_some());
        assert!(ts.find_tool("grep").is_none());
    }

    #[test]
    fn locked_tools_remain_enabled_when_addressed_by_an_alias() {
        let system = system_with(&["read_file"]);
        system.register_alias("legacy_read", "read_file");
        system.set_disabled_tools(Vec::new(), vec!["legacy_read".into()]);
        assert!(!system.is_tool_disabled("legacy_read", AgentSide::Work));
        assert!(!system.is_tool_disabled("read_file", AgentSide::Work));
    }

    #[test]
    fn concurrent_catalog_reads_and_hot_reloads_finish_without_lock_inversion() {
        let system = Arc::new(system_with(&["read_file", "wallpaper_set"]));
        let (tx, rx) = std::sync::mpsc::channel();
        for writer in [true, false, false] {
            let system = system.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                for _ in 0..300 {
                    if writer {
                        system.register_tool_with_owner(Arc::new(NamedTool("reload")), Some("plugin".into()));
                        system.register_alias("old_reload", "reload");
                        system.unregister_tool_if_owner("reload", "plugin");
                    } else {
                        system.find_tool("old_reload");
                        system.list_tools_by_category(ToolCategory::System);
                        system.list_tools_for_side(AgentSide::Companion);
                        system.get_tool_schemas();
                    }
                }
                tx.send(()).unwrap();
            });
        }
        for _ in 0..3 { rx.recv_timeout(std::time::Duration::from_secs(10)).expect("catalog deadlock"); }
        assert_eq!(system.list_tool_names(), ["read_file", "wallpaper_set"]);
    }
}
