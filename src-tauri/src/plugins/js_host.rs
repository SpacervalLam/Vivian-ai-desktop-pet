//! JS 插件宿主桥 —— 把插件目录中的 JS 代码接入宿主（provider 全链路插件化）
//!
//! 职责与装载语义：
//! - 插件 manifest 的 `js` 字段声明 JS 入口 glob（如 `["js/*.js"]` 或 `["js"]`），
//!   展开规则与 skills/tools 一致（一层 `*.js` 或整目录递归）
//! - 每个含 JS 的插件独占一个 [`JsRuntime`]（独立线程 + 独立全局对象与
//!   `ctx` 命名空间——插件间故障隔离，一个插件抛异常不影响其余插件）
//! - 贡献桥接（对齐 Cordis 的 `ctx.x()` 注册面）：
//!   - **Provider**：`ctx.provider(type, spec)` 贡献的 ProtocolSpec 编译后进入
//!     `protocol_registry` 的动态覆盖层，factory 按新 provider_type 路由到
//!     `DeclarativeProvider` —— **新协议纯靠插件接入，无需改 Rust**
//!   - **Skill**：`ctx.skill(name, desc, body)` 贡献经装载报告交给上层注册进
//!     SkillService（命名空间 `<plugin>/<skill>`）
//!   - **Mcp**：`ctx.mcp(id, config)` 贡献解析为 McpServerConfig 后由上层合并
//!     进 McpManager（与 manifest 声明的 mcp_servers 同一信任与归属规则）
//!   - **Tool**：`ctx.tool(name, desc, schema, handler)` 注册为 [`JsPluginTool`]，
//!     执行经 [`call_plugin_tool`] 路由回本插件 worker（与文件工具同走
//!     ToolSystem 的 owner 归属与卸载语义）
//! - 全局单例：`load_all_js`（启动全量）/ `reload_one` / `unload_one`
//!   （运行时装载/卸载，与 plugins.rs 的 load_one/unload_one 联动）
//!
//! 动态协议优先级：JS 贡献 > 插件 `protocols/*.json` 文件 > 内置兜底
//! （与「后到覆盖先到」一致——代码级贡献是最强的定制点）。
//! 原生 provider_type 白名单在 factory 层拦截，JS 插件不能劫持原生协议。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use async_trait::async_trait;
use serde_json::Value;

use crate::providers::js_runtime::{JsContribution, JsRuntime, JsSourceFile};
use crate::providers::protocol_registry;
use crate::providers::spec::{CompiledSpec, ProtocolSpec};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};

/// JS 插件装载报告（诊断 / 设置页展示 / 上层注册用）
#[derive(Debug, Default, Clone)]
pub struct JsLoadReport {
    /// 装载了 JS 的插件名
    pub plugins: Vec<String>,
    /// 注册为动态协议的 provider_type
    pub protocols: Vec<String>,
    /// ctx.skill 贡献（插件名，完整命名空间名，描述，正文）——由上层注册进 SkillService
    pub skills: Vec<(String, String, String, String)>,
    /// ctx.mcp 贡献（插件名，配置）——由上层合并进 McpManager
    pub mcp: Vec<(String, crate::tools::mcp::McpServerConfig)>,
    /// ctx.tool 贡献（插件名，注册名，描述，参数 schema）——由上层注册进 ToolSystem，
    /// 执行经 [`call_plugin_tool`] 路由回本插件 worker
    pub tools: Vec<(String, String, String, serde_json::Value)>,
    /// 插件名 → 目录 key（上层快照归属用）
    pub plugin_keys: HashMap<String, String>,
    /// 跳过/失败的插件（诊断用）
    pub skipped: Vec<String>,
}

/// 宿主单例：目录 key → 该插件的 JS 运行时 + 插件名 → 目录 key 反查表。
struct Host {
    runtimes: HashMap<String, JsRuntime>,
    by_name: HashMap<String, String>,
}

static HOST: OnceLock<Mutex<Host>> = OnceLock::new();

fn host() -> &'static Mutex<Host> {
    HOST.get_or_init(|| Mutex::new(Host::empty()))
}

impl Host {
    fn empty() -> Self {
        Host {
            runtimes: HashMap::new(),
            by_name: HashMap::new(),
        }
    }
}

