//! 工具注册表 - 管理工具的注册与查找
//!
//! `ToolSystem` 是工具系统的统一入口，整合注册表、缓存、沙箱、
//! 可观测性等组件，并提供线程安全的工具查找接口。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use super::cache::ToolCache;
use super::confirmation::{
    ConfirmationRequest, ConfirmationResponse, ConfirmationRisk, ToolConfirmationRegistry,
};
use super::observability::ToolObservability;
use super::sandbox::{ProtectionMode, ToolSandbox};
use super::types::{Tool, ToolCategory, ToolDefinition, ToolScene};

/// 工具系统 - 整合所有工具组件的统一入口
pub struct ToolSystem {
    /// 已注册的工具（工具名 → 工具实例）
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
    /// 工具别名（别名 → 工具名）
    aliases: RwLock<HashMap<String, String>>,
    /// 按类别组织的工具名
    categories: RwLock<HashMap<ToolCategory, Vec<String>>>,
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
    /// 用户禁用的工具名集合（来自 `config.tools.disabled_tools`）
    ///
    /// 禁用的工具不注入 LLM 工具列表（prompt 文本 / FC tools / 编程智能体 schema），
    /// 执行入口直接拒绝。`list_tools`（设置界面用）不受影响。
    disabled_tools: RwLock<std::collections::HashSet<String>>,
    /// Cordis 运行时上下文引用（策略缝 guard / post-execute 的分发目标）。
    ///
    /// 由 AppState 初始化时注入；用于 `execute_tool_use` 在 pre/post 阶段
    /// 派发可插桩的策略瀑布。为 `None` 时策略缝静默跳过，保持兼容旧调用。
    policy_ctx: RwLock<Option<Arc<crate::cordis::RuntimeContext>>>,
}