/// 调用某插件 ctx.tool 注册的处理函数（工具执行入口）。
///
/// 由 JsPluginTool 适配器的 `call` 调用；插件名 → 运行时经宿主反查表定位。
pub fn call_plugin_tool(plugin: &str, tool_name: &str, args: serde_json::Value) -> Result<String, String> {
    let h = host().lock().unwrap();
    let Some(key) = h.by_name.get(plugin) else {
        return Err(format!("插件 {plugin} 未装载 JS 运行时（可能未信任或已卸载）"));
    };
    let Some(rt) = h.runtimes.get(key) else {
        return Err(format!("插件 {plugin} 的 JS 运行时不存在"));
    };
    rt.call_tool(tool_name, args)
}

/// 把一条 JS Provider 贡献编译为 CompiledSpec（失败告警并跳过该条）。
fn compile_provider(plugin: &str, reg: &crate::providers::js_runtime::JsProviderReg) -> Option<CompiledSpec> {
    let spec = match serde_json::from_value::<ProtocolSpec>(reg.spec.clone()) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                "[JsHost] 插件 {plugin} 的协议 `{}` spec 不合法（缺 id/provider_type/request？）: {e}",
                reg.provider_type
            );
            return None;
        }
    };
    if spec.provider_type != reg.provider_type {
        tracing::warn!(
            "[JsHost] 插件 {plugin} 协议声明的 provider_type=`{}` 与 spec.provider_type=`{}` 不一致，以 spec 为准",
            reg.provider_type,
            spec.provider_type
        );
    }
    match CompiledSpec::compile(spec) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!("[JsHost] 插件 {plugin} 协议编译失败: {e}");
            None
        }
    }
}

/// 汇总宿主内全部运行时的贡献 → (specs，skills，mcps，tools)。
/// 插件名字典序遍历保证覆盖顺序稳定（与文件协议装载语义一致）。
fn collect_contributions(
    host: &Host,
) -> (
    Vec<CompiledSpec>,
    Vec<(String, String, String, String)>,
    Vec<(String, crate::tools::mcp::McpServerConfig)>,
    Vec<(String, String, String, serde_json::Value)>,
) {
    let mut specs: Vec<CompiledSpec> = Vec::new();
    let mut skills = Vec::new();
    let mut mcps = Vec::new();
    let mut tools = Vec::new();
    let mut names: Vec<&String> = host.runtimes.keys().collect();
    names.sort();
    for key in names {
        let plugin = host.by_name.get(key).cloned().unwrap_or_else(|| key.clone());
        for c in host.runtimes[key].contributions() {
            match c {
                JsContribution::Provider(reg) => {
                    if let Some(c) = compile_provider(&plugin, &reg) {
                        specs.push(c);
                    }
                }
                JsContribution::Skill(reg) => {
                    if reg.name.is_empty() {
                        tracing::warn!("[JsHost] 插件 {plugin} 的 ctx.skill 贡献缺 name，跳过");
                        continue;
                    }
                    let full = format!("{}/{}", reg.namespace, reg.name);
                    skills.push((plugin.clone(), full, reg.description, reg.body));
                }
                JsContribution::Mcp(reg) => {
                    match serde_json::from_value::<crate::tools::mcp::McpServerConfig>(reg.config)
                    {
                        Ok(mut config) => {
                            if config.id.is_empty() {
                                config.id = reg.id.clone();
                            }
                            if config.name.is_empty() {
                                config.name = reg.id.clone();
                            }
                            mcps.push((plugin.clone(), config));
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[JsHost] 插件 {plugin} 的 ctx.mcp 贡献 `{}` 配置不合法（缺 command？）: {e}",
                                reg.id
                            );
                        }
                    }
                }
                JsContribution::Tool(reg) => {
                    if reg.name.is_empty() {
                        tracing::warn!("[JsHost] 插件 {plugin} 的 ctx.tool 贡献缺 name，跳过");
                        continue;
                    }
                    tools.push((plugin.clone(), reg.name, reg.description, reg.parameters));
                }
            }
        }
    }
    (specs, skills, mcps, tools)
}

/// 动态协议注册表重建（供 load_all_js / reload_one / unload_one 共用）。
fn rebuild_dynamic(host: &Host) -> Vec<String> {
    let (specs, ..) = collect_contributions(host);
    protocol_registry::replace_dynamic_specs(specs)
}