impl ToolSystem {
    /// 创建新的工具系统（使用默认配置：缓存 TTL=300s / max=1000 / 确认超时=600s）
    pub fn new() -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
            aliases: RwLock::new(HashMap::new()),
            categories: RwLock::new(HashMap::new()),
            cache: Arc::new(ToolCache::new(300, 1000)),
            sandbox: Arc::new(ToolSandbox::new(ProtectionMode::Cautious, 600)),
            observability: Arc::new(ToolObservability::new(1000)),
            confirmation: Arc::new(ToolConfirmationRegistry::new()),
            app_handle: RwLock::new(None),
            confirmation_timeout_secs: RwLock::new(600),
            last_tool_call_at: RwLock::new(None),
            disabled_tools: RwLock::new(std::collections::HashSet::new()),
            policy_ctx: RwLock::new(None),
        }
    }

    /// 使用 ToolConfig 创建工具系统（缓存 TTL/max_size 与确认超时从配置读取）
    ///
    /// 当 `enable_cache=false` 时，TTL 设为 0 使所有条目立即过期（等效禁用缓存）。
    pub fn with_tool_config(
        cache_ttl_secs: u64,
        cache_max_size: u32,
        enable_cache: bool,
        confirmation_timeout_secs: u64,
    ) -> Self {
        let effective_ttl = if enable_cache { cache_ttl_secs } else { 0 };
        Self {
            tools: RwLock::new(HashMap::new()),
            aliases: RwLock::new(HashMap::new()),
            categories: RwLock::new(HashMap::new()),
            cache: Arc::new(ToolCache::new(effective_ttl, cache_max_size as usize)),
            sandbox: Arc::new(ToolSandbox::new(ProtectionMode::Cautious, 600)),
            observability: Arc::new(ToolObservability::new(1000)),
            confirmation: Arc::new(ToolConfirmationRegistry::new()),
            app_handle: RwLock::new(None),
            confirmation_timeout_secs: RwLock::new(confirmation_timeout_secs.max(10)),
            last_tool_call_at: RwLock::new(None),
            disabled_tools: RwLock::new(std::collections::HashSet::new()),
            policy_ctx: RwLock::new(None),
        }
    }

    /// 更新用户禁用的工具名集合（设置保存 / 启动加载时调用）
    ///
    /// 直接整体替换：设置界面按 `config.tools.disabled_tools` 全量写入。
    pub fn set_disabled_tools(&self, names: Vec<String>) {
        *self.disabled_tools.write() = names.into_iter().collect();
    }

    /// 查询工具是否被用户禁用
    pub fn is_tool_disabled(&self, name: &str) -> bool {
        self.disabled_tools.read().contains(&name.to_string())
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
        let (id, rx) = self.confirmation.create_request(request.clone());
        let request = ConfirmationRequest { request_id: id, ..request };

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
        let timeout_secs = *self.confirmation_timeout_secs.read();
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

    /// 注册工具
    pub fn register_tool(&self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        let category = tool.category();
        tracing::debug!("注册工具: {} (类别: {:?})", name, category);

        let mut tools = self.tools.write();
        tools.insert(name.clone(), tool);

        let mut categories = self.categories.write();
        let list = categories.entry(category).or_insert_with(Vec::new);
        // 幂等：同名重复注册（自建工具更新/热重载）时只保留一条类别记录
        if !list.contains(&name) {
            list.push(name);
        }
    }

    /// 注销工具
    pub fn unregister_tool(&self, name: &str) -> bool {
        let mut tools = self.tools.write();
        if tools.remove(name).is_some() {
            let mut aliases = self.aliases.write();
            aliases.retain(|_, v| v != name);

            let mut categories = self.categories.write();
            for names in categories.values_mut() {
                names.retain(|n| n != name);
            }
            tracing::debug!("已注销工具: {}", name);
            true
        } else {
            false
        }
    }

    /// 查找工具
    ///
    /// 匹配顺序：
    /// 1. 精确匹配工具名
    /// 2. 别名匹配
    /// 3. 规范化匹配（lowercase + 去除 `_`/`-`/`.` 分隔符），容错 LLM 输出大小写或分隔符偏差
    pub fn find_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        let tools = self.tools.read();
        if let Some(tool) = tools.get(name) {
            return Some(Arc::clone(tool));
        }
        let aliases = self.aliases.read();
        if let Some(real_name) = aliases.get(name) {
            if let Some(tool) = tools.get(real_name) {
                return Some(Arc::clone(tool));
            }
        }
        // 规范化匹配：lowercase + 去分隔符，容错 LLM 输出偏差
        let normalized = normalize_tool_name(name);
        if normalized != name {
            for (key, tool) in tools.iter() {
                if normalize_tool_name(key) == normalized {
                    return Some(Arc::clone(tool));
                }
            }
        }
        None
    }

    /// 是否存在指定工具
    pub fn has_tool(&self, name: &str) -> bool {
        self.find_tool(name).is_some()
    }

    /// 列出所有工具
    pub fn list_tools(&self) -> Vec<Arc<dyn Tool>> {
        let tools = self.tools.read();
        tools.values().map(Arc::clone).collect()
    }

    /// 按场景列出工具
    ///
    /// 设计变更：不再做场景黑名单硬过滤（旧 `blocked_tools()` 已废弃）。
    /// 所有注册工具均返回，由 LLM 自主判断是否调用，危险操作通过
    /// `check_permissions` 在执行时确认。场景信息通过 `ToolScene::soft_hint()`
    /// 注入 prompt 作为软提示，引导但不强制 LLM 的工具选择。
    /// 延迟加载由 `should_defer` 控制（在 `tool_call_manager` 中分流）。
    ///
    /// 用户在设置中禁用的工具（`config.tools.disabled_tools`）在此过滤，
    /// 不进入 LLM 的工具列表。
    pub fn list_tools_for_scene(&self, _scene: ToolScene) -> Vec<Arc<dyn Tool>> {
        let tools = self.tools.read();
        let disabled = self.disabled_tools.read();
        tools
            .values()
            .filter(|t| !disabled.contains(t.name()))
            .map(Arc::clone)
            .collect()
    }

    /// 列出所有工具名
    pub fn list_tool_names(&self) -> Vec<String> {
        let tools = self.tools.read();
        tools.keys().cloned().collect()
    }

    /// 按类别列出工具
    pub fn list_tools_by_category(&self, category: ToolCategory) -> Vec<Arc<dyn Tool>> {
        let categories = self.categories.read();
        let tools = self.tools.read();
        categories
            .get(&category)
            .map(|names| {
                names
                    .iter()
                    .filter_map(|n| tools.get(n).map(Arc::clone))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 获取所有类别
    pub fn get_categories(&self) -> Vec<ToolCategory> {
        self.categories.read().keys().copied().collect()
    }

    /// 获取所有工具的 schema 定义
    ///
    /// 用户禁用的工具（`config.tools.disabled_tools`）被过滤
    /// （编程智能体的 FC tools 字段由此生成）。
    pub fn get_tool_schemas(&self) -> Vec<ToolDefinition> {
        let tools = self.tools.read();
        let disabled = self.disabled_tools.read();
        tools
            .values()
            .filter(|t| !disabled.contains(t.name()))
            .map(|t| t.to_definition())
            .collect()
    }

    /// 获取所有工具的 OpenAI 格式定义
    pub fn get_openai_tools(&self) -> Vec<Value> {
        let tools = self.tools.read();
        tools.values().map(|t| t.to_openai_format()).collect()
    }

    /// 简单搜索工具（按名称/描述匹配）
    pub fn search(&self, query: &str) -> Vec<Arc<dyn Tool>> {
        let query_lower = query.to_lowercase();
        let tools = self.tools.read();
        tools
            .values()
            .filter(|t| {
                t.name().to_lowercase().contains(&query_lower)
                    || t.description().to_lowercase().contains(&query_lower)
            })
            .map(Arc::clone)
            .collect()
    }

    /// 清空所有工具
    pub fn clear(&self) {
        let mut tools = self.tools.write();
        tools.clear();
        self.aliases.write().clear();
        self.categories.write().clear();
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

/// 工具名规范化：lowercase + 去除 `_`/`-`/`.` 分隔符
///
/// 用于容错 LLM 输出的工具名偏差，例如 `Wallpaper_List` / `wallpaper-list` /
/// `WALLPAPER.LIST` 都能匹配到 `wallpaper_list`。
pub fn normalize_tool_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| *c != '_' && *c != '-' && *c != '.')
        .collect()
}