/// 读取插件目录的 JS 入口（manifest `js` glob 展开），返回 (插件名, 源文件集)。
fn entry_sources(pdir: &std::path::Path) -> Option<(String, Vec<JsSourceFile>)> {
    let (name, files) = crate::plugins::js_plugin_entry(pdir)?;
    let sources: Vec<JsSourceFile> = files
        .into_iter()
        .filter_map(|path| {
            std::fs::read_to_string(&path)
                .ok()
                .map(|source| JsSourceFile { path, source })
        })
        .collect();
    if sources.is_empty() {
        None
    } else {
        Some((name, sources))
    }
}

/// 装载单个插件到宿主（替换旧运行时），返回贡献计数。
fn mount(host: &mut Host, pdir: &std::path::Path, report: &mut JsLoadReport) {
    let key = pdir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    // 重载语义：先关旧运行时（幂等），并清理指向旧 key 的 by_name 映射
    if let Some(old) = host.runtimes.remove(&key) {
        old.shutdown();
        host.by_name.retain(|_, v| *v != key);
    }
    let Some((name, sources)) = entry_sources(pdir) else {
        return; // 未声明 JS 或入口为空：非 JS 插件，正常路径
    };
    // 信任门禁：与 plugins::load_all 同一规则（JS 是可执行代码，门禁同强度）
    if !crate::plugins::is_plugin_dir_trusted(pdir) {
        report
            .skipped
            .push(format!("{name}: 未信任，JS 未装载——请在设置 → 插件页信任后重载"));
        return;
    }
    let version = std::fs::read_to_string(pdir.join("plugin.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("version").and_then(|v| v.as_str()).map(String::from))
        .unwrap_or_else(|| "1.0.0".to_string());
    let rt = JsRuntime::new(pdir.to_path_buf(), name.clone(), version, false);
    match rt.reload_with(sources) {
        Ok(_count) => {
            let contribs = rt.contributions();
            let mut plugin_skills = Vec::new();
            let mut plugin_mcp = Vec::new();
            let mut plugin_tools = Vec::new();
            for c in &contribs {
                match c {
                    JsContribution::Tool(reg) => {
                        if reg.name.is_empty() {
                            tracing::warn!("[JsHost] 插件 {name} 的 ctx.tool 贡献缺 name，跳过");
                            continue;
                        }
                        plugin_tools.push((
                            name.clone(),
                            reg.name.clone(),
                            reg.description.clone(),
                            reg.parameters.clone(),
                        ));
                    }
                    JsContribution::Skill(reg) => {
                        if !reg.name.is_empty() {
                            plugin_skills.push((
                                name.clone(),
                                format!("{}/{}", reg.namespace, reg.name),
                                reg.description.clone(),
                                reg.body.clone(),
                            ));
                        }
                    }
                    JsContribution::Mcp(reg) => {
                        if let Ok(mut config) =
                            serde_json::from_value::<crate::tools::mcp::McpServerConfig>(
                                reg.config.clone(),
                            )
                        {
                            if config.id.is_empty() {
                                config.id = reg.id.clone();
                            }
                            if config.name.is_empty() {
                                config.name = reg.id.clone();
                            }
                            plugin_mcp.push((name.clone(), config));
                        } else {
                            report.skipped.push(format!(
                                "{name}: ctx.mcp 贡献 `{}` 配置不合法，已跳过",
                                reg.id
                            ));
                        }
                    }
                    JsContribution::Provider(_) => {} // rebuild_dynamic 统计
                }
            }
            report.skills.extend(plugin_skills);
            report.mcp.extend(plugin_mcp);
            report.tools.extend(plugin_tools);
            report.plugin_keys.insert(name.clone(), key.clone());
            // 关键修复：登记 插件名 -> 目录 key 映射（by_name 查找入口）。
            // 必须在此处插入——下一行起 name/key 会被 move 走。
            host.by_name.insert(name.clone(), key.clone());
            tracing::info!(
                "[JsHost] 插件 {name} 的 JS 已装载（{} 条贡献）",
                contribs.len()
            );
            report.plugins.push(name);
            host.runtimes.insert(key, rt);
        }
        Err(e) => {
            report.skipped.push(format!("{name}: {e}"));
            rt.shutdown();
        }
    }
}

/// 启动路径：全量装载所有声明了 JS 的插件，重建动态协议注册表。
///
/// 由 `plugins::load_all`（AppState 构造期）调用；幂等——旧运行时全部关闭重建。
pub fn load_all_js() -> JsLoadReport {
    let mut report = JsLoadReport::default();
    let root = crate::plugins::plugins_dir();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return report;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("plugin.json").exists())
        .collect();
    dirs.sort();

    let mut h = host().lock().unwrap();
    for (_, rt) in h.runtimes.drain() {
        rt.shutdown();
    }
    h.by_name.clear();
    for pdir in dirs {
        mount(&mut h, &pdir, &mut report);
    }
    report.protocols = rebuild_dynamic(&h);
    // 文件协议缓存一并失效（运行时装载的插件可能同时带 protocols/*.json）
    protocol_registry::invalidate();
    report
}

/// 运行时装载（或重载）单个插件的 JS（`plugins::load_one` 联动入口）。
///
/// 插件未声明 JS 时等价于卸载该插件的 JS 贡献（重载语义）。
pub fn reload_one(key: &str) -> JsLoadReport {
    let mut report = JsLoadReport::default();
    let pdir = crate::plugins::plugins_dir().join(key);
    let mut h = host().lock().unwrap();
    mount(&mut h, &pdir, &mut report);
    report.protocols = rebuild_dynamic(&h);
    protocol_registry::invalidate();
    report
}

/// 卸载单个插件的 JS 运行时贡献（`plugins::unload_one` / `delete_plugin` 联动）。
pub fn unload_one(key: &str) {
    let mut h = host().lock().unwrap();
    if let Some(old) = h.runtimes.remove(key) {
        old.shutdown();
    }
    h.by_name.retain(|_, v| v.as_str() != key);
    rebuild_dynamic(&h);
}

/// 收集当前全部运行时捕获的 ctx.tool 贡献（启动工具注册入口用）。
///
/// 返回 `(插件名, 目录 key, 工具名, 描述, 参数 schema)`；插件名字典序遍历，
/// 顺序稳定。注册动作由 `plugins::load_all_tools` 完成（须在内置工具注册之后，
/// 防影子化校验才有效）——本函数只读宿主快照，无副作用。
pub fn captured_tools() -> Vec<(String, String, String, String, Value)> {
    let h = host().lock().unwrap();
    let mut keys: Vec<&String> = h.runtimes.keys().collect();
    keys.sort();
    let mut out = Vec::new();
    for key in keys {
        let plugin = h.by_name.get(key).cloned().unwrap_or_else(|| key.clone());
        for c in h.runtimes[key].contributions() {
            if let JsContribution::Tool(reg) = c {
                if reg.name.is_empty() {
                    continue;
                }
                out.push((
                    plugin.clone(),
                    key.clone(),
                    reg.name.clone(),
                    reg.description.clone(),
                    reg.parameters.clone(),
                ));
            }
        }
    }
    out
}

// ============================================================================
// JsPluginTool —— ctx.tool 贡献 → Tool trait 适配器
// ============================================================================

/// JS 插件工具：执行经 [`call_plugin_tool`] 路由回插件 worker 线程。
///
/// 风险分级 Safe：boa 运行时未向插件暴露任何 I/O 能力（文件 / 网络 / 进程
/// 皆不可达），处理函数只能做纯计算——危险面在装载期（`ctx.mcp` 启动
/// MCP server、`ctx.provider` 声明请求模板），由信任门禁管辖，不在工具
/// 调用期。参数校验复用文件工具的 JSON Schema 校验（同一套语义）。
pub struct JsPluginTool {
    plugin: String,
    name: String,
    description: String,
    parameters: Value,
    hint: String,
}

impl JsPluginTool {
    pub fn new(plugin: String, name: String, description: String, parameters: Value) -> Self {
        let hint: String = description.chars().take(30).collect();
        Self { plugin, name, description, parameters, hint }
    }
}

#[async_trait]
impl Tool for JsPluginTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.parameters.clone()
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        crate::tools::custom_tools::validate_tool_input(&self.parameters, input)
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _context: &ToolUseContext) -> ToolResult {
        let plugin = self.plugin.clone();
        let name = self.name.clone();
        // call_plugin_tool 阻塞等待 worker 应答（上限 CALL_TOOL_TIMEOUT），
        // spawn_blocking 承载，不占调用方的异步 worker 线程
        match tokio::task::spawn_blocking(move || call_plugin_tool(&plugin, &name, args)).await {
            Ok(Ok(text)) => ToolResult::success(serde_json::json!({ "output": text })),
            Ok(Err(e)) => ToolResult::standard_error(&e, Some("JsPluginTool"), None),
            Err(e) => {
                ToolResult::standard_error(&format!("JS 工具任务执行失败: {e}"), Some("JsPluginTool"), None)
            }
        }
    }

    fn is_read_only(&self) -> bool {
        // 保守默认：处理函数行为由插件代码决定，视为非只读
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn is_custom(&self) -> bool {
        true
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn should_defer(&self) -> bool {
        // 与 MCP 工具一致：延迟加载，避免 prompt 膨胀
        true
    }

    fn search_hint(&self) -> &str {
        &self.hint
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造临时 JS 插件目录（plugin.json 声明 js 入口 + 一个贡献 acme 协议的脚本）
    fn acme_plugin_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vivian-jshost-{}", uuid::Uuid::new_v4()));
        let js_dir = dir.join("js");
        std::fs::create_dir_all(&js_dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"acme-js","version":"1.0.0","description":"t","js":["js/*.js"]}"#,
        )
        .unwrap();
        std::fs::write(
            js_dir.join("index.js"),
            r#"
            ctx.provider('acme', {
              id: 'acme',
              provider_type: 'acme',
              auth: { type: 'header', scheme: 'Bearer', value_from: 'api_key' },
              transport: { method: 'POST', path: '/v1/chat' },
              request: {
                message_format: 'chat_completions',
                body: {
                  model: '{model}',
                  messages: '{messages}',
                  temperature: '{temperature}',
                  stream: '{stream}'
                }
              },
              response: { content: '$.choices[0].message.content' }
            });
            ctx.log('info', 'acme plugin loaded');
            "#,
        )
        .unwrap();
        dir
    }

    /// manifest `js` glob 展开与入口读取
    #[test]
    fn entry_sources_reads_js_plugin() {
        let dir = acme_plugin_dir();
        let (name, sources) = entry_sources(&dir).expect("应识别 JS 入口");
        assert_eq!(name, "acme-js");
        assert_eq!(sources.len(), 1);
        assert!(sources[0].source.contains("ctx.provider"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 动态协议注册表是全局的（protocol_registry 单例），操作它的测试必须
    /// 串行——并行下 A 测试的清理会清掉 B 测试刚注册的协议。
    fn registry_lock() -> std::sync::MutexGuard<'static, ()> {
        static REGISTRY_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        REGISTRY_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    /// 端到端桥：装载 JS 插件 → 动态协议注册表命中 → DeclarativeProvider 可创建
    #[test]
    fn js_provider_contribution_registers_dynamic_spec() {
        let _registry = registry_lock();
        let dir = acme_plugin_dir();
        // 直接构造 Host 局部验证桥接（不触全局单例，避免污染其他测试）
        let mut h = Host::empty();
        let mut report = JsLoadReport::default();
        crate::plugins::authorize_plugin_for_tests(&dir);
        mount(&mut h, &dir, &mut report);
        assert_eq!(report.plugins.len(), 1);
        assert!(report.skipped.is_empty());

        let registered = rebuild_dynamic(&h);
        assert!(registered.contains(&"acme".to_string()), "acme 协议应注册");
        // 注册表可按 provider_type 查到（动态层优先于文件层）
        let spec = protocol_registry::spec_for("acme").expect("动态协议应可查询");
        assert_eq!(spec.spec.id, "acme");
        assert_eq!(spec.spec.request.message_format, crate::providers::spec::MessageFormat::ChatCompletions);

        // 清理全局动态层（测试隔离）
        protocol_registry::replace_dynamic_specs(Vec::new());
        for (_, rt) in h.runtimes.drain() {
            rt.shutdown();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 端到端：JS 插件贡献协议 → 动态注册表 → DeclarativeProvider
    /// 经真实 HTTP 请求跑通 call_chat（本地 mock server）。
    ///
    /// 验证整条链路：ctx.provider(spec) → 编译注册 → factory 同款 spec 查询 →
    /// 请求体构造（占位符替换 + 消息序列化）→ 鉴权头 → 响应路径提取。
    #[tokio::test]
    async fn js_provider_end_to_end_call_chat() {
        // 全局动态协议注册表的操作段与其他测试串行（锁跨 await 安全：
        // 只有两个注册表测试竞争，无嵌套锁）
        let _registry = registry_lock();
        use crate::config::manager::ProviderConfig;
        use crate::providers::{BaseProvider, DeclarativeProvider};
        use crate::types::response::ChatMessage;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // 1. 装载 JS 插件，协议进动态注册表
        let dir = acme_plugin_dir();
        let mut h = Host::empty();
        let mut report = JsLoadReport::default();
        crate::plugins::authorize_plugin_for_tests(&dir);
        mount(&mut h, &dir, &mut report);
        assert!(report.skipped.is_empty(), "JS 装载不应失败: {:?}", report.skipped);
        rebuild_dynamic(&h);
        let spec = protocol_registry::spec_for("acme").expect("acme 动态协议应可查询");

        // 2. 本地 mock HTTP server：收一个请求，回 chat_completions 风格 JSON
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 16384];
            let mut read = 0usize;
            // 读到请求头结束且 body 按 Content-Length 收满
            loop {
                let n = sock.read(&mut buf[read..]).await.unwrap();
                if n == 0 {
                    break;
                }
                read += n;
                let req = String::from_utf8_lossy(&buf[..read]);
                if let Some(pos) = req.find("\r\n\r\n") {
                    let body_len: usize = req
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    if req.len() - pos - 4 >= body_len {
                        break;
                    }
                }
            }
            let captured = String::from_utf8_lossy(&buf[..read]).to_string();
            let resp_body = r#"{"choices":[{"message":{"content":"hello from acme"},"finish_reason":"stop"}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp_body.len(),
                resp_body
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.shutdown().await.ok();
            captured
        });

        // 3. DeclarativeProvider 指向 mock server（直连客户端避免系统代理干扰）
        let config = ProviderConfig {
            base_url: format!("http://{addr}"),
            api_key: "sk-acme-test".into(),
            model: "acme-1".into(),
        };
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let provider =
            DeclarativeProvider::new(&config, 0.7, 512, None, Some(client), spec, String::new());

        let msgs = vec![ChatMessage {
            role: "user".into(),
            content: "hi".into(),
            images: None,
            tool_calls: None,
            tool_call_id: None,
            reasoning: None,
            timestamp: None,
            meta: None,
        }];
        let out = provider.call_chat(msgs).await.expect("call_chat 应成功");
        assert_eq!(out, "hello from acme");

        // 4. 校验 mock 收到的请求：路径 / 鉴权头 / 模型 / 消息序列化 / 流式开关
        let captured = server.await.unwrap();
        assert!(
            captured.starts_with("POST /v1/chat HTTP/1.1"),
            "请求行应为 POST /v1/chat，实际: {}",
            captured.lines().next().unwrap_or_default()
        );
        // 头名大小写不敏感（reqwest 发送时统一小写化），断言用小写比较
        let captured_lower = captured.to_ascii_lowercase();
        assert!(
            captured_lower.contains("authorization: bearer sk-acme-test"),
            "应按 spec 鉴权声明发送 Bearer 头，实际: {captured}"
        );
        assert!(captured.contains(r#""model":"acme-1""#), "model 占位符应替换");
        // 键序无关断言：serde_json 默认 BTreeMap（字母序），不保证 role 在前
        assert!(captured.contains(r#""messages":"#), "messages 字段应存在: {captured}");
        assert!(captured.contains(r#""role":"user""#), "user 角色应存在: {captured}");
        assert!(captured.contains(r#""content":"hi""#), "消息内容应存在: {captured}");
        assert!(captured.contains(r#""stream":false"#), "非流式调用 stream=false");

        // 清理
        protocol_registry::replace_dynamic_specs(Vec::new());
        for (_, rt) in h.runtimes.drain() {
            rt.shutdown();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
