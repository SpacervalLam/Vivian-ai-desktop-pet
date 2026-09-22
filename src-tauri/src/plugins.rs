//! 插件贡献点体系 —— 从 `<用户数据目录>/plugins/` 装载插件声明的能力
//!
//! 支持四类贡献点：
//! - **skills**：插件目录下的 *.md 技能文件，以 `<plugin>/<skill>` 命名注册进
//!   SkillService（命名空间隔离，不与用户技能冲突），对所有角色可见
//! - **tools**：插件目录下的 *.json 工具定义（格式同自建工具 CustomToolDef），
//!   经 DynamicTool 适配器注册进 ToolSystem——插件由此贡献**可执行能力**
//! - **mcp_servers**：stdio MCP server 声明，按 id 去重合并进 servers.json
//!   （用户已有的同 id 配置优先，插件不覆盖；条目携带 `source_plugin` 归属，
//!   供运行时按插件撤销；仅受信插件参与合并，连接前另有信任复核），由
//!   init_all 统一连接
//! - **providers**：LLM 供应商预设数据文件（如 providers.json），设置 → LLM 页
//!   厂商卡片的数据源；前端按 id 与内置兜底合并（插件覆盖同名项），
//!   按需读取（每次查询重新读盘），编辑后重开设置即生效
//!
//! 插件格式：`plugins/<name>/plugin.json`
//! ```json
//! {
//!   "name": "my-plugin",
//!   "version": "1.0.0",
//!   "description": "示例插件",
//!   "skills": ["skills/*.md"],
//!   "tools": ["tools/*.json"],
//!   "mcp_servers": [{ "id": "fs", "name": "文件系统", "command": "npx",
//!                     "args": ["-y", "@modelcontextprotocol/server-filesystem", "."] }],
//!   "providers": "providers.json"
//! }
//! ```
//!
//! 装载语义（信任门禁 > 用户/手动配置 > 插件声明）：
//! - **信任门禁**：插件首次进入或清单变更后处于未信任状态，其全部贡献
//!   （技能/工具/MCP/JS）不装载——防止向插件目录投放文件即获得本地执行能力；
//!   信任动作在设置窗口完成（记录清单指纹），清单再变更即回到未信任
//! - MCP id 冲突：保留现有配置，跳过插件声明（归属同插件的 id 例外——
//!   重载时以插件自身新声明为准）
//! - 技能命名空间化：`<plugin>/<skill_name>`，与用户目录技能天然无冲突，
//!   同名插件技能重复装载时原子替换（幂等，可热重载复用）
//! - 工具注册时防影子化：与内置/自建工具重名的定义跳过（后注册不得覆盖）
//! - 供应商预设按 id 先到保留（插件目录按名字典序，同 id 冲突跳过后者并记录）；
//!   内置 llm-providers 的预设 id 受保护，任何其他插件不得覆盖
//!
//! **运行时装载/卸载**（创造模式的落点）：
//! - [`load_one`]：装载（或重载）单个插件——先撤销该插件旧贡献（技能按
//!   命名空间前缀、工具按目录展开名单、MCP 按归属），再注册新贡献并连接
//!   新增 server。`create_plugin` 元工具落盘后调用它，注册即生效。
//! - [`unload_one`]：只撤销运行时贡献，不动磁盘文件。
//! - [`delete_plugin`]：撤销贡献并删除插件目录。
//! - 内置插件（llm-providers 等）禁改禁删——升级播种会覆盖手工修改，
//!   改内置的正确路径是复制为新插件再改。
//!
//! 内置插件 `llm-providers`（供应商预设 + 预设核对技能）编译期嵌入，启动时播种到
//! 用户插件目录；仅当磁盘版本号低于内置版本时覆盖升级（手工定制者保持自身
//! version ≥ 内置版本即可保留修改）。`plugin-authoring`（插件创作技能）同机制播种。
//!
//! 第五类贡献点 **js**（manifest `js` 字段，如 `["js/*.js"]`）：插件携带可执行
//! JS 代码，由 boa 宿主装载（[`js_host`]）——`ctx.provider()` 贡献声明式协议
//! （新 LLM 协议纯靠插件接入，无需改 Rust），`ctx.tool/skill/mcp` 分别注册进
//! ToolSystem（JsPluginTool 适配器）/ SkillService / McpManager。

pub mod js_host;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::skills::SkillService;
use crate::tools::custom_tools::{CustomToolDef, DynamicTool};
use crate::tools::mcp::McpServerConfig;
use crate::tools::registry::ToolSystem;
use crate::tools::McpManager;

/// 插件清单（plugins/<name>/plugin.json）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PluginManifest {
    /// 插件名（缺省取目录名）
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
    /// 技能文件 glob（相对插件目录），如 ["skills/*.md"]
    #[serde(default)]
    skills: Vec<String>,
    /// 工具定义文件 glob（相对插件目录），如 ["tools/*.json"]，
    /// 文件内容为 `CustomToolDef` 的 JSON 序列化
    #[serde(default)]
    tools: Vec<String>,
    /// MCP server 声明（按 id 去重合并）
    #[serde(default)]
    mcp_servers: Vec<McpServerConfig>,
    /// 供应商预设数据文件（相对插件目录，如 "providers.json"），
    /// 内容为 `ProviderPresetData` 的 JSON 数组
    #[serde(default)]
    providers: Option<String>,
    /// 云端嵌入服务预设数据文件（相对插件目录）
    #[serde(default)]
    embeddings: Option<String>,
    /// JS 插件入口 glob（相对插件目录，如 ["js/*.js"] 或 ["js"]）。
    /// 声明后由 js_host 创建独立 JS 运行时装载（ctx.skill/tool/provider/mcp 贡献面）
    #[serde(default)]
    js: Vec<String>,
}

/// 供应商预设的协议变体（对齐前端 `ProviderProtocol`，camelCase 序列化）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProtocolData {
    /// 后端 provider_type 值（openai / chat_completions / anthropic / …）
    pub provider_type: String,
    /// 协议显示名 i18n key（复用 config.proto_* 键）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_key: Option<String>,
    /// 直接显示名（第三方插件无 i18n 键时使用，优先于 label_key）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// 该协议的接口端点
    #[serde(default)]
    pub endpoint: String,
}

/// 供应商预设（对齐前端 `ProviderPreset`，camelCase 序列化）
///
/// 由插件 `providers.json` 声明，设置 → LLM 页厂商卡片按 id 与内置兜底合并；
/// id 是稳定标识（provider_cache 凭据快照按 id 索引），已发布的 id 不可变更。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPresetData {
    /// 稳定标识（如 "deepseek"），前端按此与内置预设合并去重
    pub id: String,
    /// 服务商名 i18n key（复用 config.preset_* 键）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_key: Option<String>,
    /// 直接显示名（第三方插件无 i18n 键时使用，优先于 label_key）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// 默认协议的 provider_type
    pub provider_type: String,
    /// 默认协议端点（base_url）
    #[serde(default)]
    pub endpoint: String,
    /// 默认模型 ID
    #[serde(default)]
    pub default_model: String,
    /// 模型名输入建议列表（datalist，仍可自由输入）
    #[serde(default)]
    pub main_models: Vec<String>,
    /// 上下文窗口（tokens），用于自动压缩阈值判定
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    /// 建议单次输出上限（主配置切换预设时写入 ai.max_tokens）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_max_tokens: Option<u32>,
    /// 是否需要 api_secret（文心等 OAuth/HMAC 鉴权）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_secret: Option<bool>,
    /// 是否需要 app_id
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_app_id: Option<bool>,
    /// 供应商 API 控制台（「获取 API Key」跳转页）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console_url: Option<String>,
    /// 协议变体（≥2 时前端显示协议切换器）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocols: Option<Vec<ProviderProtocolData>>,
    /// 上次逐字段核对官方 API 文档的日期（YYYY-MM-DD）。
    ///
    /// 由核对流程写入（经 `upsert_provider_preset` 时取本地系统时间，
    /// 不信任模型凭记忆给出的日期）；字符串而非强类型日期——解析宽容，
    /// 单行格式异常不应导致整个预设文件被拒绝反序列化。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
    /// 本次核对依据的官方文档入口 URL（下次核对直接回访）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_source: Option<String>,
}

/// 云端嵌入服务预设（**厂商级**）。
///
/// 运行时仍复用统一的 OpenAI-compatible 嵌入适配器；插件贡献的是「选哪家厂商 →
/// 端点、可用模型、维度与请求能力」这层元数据，供设置表单在选择服务商后
/// 自动填充端点等字段，并让适配器按厂商能力拼请求体。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingProviderPresetData {
    /// 厂商稳定 id（与 LLM 预设 id 尽量一致，便于复用显示名与图标）
    pub id: String,
    /// 厂商展示名
    pub provider: String,
    /// OpenAI 兼容端点（base_url，不含 `/embeddings`）
    pub endpoint: String,
    /// 该厂商可用的嵌入模型（至少一条）
    pub models: Vec<EmbeddingModelPresetData>,
    /// 地域说明（如「中国大陆」「国际」）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// 控制台 / API Key 页面
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console_url: Option<String>,
    /// 请求体里下发维度的参数名（`dimensions` / `output_dimension`）。
    /// 缺省表示**不下发**：服务端维度固定，或兼容层明确不支持该参数
    /// （如 Cohere Compatibility API 把 `dimensions` 列为 unsupported）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimension_param: Option<String>,
    /// 是否需要真实 API Key；本地服务（LM Studio / vLLM / Ollama）填任意占位值即可
    #[serde(default = "default_embedding_needs_api_key")]
    pub needs_api_key: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_for: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_source: Option<String>,
}

/// 厂商下的一个嵌入模型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingModelPresetData {
    /// 模型 ID 原文（大小写、分隔符与官方一致）
    pub model: String,
    /// 默认输出维度
    pub dimension: usize,
    /// 单请求 `input` 数组条数上限。服务商差异极大（OpenAI 2048、智谱 64、百炼 10），
    /// 适配器按此值分块，缺省回退到内置兜底值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_batch: Option<usize>,
    /// 可切换的维度取值（仅用于表单提示，不参与请求）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<Vec<usize>>,
    /// 备注（推荐场景等）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn default_embedding_needs_api_key() -> bool {
    true
}

/// 按模型 ID 反查厂商预设（运行时能力查询用；同模型多处出现时取第一条）
pub fn find_embedding_preset_by_model(model: &str) -> Option<EmbeddingProviderPresetData> {
    let model = model.trim();
    load_embedding_provider_presets().into_iter().find(|p| {
        p.models.iter().any(|m| m.model.trim() == model)
    })
}

/// 插件清单条目（设置窗口「插件」页只读盘点用）
#[derive(Debug, Clone, Serialize)]
pub struct PluginInventoryEntry {
    /// 插件目录 key（运行时装卸载的稳定主键）
    pub key: String,
    /// 插件名（命名空间前缀，允许与目录 key 不同）
    pub name: String,
    pub version: String,
    pub description: String,
    /// 贡献的技能（命名空间名，如 `my-plugin/skill_a`）
    pub skills: Vec<String>,
    /// 贡献的可执行工具名（经 DynamicTool 注册）
    pub tools: Vec<String>,
    /// 贡献的 MCP server id
    pub mcp_servers: Vec<String>,
    /// 贡献的供应商预设 id（设置 → LLM 页厂商卡片数据）
    pub providers: Vec<String>,
    /// 贡献的云端嵌入服务预设 id
    pub embeddings: Vec<String>,
    /// "loaded"（正常装载）或 "skipped"（清单缺失/解析失败/命名非法）
    pub status: String,
    /// 信任状态：`trusted` / `changed`（曾信任但清单已变更）/ `untrusted`
    pub trust: String,
    /// 跳过原因（仅 status="skipped" 时非空）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 插件目录路径（便于用户定位/编辑）
    pub dir: String,
}

/// 插件装载结果
#[derive(Debug, Default, Clone, Serialize)]
pub struct PluginLoadReport {
    /// 成功装载的插件名
    pub plugins: Vec<String>,
    /// 注册的技能（命名空间名）
    pub skills: Vec<String>,
    /// 注册的工具名
    pub tools: Vec<String>,
    /// 合并的 MCP server id
    pub mcp_servers: Vec<String>,
    /// JS 插件贡献的动态协议 provider_type（声明式路由面）
    pub protocols: Vec<String>,
    /// 跳过/失败的清单（诊断用）
    pub skipped: Vec<String>,
}

// ============================================================================
// 运行时诊断（设置页「插件诊断」数据源）
//
// 两类信息：
// - **最近一次装载报告**：load_all / load_one 成功路径原子替换，设置页直接看
//   「装载了什么、跳过了什么」
// - **诊断事件**：目前只进日志的静默决策（MCP id 被占用跳过、工具重名跳过、
//   provider 预设冲突等）在此登记，有界 FIFO（最新在前），按内容去重——
//   查询类入口（如 provider 预设每次重开设置重读盘）不会刷屏
// ============================================================================

/// 最近一次插件装载报告
fn last_report_slot() -> &'static Mutex<Option<PluginLoadReport>> {
    static SLOT: OnceLock<Mutex<Option<PluginLoadReport>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// 记录最近一次装载报告（load_all / load_one 成功路径调用）
fn set_last_report(report: &PluginLoadReport) {
    *last_report_slot().lock().unwrap() = Some(report.clone());
}

/// 读取最近一次装载报告
pub fn last_report() -> Option<PluginLoadReport> {
    last_report_slot().lock().unwrap().clone()
}

/// 把启动后半段（[`load_all_tools`]）注册的工具名并入最近装载报告——
/// 工具注册晚于 load_all（须等内置工具先注册），不并入会让诊断页少计。
pub(crate) fn record_loaded_tools(names: Vec<String>) {
    if names.is_empty() {
        return;
    }
    let mut slot = last_report_slot().lock().unwrap();
    if let Some(rep) = slot.as_mut() {
        rep.tools.extend(names);
    }
}

/// 诊断事件容量（条）
const DIAG_CAP: usize = 100;

/// 诊断事件队列（最新在前）
fn diag_slot() -> &'static Mutex<Vec<String>> {
    static SLOT: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(Vec::new()))
}

/// 登记一条运行时诊断事件（去重：与已有条目逐字相同则忽略）
pub fn push_diag_event(entry: String) {
    let mut slot = diag_slot().lock().unwrap();
    if slot.iter().any(|e| e == &entry) {
        return;
    }
    slot.insert(0, entry);
    slot.truncate(DIAG_CAP);
}

/// 读取全部诊断事件（最新在前）
pub fn diag_events() -> Vec<String> {
    diag_slot().lock().unwrap().clone()
}

/// 插件根目录：`<用户数据目录>/plugins`
pub fn plugins_dir() -> PathBuf {
    crate::utils::path::get_user_data_dir().join("plugins")
}

// ============================================================================
// 插件信任状态（plugins/.trust.json）
//
// 插件的 MCP 声明会执行本地命令、工具声明会执行 PowerShell——「把文件放进
// 插件目录」不能等价于「授权执行」。信任以清单指纹锚定：首次装载前须在
// 设置窗口显式信任；信任后清单任何安全相关字段（name/skills/tools/mcp/
// providers/js/version）变更即回到未信任，需重新确认。
// ============================================================================

/// 单个受信插件的指纹记录
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrustedEntry {
    fingerprint: String,
    trusted_at: String,
}

/// 信任存储（持久化为 plugins/.trust.json）
#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustStore {
    trusted: HashMap<String, TrustedEntry>,
}

fn trust_store_path() -> PathBuf {
    // 测试构建可整体重定向信任存储（见 authorize_plugin_for_tests），
    // 单测绝不写真实用户目录的 .trust.json
    #[cfg(test)]
    if let Some(dir) = TRUST_STORE_DIR_OVERRIDE.lock().unwrap().clone() {
        return dir.join(".trust.json");
    }
    plugins_dir().join(".trust.json")
}

fn load_trust_store() -> TrustStore {
    std::fs::read_to_string(trust_store_path())
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

/// 原子写信任存储（临时文件 + rename，避免中断留下损坏文件）
fn save_trust_store(store: &TrustStore) -> Result<(), String> {
    let content = serde_json::to_string_pretty(store)
        .map_err(|e| format!("序列化信任存储失败: {e}"))?;
    crate::utils::fs::write_atomic(&trust_store_path(), &content)
        .map_err(|e| format!("写信任存储失败: {e}"))
}

/// 插件指纹：清单全量内容 + 全部声明资源文件内容的联合 SHA-256。
///
/// 清单只声明 glob 路径，真正的执行体在文件内容里（工具 PowerShell 脚本 /
/// JS 代码 / 协议与预设数据）——指纹必须锚定内容而非仅清单文本，否则信任
/// 后替换工具脚本即可绕过门禁。任一声明文件新增 / 删除 / 改写都产生不同
/// 指纹（保守方向的误报可接受：重新信任一次即可）。
///
/// 清单文本取 serde_json 序列化结果（按结构体字段序，与文件中的键顺序无关），
/// 重排 JSON 键不会误报；值变更会。协议 glob 不在类型化清单字段中
/// （protocol_registry 从原始 JSON 读取 `protocols` 键），单独取出参与哈希。
fn manifest_fingerprint(pdir: &Path, manifest: &PluginManifest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_string(manifest).unwrap_or_default().as_bytes());
    let protocols_glob = std::fs::read_to_string(pdir.join("plugin.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|v| v["protocols"].as_str().map(str::to_string))
        .unwrap_or_else(|| "protocols".to_string());
    let mut paths: Vec<PathBuf> = Vec::new();
    for pattern in &manifest.skills {
        paths.extend(glob_files(pdir, pattern));
    }
    for pattern in &manifest.tools {
        paths.extend(glob_json_files(pdir, pattern));
    }
    for pattern in &manifest.js {
        paths.extend(glob_js_files(pdir, pattern));
    }
    paths.extend(glob_json_files(pdir, &protocols_glob));
    if let Some(rel) = manifest.providers.as_deref() {
        if is_safe_manifest_path(rel) {
            let path = pdir.join(rel);
            if path.is_file() && is_safe_plugin_path(pdir, &path) {
                paths.push(path);
            }
        }
    }
    if let Some(rel) = manifest.embeddings.as_deref() {
        if is_safe_manifest_path(rel) {
            let path = pdir.join(rel);
            if path.is_file() && is_safe_plugin_path(pdir, &path) {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths.dedup();
    for path in paths {
        // 相对路径参与哈希（与插件目录所在盘符/用户名无关，指纹跨机器稳定）
        let rel = path
            .strip_prefix(pdir)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        hasher.update(rel.as_bytes());
        hasher.update([0]);
        if let Ok(content) = std::fs::read(&path) {
            hasher.update((content.len() as u64).to_le_bytes());
            hasher.update(&content);
        }
        hasher.update([1]);
    }
    format!("{:x}", hasher.finalize())
}

/// 指纹计算缓存（进程生命周期内，按目录 key 缓存；写路径调用 invalidate_fingerprint_cache）
fn fingerprint_cache() -> &'static Mutex<HashMap<String, (String, std::time::Instant)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (String, std::time::Instant)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 缓存有效期（同一插件在此时间窗口内不重算指纹）
const FINGERPRINT_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// 失效指纹缓存（写操作后调用：信任/重载/删除/创建）
pub fn invalidate_fingerprint_cache() {
    fingerprint_cache().lock().unwrap().clear();
}

/// 带缓存的指纹计算
fn cached_fingerprint(pdir: &Path, manifest: &PluginManifest) -> String {
    let key = pdir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let now = std::time::Instant::now();
    {
        let cache = fingerprint_cache().lock().unwrap();
        if let Some((fp, ts)) = cache.get(&key) {
            if now.duration_since(*ts) < FINGERPRINT_CACHE_TTL {
                return fp.clone();
            }
        }
    }
    let fp = manifest_fingerprint(pdir, manifest);
    fingerprint_cache().lock().unwrap().insert(key, (fp.clone(), now));
    fp
}

/// 插件信任状态：
/// - `trusted`：已信任且清单未变更
/// - `changed`：曾信任但清单已变更（须重新确认）
/// - `untrusted`：从未信任
pub fn trust_status_of(key: &str, fingerprint: &str) -> &'static str {
    if BUILTIN_PLUGIN_DIRS.contains(&key) {
        // 内置插件仍须验证 manifest.name 与预期一致（防伪造同名目录）
        let pdir = plugins_dir().join(key);
        if let Ok((name, _)) = read_manifest(&pdir) {
            let expected = if key == BUILTIN_PLUGIN_DIR { "llm-providers" } else { "plugin-authoring" };
            if name == expected {
                return "trusted";
            }
            tracing::warn!(
                "[Plugins] 内置目录 {key} 的 manifest.name={name} 与预期 {expected} 不符，视为未信任"
            );
        }
        // fallthrough to normal trust check
    }
    match load_trust_store().trusted.get(key) {
        Some(entry) if entry.fingerprint == fingerprint => "trusted",
        Some(_) => "changed",
        None => "untrusted",
    }
}

/// 记录信任（upsert 指纹）。调用方须保证这是用户显式授权的入口
/// （设置窗口信任按钮 / create_plugin 的用户预览卡片）。
fn record_trust(key: &str, fingerprint: &str) -> Result<(), String> {
    let mut store = load_trust_store();
    store.trusted.insert(
        key.to_string(),
        TrustedEntry {
            fingerprint: fingerprint.to_string(),
            trusted_at: chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        },
    );
    save_trust_store(&store)
}

// ---- 测试信任隔离 ----
//
// 单测需要过信任门禁（js_host 装载测试），但绝不能写真实用户目录的
// .trust.json——信任存储路径可整体重定向到测试临时目录，全部测试共享
// 一个目录（OnceLock），授权写入用互斥锁串行化（避免并发 read-modify-write
// 丢条目）。
#[cfg(test)]
static TRUST_STORE_DIR_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

#[cfg(test)]
static TEST_TRUST_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 测试专用：授权一个插件目录（计算当前指纹并写入测试信任存储）。
///
/// 首次调用时创建共享测试信任目录并接管 trust_store_path；后续调用只
/// upsert 该插件的指纹。串行化保证并发测试不丢授权条目。
#[cfg(test)]
pub(crate) fn authorize_plugin_for_tests(pdir: &Path) {
    static TRUST_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = TRUST_WRITE_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let dir = TEST_TRUST_DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("vivian-trust-test-{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    });
    *TRUST_STORE_DIR_OVERRIDE.lock().unwrap() = Some(dir.clone());
    let key = pdir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let (_, manifest) = read_manifest(pdir).expect("测试插件清单应可解析");
    record_trust(&key, &manifest_fingerprint(pdir, &manifest)).expect("测试信任写入应成功");
}

/// 判断一个插件目录当前是否受信（js_host 装载门禁用）。
pub(crate) fn is_plugin_dir_trusted(pdir: &Path) -> bool {
    let key = pdir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    match read_manifest(pdir) {
        Ok((_, manifest)) => trust_status_of(key, &cached_fingerprint(pdir, &manifest)) == "trusted",
        Err(_) => false,
    }
}

/// 判断插件名（manifest.name 或目录 key）当前是否有受信插件在册。
///
/// MCP init_all 的连接门禁：持久化配置里带 `source_plugin` 的 server 只在
/// 其来源插件受信时才自动连接；来源插件缺失/未信任则跳过。
pub fn is_plugin_name_trusted(name: &str) -> bool {
    let dir = plugins_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let pdir = entry.path();
        if !pdir.is_dir() || !pdir.join("plugin.json").exists() {
            continue;
        }
        let key = pdir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if key == name {
            return is_plugin_dir_trusted(&pdir);
        }
        if let Ok((manifest_name, manifest)) = read_manifest(&pdir) {
            if manifest_name == name {
                return trust_status_of(key, &cached_fingerprint(&pdir, &manifest)) == "trusted";
            }
        }
    }
    false
}

// ============================================================================
// 插件贡献快照
//
// 卸载/更新必须按「装载时注册了什么」撤销，而不是按磁盘当前内容猜——
// 插件更新会替换磁盘文件，旧版本声明的技能/工具/MCP 只存在于内存快照中。
// ============================================================================

/// 单个插件已注册贡献的内存快照（目录 key → 快照）
#[derive(Debug, Default, Clone)]
struct PluginContribSnapshot {
    /// 已注册技能的完整命名空间名（如 `my-plugin/skill_a`）
    skills: Vec<String>,
    /// 已注册工具名
    tools: Vec<String>,
    /// 已注册 MCP server id
    mcp_ids: Vec<String>,
    /// JS 运行时是否已装载（JS 贡献由 js_host 自管，这里只记归属）
    js: bool,
}

fn contrib_snapshots() -> &'static Mutex<HashMap<String, PluginContribSnapshot>> {
    static SNAPSHOTS: OnceLock<Mutex<HashMap<String, PluginContribSnapshot>>> = OnceLock::new();
    SNAPSHOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn read_snapshot(key: &str) -> Option<PluginContribSnapshot> {
    contrib_snapshots().lock().unwrap().get(key).cloned()
}

/// 整体替换快照（装载成功后调用）
fn store_snapshot(key: &str, snapshot: PluginContribSnapshot) {
    contrib_snapshots().lock().unwrap().insert(key.to_string(), snapshot);
}

/// 移除快照（卸载/删除后调用）
fn drop_snapshot(key: &str) {
    contrib_snapshots().lock().unwrap().remove(key);
}

/// 判断 manifest 声明的路径是否为安全的相对路径。
///
/// 插件资源必须留在自己的目录内；同时检查 `/` 与 Windows `\\`，避免
/// 通过另一种分隔符绕过 `..` 检查。通配符只用于文件匹配，不改变目录边界。
pub(crate) fn is_safe_manifest_path(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || Path::new(value).is_absolute() {
        return false;
    }
    let normalized = value.replace('\\', "/");
    !normalized.split('/').any(|part| part == "..")
}

/// 校验路径解析后的真实目标仍位于插件目录内。
///
/// `canonicalize` 同时覆盖符号链接与 Windows junction；不存在的路径直接
/// 视为不安全，因为插件资源在展开前必须已经存在。
pub(crate) fn is_safe_plugin_path(pdir: &Path, path: &Path) -> bool {
    let Ok(base) = std::fs::canonicalize(pdir) else {
        return false;
    };
    let Ok(candidate) = std::fs::canonicalize(path) else {
        return false;
    };
    candidate.starts_with(base)
}

/// 读取并校验单个插件的清单；失败返回 (插件名, 原因)。
///
/// 校验内容：manifest.json 存在且可解析、插件名非空、仅含安全字符（字母/数字/-/_）。
fn read_manifest(pdir: &Path) -> Result<(String, PluginManifest), (String, String)> {
    let dir_name = pdir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("plugin")
        .to_string();
    let manifest_path = pdir.join("plugin.json");
    let content = std::fs::read_to_string(&manifest_path)
        .map_err(|e| (dir_name.clone(), format!("清单读取失败 {e}")))?;
    let mut manifest: PluginManifest =
        serde_json::from_str(&content).map_err(|e| (dir_name.clone(), format!("清单解析失败 {e}")))?;
    if manifest.name.trim().is_empty() {
        manifest.name = dir_name;
    }
    // 插件名只允许安全字符（避免污染技能命名空间与日志注入）
    if !manifest
        .name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err((
            manifest.name.clone(),
            "插件名含非法字符（仅允许字母/数字/-/_）".into(),
        ));
    }
    Ok((manifest.name.clone(), manifest))
}

/// 展开插件声明的技能 glob，返回 `(命名空间名, 描述, 关键词, 正文)` 四元组。
fn expanded_skills(
    pdir: &Path,
    plugin_name: &str,
    patterns: &[String],
) -> Vec<(String, String, Vec<String>, String)> {
    let mut out = Vec::new();
    for pattern in patterns {
        for skill_path in glob_files(pdir, pattern) {
            let Ok(text) = std::fs::read_to_string(&skill_path) else {
                continue;
            };
            let (skill_name, description, keywords, body) =
                crate::skills::parse_skill_file(&text, &skill_path);
            if body.trim().is_empty() {
                continue;
            }
            out.push((
                format!("{plugin_name}/{skill_name}"),
                description,
                keywords,
                body,
            ));
        }
    }
    out
}

/// 展开插件声明的工具 glob，返回校验通过的 `CustomToolDef` 列表。
///
/// 校验（与自建工具装载同规则）：名字合法、脚本过黑名单、schema 为 object。
/// 不合格的单个定义跳过（不影响其余）；AST 审计与防影子化由注册方
/// 在装载时批量完成（[`load_all_tools`] / [`load_one`]）。
fn expanded_tools(pdir: &Path, patterns: &[String]) -> Vec<CustomToolDef> {
    let mut out = Vec::new();
    for pattern in patterns {
        for path in glob_json_files(pdir, pattern) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(def) = serde_json::from_str::<CustomToolDef>(&text) else {
                tracing::warn!("[Plugins] 插件工具定义损坏，跳过: {}", path.display());
                continue;
            };
            if !crate::tools::custom_tools::is_valid_tool_name(&def.name) {
                tracing::warn!("[Plugins] 插件工具名不合法，跳过: {}", def.name);
                continue;
            }
            if crate::tools::custom_tools::forbidden_fragment_in(&def.script).is_some() {
                tracing::warn!("[Plugins] 插件工具脚本含破坏性片段，跳过: {}", def.name);
                continue;
            }
            match crate::tools::custom_tools::sanitize_parameters(&def.parameters) {
                Some(params) => out.push(CustomToolDef { parameters: params, ..def }),
                None => {
                    tracing::warn!("[Plugins] 插件工具参数 schema 非 object，跳过: {}", def.name)
                }
            }
        }
    }
    out
}

/// 简化 glob：展开插件目录下 `*.json` 一层或递归两种模式（工具定义文件）。
fn glob_json_files(base: &Path, pattern: &str) -> Vec<PathBuf> {
    if !is_safe_manifest_path(pattern) {
        tracing::warn!("[Plugins] 拒绝越界工具路径声明: {}", pattern);
        return Vec::new();
    }
    let pattern = pattern.trim().trim_start_matches("./");
    let (dir_part, suffix_glob) = match pattern.rsplit_once('/') {
        Some((d, s)) => (d.to_string(), s.to_string()),
        None => (String::new(), pattern.to_string()),
    };
    let recursive = suffix_glob == "**" || suffix_glob.is_empty();
    if suffix_glob != "*.json" && !recursive {
        return Vec::new();
    }
    let root = if dir_part.is_empty() {
        base.to_path_buf()
    } else {
        base.join(dir_part)
    };
    if !root.is_dir() || !is_safe_plugin_path(base, &root) {
        return Vec::new();
    }
    fn collect(base: &Path, dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if recursive {
                    if is_safe_plugin_path(base, &path) {
                        collect(base, &path, recursive, out);
                    }
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("json")
                && is_safe_plugin_path(base, &path)
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    collect(base, &root, recursive, &mut out);
    out.sort();
    out
}

/// 简化 glob：展开插件 JS 入口（`js/*.js` 一层或 `js` / `js/**` 整目录递归）。
fn glob_js_files(base: &Path, pattern: &str) -> Vec<PathBuf> {
    if !is_safe_manifest_path(pattern) {
        tracing::warn!("[Plugins] 拒绝越界 JS 路径声明: {}", pattern);
        return Vec::new();
    }
    let pattern = pattern.trim().trim_start_matches("./");
    let (dir_part, suffix_glob) = match pattern.rsplit_once('/') {
        Some((d, s)) => (d.to_string(), s.to_string()),
        None => (String::new(), pattern.to_string()),
    };
    let recursive = suffix_glob == "**" || suffix_glob.is_empty();
    if suffix_glob != "*.js" && !recursive {
        return Vec::new();
    }
    let root = if dir_part.is_empty() {
        base.to_path_buf()
    } else {
        base.join(dir_part)
    };
    if !root.is_dir() || !is_safe_plugin_path(base, &root) {
        return Vec::new();
    }
    fn collect(base: &Path, dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if recursive {
                    if is_safe_plugin_path(base, &path) {
                        collect(base, &path, recursive, out);
                    }
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("js")
                && is_safe_plugin_path(base, &path)
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    collect(base, &root, recursive, &mut out);
    out.sort();
    out
}

/// 读取插件的 JS 入口（manifest `js` glob 展开为文件列表）。
///
/// 返回 `(插件名, JS 文件列表)`；None 表示插件未声明 JS 或入口为空
/// （非 JS 插件的正常路径）。供 js_host 创建 JS 运行时。
pub fn js_plugin_entry(pdir: &Path) -> Option<(String, Vec<PathBuf>)> {
    let (name, manifest) = read_manifest(pdir).ok()?;
    if manifest.js.is_empty() {
        return None;
    }
    let mut files = Vec::new();
    for pattern in &manifest.js {
        files.extend(glob_js_files(pdir, pattern));
    }
    if files.is_empty() {
        None
    } else {
        Some((name, files))
    }
}

/// 读取单个插件声明的供应商预设文件（解析失败仅告警，不影响其他贡献点）
fn read_provider_presets(pdir: &Path, manifest: &PluginManifest) -> Vec<ProviderPresetData> {
    let Some(rel) = manifest.providers.as_deref() else {
        return Vec::new();
    };
    if !is_safe_manifest_path(rel) {
        tracing::warn!("[Plugins] 拒绝越界供应商预设路径声明: {}", rel);
        return Vec::new();
    }
    let path = pdir.join(rel);
    if !is_safe_plugin_path(pdir, &path) {
        tracing::warn!("[Plugins] 拒绝插件目录外供应商预设路径: {}", path.display());
        return Vec::new();
    }
    let Ok(content) = std::fs::read_to_string(&path) else {
        tracing::warn!(
            "[Plugins] 供应商预设文件读取失败: {}",
            path.display()
        );
        return Vec::new();
    };
    match serde_json::from_str::<Vec<ProviderPresetData>>(&content) {
        Ok(rows) => rows
            .into_iter()
            .filter(|p| {
                // id 非空且仅含安全字符，避免污染前端合并与 provider_cache 索引
                !p.id.trim().is_empty()
                    && p.id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            })
            .collect(),
        Err(e) => {
            tracing::warn!(
                "[Plugins] 供应商预设文件解析失败 {}: {e}",
                path.display()
            );
            Vec::new()
        }
    }
}

/// 全部插件贡献的供应商预设（按需读取，每次重新读盘——编辑 providers.json 后
/// 重开设置窗口即生效，无需重启）。
///
/// 合并语义（**先到保留**，杜绝静默覆盖）：
/// - 内置 llm-providers 声明的 id 受保护：其他插件同 id 声明一律跳过（警告）
/// - 插件间同 id 冲突：插件目录按名字典序先到保留，后者跳过（警告）——
///   生效者只取决于稳定的目录排序，不取决于谁覆盖谁
pub fn load_provider_presets() -> Vec<ProviderPresetData> {
    static BUILTIN_IDS: OnceLock<std::collections::HashSet<String>> = OnceLock::new();
    let builtin_ids = BUILTIN_IDS.get_or_init(|| {
        serde_json::from_str::<Vec<ProviderPresetData>>(BUILTIN_PLUGIN_PROVIDERS)
            .map(|rows| rows.into_iter().map(|p| p.id).collect())
            .unwrap_or_default()
    });

    let dir = plugins_dir();
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut plugin_dirs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir() && p.join("plugin.json").exists())
                .collect()
        })
        .unwrap_or_default();
    plugin_dirs.sort();

    let mut by_id: Vec<(String, String, ProviderPresetData)> = Vec::new();
    for pdir in plugin_dirs {
        let dir_key = pdir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let Ok((name, manifest)) = read_manifest(&pdir) else {
            continue;
        };
        // 信任门禁：未信任插件的供应商预设不合并（内置插件豁免）
        if !BUILTIN_PLUGIN_DIRS.contains(&dir_key.as_str())
            && trust_status_of(&dir_key, &cached_fingerprint(&pdir, &manifest)) != "trusted"
        {
            continue;
        }
        for preset in read_provider_presets(&pdir, &manifest) {
            // 内置预设 id 受保护：内置声明恒优先，其他插件不得覆盖
            if builtin_ids.contains(&preset.id) && dir_key != BUILTIN_PLUGIN_DIR {
                tracing::warn!(
                    "[Plugins] 插件 {name} 的供应商预设 {} 与内置预设 id 冲突，已跳过（内置不可覆盖）",
                    preset.id
                );
                push_diag_event(format!(
                    "插件 {name} 供应商预设 {} 与内置预设 id 冲突，已跳过（内置不可覆盖）",
                    preset.id
                ));
                continue;
            }
            match by_id.iter_mut().find(|(id, _, _)| *id == preset.id) {
                None => by_id.push((preset.id.clone(), name.clone(), preset)),
                Some((id, owner, existing)) => {
                    // 内置插件声明迟到时夺回（如排序恰好靠后）；插件间先到保留
                    if dir_key == BUILTIN_PLUGIN_DIR {
                        tracing::warn!(
                            "[Plugins] 内置插件收回供应商预设 {id}（此前被插件 {owner} 声明）"
                        );
                        push_diag_event(format!(
                            "内置插件收回供应商预设 {id}（此前被插件 {owner} 声明）"
                        ));
                        *owner = name.clone();
                        *existing = preset;
                    } else {
                        tracing::warn!(
                            "[Plugins] 供应商预设 {} 冲突：插件 {name} 的声明被跳过（已由插件 {owner} 声明）",
                            id
                        );
                        push_diag_event(format!(
                            "供应商预设 {id} 冲突：插件 {name} 的声明被跳过（已由插件 {owner} 声明）"
                        ));
                    }
                }
            }
        }
    }
    by_id.into_iter().map(|(_, _, p)| p).collect()
}

fn read_embedding_provider_presets(
    pdir: &Path,
    manifest: &PluginManifest,
) -> Vec<EmbeddingProviderPresetData> {
    let Some(rel) = manifest.embeddings.as_deref() else {
        return Vec::new();
    };
    if !is_safe_manifest_path(rel) {
        tracing::warn!("[Plugins] 拒绝越界嵌入预设路径声明: {rel}");
        return Vec::new();
    }
    let path = pdir.join(rel);
    if !is_safe_plugin_path(pdir, &path) {
        tracing::warn!("[Plugins] 拒绝插件目录外嵌入预设路径: {}", path.display());
        return Vec::new();
    }
    let Ok(content) = std::fs::read_to_string(&path) else {
        tracing::warn!("[Plugins] 嵌入预设文件读取失败: {}", path.display());
        return Vec::new();
    };
    match serde_json::from_str::<Vec<EmbeddingProviderPresetData>>(&content) {
        Ok(rows) => rows.into_iter().filter(is_valid_embedding_preset).collect(),
        Err(e) => {
            tracing::warn!("[Plugins] 嵌入预设文件解析失败 {}: {e}", path.display());
            Vec::new()
        }
    }
}

/// 单条厂商级嵌入预设是否可用（解析后过滤脏数据，非法行整条丢弃）
fn is_valid_embedding_preset(p: &EmbeddingProviderPresetData) -> bool {
    !p.id.trim().is_empty()
        && p.id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !p.provider.trim().is_empty()
        && !p.endpoint.trim().is_empty()
        && !p.models.is_empty()
        && p.models
            .iter()
            .all(|m| !m.model.trim().is_empty() && m.dimension > 0)
}

/// 加载所有受信任插件贡献的云端嵌入预设；同 id 先到保留，内置 id 不可覆盖。
pub fn load_embedding_provider_presets() -> Vec<EmbeddingProviderPresetData> {
    let builtin_ids: std::collections::HashSet<String> =
        serde_json::from_str::<Vec<EmbeddingProviderPresetData>>(BUILTIN_PLUGIN_EMBEDDINGS)
            .map(|rows| rows.into_iter().map(|p| p.id).collect())
            .unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(plugins_dir())
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir() && p.join("plugin.json").exists())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    let mut rows: Vec<(String, String, EmbeddingProviderPresetData)> = Vec::new();
    for pdir in dirs {
        let key = pdir.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        let Ok((name, manifest)) = read_manifest(&pdir) else { continue };
        if !BUILTIN_PLUGIN_DIRS.contains(&key)
            && trust_status_of(key, &cached_fingerprint(&pdir, &manifest)) != "trusted"
        {
            continue;
        }
        for preset in read_embedding_provider_presets(&pdir, &manifest) {
            if builtin_ids.contains(&preset.id) && key != BUILTIN_PLUGIN_DIR {
                tracing::warn!("[Plugins] 插件 {name} 的嵌入预设 {} 与内置 id 冲突，已跳过", preset.id);
                continue;
            }
            match rows.iter_mut().find(|(id, _, _)| *id == preset.id) {
                None => rows.push((preset.id.clone(), name.clone(), preset)),
                Some((_, owner, existing)) if key == BUILTIN_PLUGIN_DIR => {
                    *owner = name.clone();
                    *existing = preset;
                }
                Some(_) => tracing::warn!("[Plugins] 嵌入预设 {} 冲突，插件 {name} 的声明已跳过", preset.id),
            }
        }
    }
    rows.into_iter().map(|(_, _, p)| p).collect()
}

/// 内置插件 llm-providers：供应商预设数据 + 预设核对技能。
/// 编译期嵌入（src-tauri/plugins/llm-providers/），启动时播种到用户插件目录。
const BUILTIN_PLUGIN_DIR: &str = "llm-providers";
const BUILTIN_PLUGIN_MANIFEST: &str = include_str!("../plugins/llm-providers/plugin.json");
const BUILTIN_PLUGIN_PROVIDERS: &str = include_str!("../plugins/llm-providers/providers.json");
const BUILTIN_PLUGIN_EMBEDDINGS: &str =
    include_str!("../plugins/llm-providers/embedding-providers.json");
const BUILTIN_PLUGIN_SKILL_VERIFY: &str =
    include_str!("../plugins/llm-providers/skills/verify-provider-presets.md");

/// 内置插件 plugin-authoring：插件创作技能（教 create_plugin 的格式约定）。
/// 与 llm-providers 同机制播种；它是"插件创造插件"这一自引用模式的文档面。
const AUTHORING_PLUGIN_MANIFEST: &str = include_str!("../plugins/plugin-authoring/plugin.json");
const AUTHORING_PLUGIN_SKILL: &str =
    include_str!("../plugins/plugin-authoring/skills/plugin-authoring.md");

/// 解析 "major.minor.patch" 版本号（解析失败返回 None，视为不可比较）
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.trim().split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().unwrap_or("0").parse().unwrap_or(0);
    let patch = it.next().unwrap_or("0").parse().unwrap_or(0);
    Some((major, minor, patch))
}

/// 播种单个内置插件的通用实现：不存在或磁盘版本落后时写入完整文件集。
///
/// 版本比较以 manifest 内嵌 version 为准；磁盘清单解析失败（被改坏）也重播。
fn seed_builtin_plugin(dir_name: &str, manifest: &str, files: &[(PathBuf, &str)]) {
    let dir = plugins_dir().join(dir_name);
    let manifest_path = dir.join("plugin.json");
    let builtin_ver = serde_json::from_str::<serde_json::Value>(manifest)
        .ok()
        .and_then(|m| m["version"].as_str().map(str::to_string))
        .and_then(|v| parse_version(&v))
        .unwrap_or((0, 0, 0));
    let need_seed = if !manifest_path.exists() {
        true
    } else {
        let disk_version = std::fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
            .and_then(|m| m["version"].as_str().map(str::to_string))
            .and_then(|v| parse_version(&v));
        match disk_version {
            Some(v) => v < builtin_ver,
            None => true,
        }
    };
    if !need_seed {
        return;
    }
    let _ = std::fs::create_dir_all(dir.join("skills"));
    let mut writes = vec![(manifest_path, manifest)];
    writes.extend(files.iter().cloned());
    for (path, content) in writes {
        if let Err(e) = std::fs::write(&path, content) {
            tracing::warn!(
                "[Plugins] 内置插件 {dir_name} 播种失败 {}: {e}",
                path.display()
            );
            return;
        }
    }
    tracing::info!(
        "[Plugins] 内置插件 {dir_name} 已播种/升级到 v{}.{}.{}",
        builtin_ver.0,
        builtin_ver.1,
        builtin_ver.2
    );
}

/// 确保内置插件存在于用户插件目录：
/// - 不存在 → 完整播种（技能随下次 load_all 注册，预设按需读取）
/// - 磁盘版本 < 内置版本 → 覆盖升级（手工定制者保持自身 version ≥ 内置版本即可保留修改）
/// - 其余情况不动（尊重用户对已播种文件的编辑）
pub fn ensure_builtin_plugins() {
    seed_builtin_plugin(
        BUILTIN_PLUGIN_DIR,
        BUILTIN_PLUGIN_MANIFEST,
        &[
            (plugins_dir().join(BUILTIN_PLUGIN_DIR).join("providers.json"), BUILTIN_PLUGIN_PROVIDERS),
            (
                plugins_dir().join(BUILTIN_PLUGIN_DIR).join("embedding-providers.json"),
                BUILTIN_PLUGIN_EMBEDDINGS,
            ),
            (
                plugins_dir()
                    .join(BUILTIN_PLUGIN_DIR)
                    .join("skills")
                    .join("verify-provider-presets.md"),
                BUILTIN_PLUGIN_SKILL_VERIFY,
            ),
        ],
    );
    let authoring_dir = plugins_dir().join("plugin-authoring");
    seed_builtin_plugin(
        "plugin-authoring",
        AUTHORING_PLUGIN_MANIFEST,
        &[(
            authoring_dir.join("skills").join("plugin-authoring.md"),
            AUTHORING_PLUGIN_SKILL,
        )],
    );
}

/// 运行时 upsert 一条供应商预设到内置 llm-providers 插件（用户目录数据文件）。
///
/// 语义（`update_provider_preset` 工具的核心实现）：
/// - 按 id **整行替换**；id 不存在则追加（新增供应商预设走此路径）
/// - `verified_at` 由本地系统时间写入——不信任调用方给出的日期，
///   模型对"今天几号"的先验不可靠
/// - 同步递增 plugin.json 的 version（patch +1，保证严格大于磁盘当前值），
///   保住修改不被下次启动播种覆盖（播种仅在磁盘版本 < 内置版本时发生）
/// - 插件文件不存在时先播种内置数据再修改（首次运行即可用）
///
/// 返回 `(更新后的行, 新 version, 是否为新增)`，供调用方向用户汇报核对结果。
pub fn upsert_provider_preset(
    preset: ProviderPresetData,
) -> Result<(ProviderPresetData, String, bool), String> {
    // 确保插件文件存在（不存在则播种、版本过旧则升级——之后统一在其上修改）
    ensure_builtin_plugins();
    upsert_provider_preset_at(&plugins_dir().join(BUILTIN_PLUGIN_DIR), preset)
}

/// 更新或新增内置 llm-providers 插件中的云端嵌入预设。
pub fn upsert_embedding_provider_preset(
    mut preset: EmbeddingProviderPresetData,
) -> Result<(EmbeddingProviderPresetData, String, bool), String> {
    ensure_builtin_plugins();
    validate_embedding_preset(&preset)?;
    let dir = plugins_dir().join(BUILTIN_PLUGIN_DIR);
    let path = dir.join("embedding-providers.json");
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取 embedding-providers.json 失败: {e}"))?;
    let mut rows: Vec<EmbeddingProviderPresetData> = serde_json::from_str(&content)
        .map_err(|e| format!("解析 embedding-providers.json 失败: {e}"))?;
    preset.verified_at = Some(chrono::Local::now().format("%Y-%m-%d").to_string());
    let is_new = match rows.iter().position(|p| p.id == preset.id) {
        Some(i) => {
            rows[i] = preset.clone();
            false
        }
        None => {
            rows.push(preset.clone());
            true
        }
    };
    let body = serde_json::to_string_pretty(&rows)
        .map_err(|e| format!("序列化 embedding-providers.json 失败: {e}"))?;
    crate::utils::fs::write_atomic(&path, &body)
        .map_err(|e| format!("写入 embedding-providers.json 失败: {e}"))?;
    let version = bump_plugin_version(&dir.join("plugin.json"))?;
    invalidate_fingerprint_cache();
    Ok((preset, version, is_new))
}

fn validate_embedding_preset(p: &EmbeddingProviderPresetData) -> Result<(), String> {
    if p.id.trim().is_empty()
        || !p.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("嵌入预设 id 非法（仅允许字母/数字/-/_）".into());
    }
    if p.provider.trim().is_empty() || p.endpoint.trim().is_empty() {
        return Err("嵌入预设 provider、endpoint 均不能为空".into());
    }
    if p.models.is_empty() {
        return Err("嵌入预设至少需要一个模型".into());
    }
    for m in &p.models {
        if m.model.trim().is_empty() {
            return Err("嵌入预设的模型名不能为空".into());
        }
        if m.dimension == 0 {
            return Err(format!("嵌入模型 {} 的 dimension 必须大于 0", m.model));
        }
    }
    if let Some(param) = p.dimension_param.as_deref() {
        // 只允许已知的两种参数名，避免把任意字段名塞进请求体
        if param != "dimensions" && param != "output_dimension" {
            return Err("dimensionParam 仅支持 dimensions 或 output_dimension".into());
        }
    }
    Ok(())
}

/// 新增/更新「某厂商的某个嵌入模型」：厂商行不存在则创建，存在则只动这一个模型。
///
/// 与 [`upsert_embedding_provider_preset`] 的整行替换语义不同：这里做合并，
/// 让 `manage_provider_preset` 工具可以在不改动同厂商其他模型的前提下增删改单条模型。
pub fn upsert_embedding_model_preset(
    mut preset: EmbeddingProviderPresetData,
) -> Result<(EmbeddingProviderPresetData, String, bool), String> {
    ensure_builtin_plugins();
    validate_embedding_preset(&preset)?;
    let dir = plugins_dir().join(BUILTIN_PLUGIN_DIR);
    let path = dir.join("embedding-providers.json");
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取 embedding-providers.json 失败: {e}"))?;
    let mut rows: Vec<EmbeddingProviderPresetData> = serde_json::from_str(&content)
        .map_err(|e| format!("解析 embedding-providers.json 失败: {e}"))?;
    preset.verified_at = Some(chrono::Local::now().format("%Y-%m-%d").to_string());
    let (merged, is_new) = match rows.iter().position(|p| p.id == preset.id) {
        Some(i) => {
            let existing = &mut rows[i];
            existing.provider = preset.provider.clone();
            existing.endpoint = preset.endpoint.clone();
            existing.dimension_param = preset.dimension_param.clone();
            existing.needs_api_key = preset.needs_api_key;
            existing.region = preset.region.clone();
            existing.console_url = preset.console_url.clone();
            existing.recommended_for = preset.recommended_for.clone();
            existing.verified_at = preset.verified_at.clone();
            existing.verified_source = preset.verified_source.clone();
            for incoming in &preset.models {
                match existing
                    .models
                    .iter()
                    .position(|m| m.model == incoming.model)
                {
                    Some(j) => existing.models[j] = incoming.clone(),
                    None => existing.models.push(incoming.clone()),
                }
            }
            (existing.clone(), false)
        }
        None => {
            rows.push(preset.clone());
            (preset.clone(), true)
        }
    };
    let body = serde_json::to_string_pretty(&rows)
        .map_err(|e| format!("序列化 embedding-providers.json 失败: {e}"))?;
    crate::utils::fs::write_atomic(&path, &body)
        .map_err(|e| format!("写入 embedding-providers.json 失败: {e}"))?;
    let version = bump_plugin_version(&dir.join("plugin.json"))?;
    invalidate_fingerprint_cache();
    Ok((merged, version, is_new))
}

/// 从内置供应商插件删除一条 LLM 或嵌入预设；删除的是预设元数据，不触碰凭据。
pub fn remove_provider_preset(kind: &str, id: &str) -> Result<(String, String), String> {
    ensure_builtin_plugins();
    let id = id.trim();
    if id.is_empty() {
        return Err("id 不能为空".into());
    }
    let dir = plugins_dir().join(BUILTIN_PLUGIN_DIR);
    let removed = match kind {
        "llm" => {
            let path = dir.join("providers.json");
            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("读取 providers.json 失败: {e}"))?;
            let mut rows: Vec<ProviderPresetData> = serde_json::from_str(&content)
                .map_err(|e| format!("解析 providers.json 失败: {e}"))?;
            let before = rows.len();
            rows.retain(|p| p.id != id);
            if rows.len() == before { false } else {
                let body = serde_json::to_string_pretty(&rows)
                    .map_err(|e| format!("序列化 providers.json 失败: {e}"))?;
                crate::utils::fs::write_atomic(&path, &body)
                    .map_err(|e| format!("写入 providers.json 失败: {e}"))?;
                true
            }
        }
        "embedding" => {
            let path = dir.join("embedding-providers.json");
            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("读取 embedding-providers.json 失败: {e}"))?;
            let mut rows: Vec<EmbeddingProviderPresetData> = serde_json::from_str(&content)
                .map_err(|e| format!("解析 embedding-providers.json 失败: {e}"))?;
            let before = rows.len();
            rows.retain(|p| p.id != id);
            if rows.len() == before { false } else {
                let body = serde_json::to_string_pretty(&rows)
                    .map_err(|e| format!("序列化 embedding-providers.json 失败: {e}"))?;
                crate::utils::fs::write_atomic(&path, &body)
                    .map_err(|e| format!("写入 embedding-providers.json 失败: {e}"))?;
                true
            }
        }
        _ => return Err("kind 仅支持 llm 或 embedding".into()),
    };
    if !removed {
        return Err(format!("未找到 {kind} 预设: {id}"));
    }
    let version = bump_plugin_version(&dir.join("plugin.json"))?;
    invalidate_fingerprint_cache();
    Ok((id.to_string(), version))
}

/// `upsert_provider_preset` 的目录参数化实现（测试用临时目录隔离）。
fn upsert_provider_preset_at(
    dir: &Path,
    mut preset: ProviderPresetData,
) -> Result<(ProviderPresetData, String, bool), String> {
    // 入口校验：对齐 read_provider_presets 对磁盘数据的过滤规则
    if preset.id.trim().is_empty()
        || !preset
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("预设 id 非法（仅允许字母/数字/-/_）: {}", preset.id));
    }
    if preset.provider_type.trim().is_empty() {
        return Err(format!("预设 {} 缺 provider_type", preset.id));
    }

    // 元数据：核对日期取本地系统时间（覆盖调用方传入值）
    preset.verified_at = Some(chrono::Local::now().format("%Y-%m-%d").to_string());

    // 读现有预设（文件缺失/解析失败直接报错——入口已保证播种，此处失败说明环境异常）
    let providers_path = dir.join("providers.json");
    let content = std::fs::read_to_string(&providers_path)
        .map_err(|e| format!("读取 {} 失败: {e}", providers_path.display()))?;
    let mut rows: Vec<ProviderPresetData> = serde_json::from_str(&content)
        .map_err(|e| format!("解析 providers.json 失败: {e}"))?;

    // 按 id 整行替换或追加
    let is_new = !rows.iter().any(|p| p.id == preset.id);
    match rows.iter_mut().find(|p| p.id == preset.id) {
        Some(slot) => *slot = preset.clone(),
        None => rows.push(preset.clone()),
    }

    // 写回（美化格式，与播种数据风格一致）
    let new_content = serde_json::to_string_pretty(&rows)
        .map_err(|e| format!("序列化 providers.json 失败: {e}"))?;
    std::fs::write(&providers_path, new_content)
        .map_err(|e| format!("写 {} 失败: {e}", providers_path.display()))?;

    // 递增清单版本（磁盘版本高于内置版本 → 播种不会覆盖本次修改）
    let new_version = bump_plugin_version(&dir.join("plugin.json"))?;

    Ok((preset, new_version, is_new))
}

/// 递增插件清单版本号（patch +1，保持 major/minor 不变）。
///
/// 磁盘 version 解析失败（被改坏）按 0.0.0 处理，bump 到 0.0.1。
fn bump_plugin_version(manifest_path: &Path) -> Result<String, String> {
    let content = std::fs::read_to_string(manifest_path)
        .map_err(|e| format!("读取 plugin.json 失败: {e}"))?;
    let mut manifest: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("解析 plugin.json 失败: {e}"))?;
    let old = manifest["version"]
        .as_str()
        .and_then(parse_version)
        .unwrap_or((0, 0, 0));
    let new_version = format!("{}.{}.{}", old.0, old.1, old.2 + 1);
    manifest["version"] = serde_json::Value::String(new_version.clone());
    let new_content = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("序列化 plugin.json 失败: {e}"))?;
    std::fs::write(manifest_path, new_content)
        .map_err(|e| format!("写 plugin.json 失败: {e}"))?;
    Ok(new_version)
}

/// 只读盘点全部插件（设置窗口「插件」页展示用，不装载、无副作用）。
pub fn scan_inventory() -> Vec<PluginInventoryEntry> {
    let dir = plugins_dir();
    let mut out = Vec::new();
    if !dir.exists() {
        return out;
    }
    let mut plugin_dirs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    plugin_dirs.sort();
    for pdir in plugin_dirs {
        if !pdir.join("plugin.json").exists() {
            continue;
        }
        let dir_str = pdir.display().to_string();
        match read_manifest(&pdir) {
            Ok((name, manifest)) => {
                let fingerprint = cached_fingerprint(&pdir, &manifest);
                let key = pdir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                out.push(PluginInventoryEntry {
                    skills: expanded_skills(&pdir, &name, &manifest.skills)
                        .into_iter()
                        .map(|(n, _, _, _)| n)
                        .collect(),
                    tools: expanded_tools(&pdir, &manifest.tools)
                        .into_iter()
                        .map(|d| d.name)
                        .collect(),
                    mcp_servers: manifest
                        .mcp_servers
                        .iter()
                        .map(|s| s.id.clone())
                        .collect(),
                    providers: read_provider_presets(&pdir, &manifest)
                        .into_iter()
                        .map(|p| p.id)
                        .collect(),
                    embeddings: read_embedding_provider_presets(&pdir, &manifest)
                        .into_iter()
                        .map(|p| p.id)
                        .collect(),
                    name: name.clone(),
                    version: manifest.version,
                    description: manifest.description,
                    status: "loaded".into(),
                    trust: trust_status_of(&key, &fingerprint).to_string(),
                    reason: None,
                    dir: dir_str,
                    key,
                })
            }
            Err((name, reason)) => out.push(PluginInventoryEntry {
                key: pdir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string(),
                name,
                version: String::new(),
                description: String::new(),
                skills: Vec::new(),
                tools: Vec::new(),
                mcp_servers: Vec::new(),
                providers: Vec::new(),
                embeddings: Vec::new(),
                status: "skipped".into(),
                trust: "untrusted".into(),
                reason: Some(reason),
                dir: dir_str,
            }),
        }
    }
    out
}

/// 装载全部插件的技能与 MCP 声明（启动路径；幂等：技能原子替换、MCP 按 id 去重）。
///
/// MCP 合并必须早于 `McpManager::init_all`（插件 server 才会被连接），
/// 因此本函数在 AppState 构造期调用；工具贡献的注册独立为
/// [`load_all_tools`]——它依赖 `register_builtin_tools` 先完成（防影子化
/// 校验才有效），时点在 initialize()。
pub fn load_all(skill_service: &SkillService, mcp_manager: &McpManager) -> PluginLoadReport {
    let dir = plugins_dir();
    if !dir.exists() {
        // 首次运行创建目录，用户放入插件即生效（下次启动装载）
        let _ = std::fs::create_dir_all(&dir);
        return PluginLoadReport::default();
    }
    let mut report = PluginLoadReport::default();

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!("[Plugins] 读取插件目录 {} 失败: {err}", dir.display());
            return report;
        }
    };
    // 目录名排序保证装载顺序稳定（glob 冲突时先到先得）
    let mut plugin_dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    plugin_dirs.sort();

    for pdir in plugin_dirs {
        if !pdir.join("plugin.json").exists() {
            continue;
        }
        let manifest = match read_manifest(&pdir) {
            Ok((_, m)) => m,
            Err((name, reason)) => {
                report.skipped.push(format!("{name}: {reason}"));
                continue;
            }
        };
        let plugin_name = manifest.name.clone();
        let dir_key = pdir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();

        // 信任门禁：未信任/清单已变更的插件不装载任何贡献（MCP 声明会执行
        // 本地命令、工具会执行 PowerShell——投放文件不等于授权执行）
        let fingerprint = cached_fingerprint(&pdir, &manifest);
        let trust = trust_status_of(&dir_key, &fingerprint);
        if trust != "trusted" {
            report.skipped.push(format!(
                "{plugin_name}: 未信任（{trust}），技能/工具/MCP 未装载——请在设置 → 插件页信任后重载"
            ));
            continue;
        }

        // 贡献点 1：技能（glob 展开相对插件目录，命名空间注册，同名原子替换幂等）
        let mut registered_skills = Vec::new();
        for (skill_name, description, keywords, body) in expanded_skills(&pdir, &plugin_name, &manifest.skills) {
            skill_service.replace_or_register(
                crate::skills::Skill::global(skill_name.clone(), description, body)
                    .with_keywords(keywords),
            );
            report.skills.push(skill_name.clone());
            registered_skills.push(skill_name);
        }

        // 贡献点 2：MCP servers（按插件归属合并，用户手配优先）
        let added = mcp_manager.merge_plugin_servers(&manifest.mcp_servers, &plugin_name);
        report.mcp_servers.extend(added.iter().cloned());

        // 记录贡献快照（更新/卸载时按快照撤销，防旧版本贡献残留）
        store_snapshot(
            &dir_key,
            PluginContribSnapshot {
                skills: registered_skills,
                tools: read_snapshot(&dir_key).map(|s| s.tools).unwrap_or_default(),
                mcp_ids: added,
                js: read_snapshot(&dir_key).map(|s| s.js).unwrap_or(false),
            },
        );

        report.plugins.push(plugin_name.clone());
        tracing::info!(
            "[Plugins] 装载插件 {} v{}（{}）：{} 条技能声明、{} 条工具声明、{} 个 MCP server 声明",
            plugin_name,
            manifest.version,
            manifest.description,
            manifest.skills.len(),
            manifest.tools.len(),
            manifest.mcp_servers.len()
        );
    }

    // 贡献点 5：JS 插件（boa 宿主装载，ctx.provider 贡献动态协议，
    // ctx.skill / ctx.mcp 贡献在此注册进 SkillService / McpManager；
    // ctx.tool 贡献的工具注册在 [`load_all_tools`]——须等内置工具注册完成，
    // 防影子化校验才有效）
    let js_report = js_host::load_all_js();
    report.protocols.extend(js_report.protocols);
    for (plugin, full_name, description, body) in &js_report.skills {
        skill_service.replace_or_register(crate::skills::Skill::global(
            full_name.clone(),
            description.clone(),
            body.clone(),
        ));
        report.skills.push(full_name.clone());
        // 把 JS 技能并入对应插件快照（技能按命名空间前缀撤销，此处补记全名）
        if let Some(key) = js_report.plugin_keys.get(plugin) {
            let mut snap = read_snapshot(key).unwrap_or_default();
            if !snap.skills.contains(full_name) {
                snap.skills.push(full_name.clone());
            }
            snap.js = true;
            store_snapshot(key, snap);
        }
    }
    for (plugin, config) in &js_report.mcp {
        let added = mcp_manager.merge_plugin_servers(std::slice::from_ref(config), plugin);
        report.mcp_servers.extend(added.iter().cloned());
        if let Some(key) = js_report.plugin_keys.get(plugin) {
            let mut snap = read_snapshot(key).unwrap_or_default();
            for id in &added {
                if !snap.mcp_ids.contains(id) {
                    snap.mcp_ids.push(id.clone());
                }
            }
            snap.js = true;
            store_snapshot(key, snap);
        }
    }
    // JS 工具（ctx.tool）不在此注册：load_all 阶段 ToolSystem 尚未注册内置
    // 工具，防影子化校验无效——统一延后到 [`load_all_tools`]
    for skip in &js_report.skipped {
        report.skipped.push(format!("js: {skip}"));
    }
    set_last_report(&report);
    report
}

/// 装载全部插件贡献的工具（启动路径的后半段，须在 `register_builtin_tools`
/// 之后、自建工具装载之前调用）。
///
/// 防影子化：与已注册工具（内置等）重名的定义跳过，后注册不得覆盖先注册。
/// 自建工具在本函数之后装载，与插件工具同名时自建覆盖插件（一等公民优先）。
/// 返回 `plugin/tool` 形式的展示名列表。
pub fn load_all_tools(tool_system: &ToolSystem) -> Vec<String> {
    let dir = plugins_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut loaded: Vec<String> = Vec::new();
    let mut all_registered: Vec<String> = Vec::new();
    let mut plugin_dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .collect();
    plugin_dirs.sort();
    for pdir in plugin_dirs {
        if !pdir.is_dir() || !pdir.join("plugin.json").exists() {
            continue;
        }
        let Ok((plugin_name, manifest)) = read_manifest(&pdir) else {
            continue;
        };
        // 信任门禁：与 load_all 同一规则（未信任插件的工具不注册）
        let dir_key = pdir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        if trust_status_of(&dir_key, &cached_fingerprint(&pdir, &manifest)) != "trusted" {
            tracing::warn!(
                "[Plugins] 插件 {plugin_name} 未信任（清单可能已变更），工具未注册"
            );
            continue;
        }
        let mut registered_names = Vec::new();
        let defs = expanded_tools(&pdir, &manifest.tools);
        // AST 审计批量过检（一个 PS 进程处理本插件全部工具；不可用即整批拒绝）
        let scripts: Vec<String> = defs.iter().map(|d| d.script.clone()).collect();
        let audits = match crate::tools::ps_audit::audit_scripts_blocking(&scripts) {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("[Plugins] 插件 {plugin_name} 工具审计不可用，全部跳过: {e}");
                push_diag_event(format!(
                    "插件 {plugin_name} 工具审计子系统不可用，{n} 个工具被跳过: {e}",
                    n = scripts.len()
                ));
                continue;
            }
        };
        for (def, audit) in defs.into_iter().zip(audits) {
            if tool_system.has_tool(&def.name) {
                tracing::warn!(
                    "[Plugins] 插件 {} 工具「{}」与已注册工具重名，跳过",
                    plugin_name,
                    def.name
                );
                push_diag_event(format!(
                    "插件 {plugin_name} 工具「{}」与已注册工具重名，跳过（不得影子化）",
                    def.name
                ));
                continue;
            }
            if let Some(summary) = audit.summary() {
                tracing::warn!(
                    "[Plugins] 插件 {plugin_name} 工具「{}」审计未通过，跳过: {}",
                    def.name,
                    summary.replace('\n', " | ")
                );
                continue;
            }
            let name = def.name.clone();
            tool_system.register_tool_with_owner(
                std::sync::Arc::new(DynamicTool::new(def)),
                Some(format!("plugin:{plugin_name}")),
            );
            loaded.push(format!("{plugin_name}/{name}"));
            registered_names.push(name);
        }
        // 并入贡献快照（工具在 load_all 之后单独装载，此处补记）
        let mut snap = read_snapshot(&dir_key).unwrap_or_default();
        for name in &registered_names {
            if !snap.tools.contains(name) {
                snap.tools.push(name.clone());
            }
        }
        store_snapshot(&dir_key, snap);
        all_registered.extend(registered_names);
    }

    // JS 插件贡献的工具（ctx.tool → JsPluginTool，执行路由回插件 worker）。
    // 启动时序保证：load_all 先装载 JS 运行时，此处捕获的贡献才可用。
    for (plugin, key, name, description, schema) in js_host::captured_tools() {
        if tool_system.has_tool(&name) {
            tracing::warn!(
                "[Plugins] 插件 {} 的 JS 工具「{}」与已注册工具重名，跳过",
                plugin,
                name
            );
            push_diag_event(format!(
                "插件 {plugin} 的 JS 工具「{name}」与已注册工具重名，跳过（不得影子化）"
            ));
            continue;
        }
        let Some(params) = crate::tools::custom_tools::sanitize_parameters(&schema) else {
            tracing::warn!(
                "[Plugins] 插件 {} 的 JS 工具「{}」参数 schema 非 object，跳过",
                plugin,
                name
            );
            continue;
        };
        tool_system.register_tool_with_owner(
            std::sync::Arc::new(js_host::JsPluginTool::new(plugin.clone(), name.clone(), description, params)),
            Some(format!("plugin:{plugin}")),
        );
        loaded.push(format!("{plugin}/{name}"));
        // 归属快照按目录 key 记录（卸载时随插件贡献一并撤销）
        let mut snap = read_snapshot(&key).unwrap_or_default();
        if !snap.tools.contains(&name) {
            snap.tools.push(name.clone());
        }
        store_snapshot(&key, snap);
        all_registered.push(name);
    }
    record_loaded_tools(all_registered);
    loaded
}

// ============================================================================
// 运行时装载 / 卸载 / 删除（创造模式落点）
// ============================================================================

/// 内置插件目录名（播种体系所有；运行时删除与 create_plugin 覆盖的目标禁入）。
pub const BUILTIN_PLUGIN_DIRS: &[&str] = &[BUILTIN_PLUGIN_DIR, "plugin-authoring"];

/// 目录名（安全字符校验与插件名一致），供装卸 API 以目录为主键定位插件。
fn valid_plugin_dir_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 从磁盘目录读出插件当前贡献的工具定义（manifest 可读用其 glob 声明，
/// 不可读时兜底扫 `tools/*.json` 约定布局），供卸载时展开工具名单。
fn tool_defs_on_disk(pdir: &Path) -> Vec<CustomToolDef> {
    let mut defs = match read_manifest(pdir) {
        Ok((_, m)) => expanded_tools(pdir, &m.tools),
        Err(_) => expanded_tools(pdir, &["tools/*.json".to_string()]),
    };
    // 约定布局兜底并入（glob 声明可能指向别处，两者并集防卸载残留）
    for extra in expanded_tools(pdir, &["tools/*.json".to_string()]) {
        if !defs.iter().any(|d| d.name == extra.name) {
            defs.push(extra);
        }
    }
    defs
}

/// 撤销一个插件目录的全部运行时贡献（技能按命名空间前缀、工具按 owner、
/// MCP 按归属），不动磁盘文件。幂等：贡献不存在时静默通过。
///
/// 撤销依据 = **装载快照优先**（插件更新后磁盘只剩新版本声明，旧版本
/// 贡献只有快照记得），磁盘展开名单兜底（覆盖无快照的历史装载）。
/// manifest.name 与目录名不一致时两个名字都清（防手写清单时命名空间漂移）。
async fn unload_contributions(
    skill_service: &SkillService,
    mcp_manager: &McpManager,
    tool_system: &ToolSystem,
    pdir: &Path,
) {
    let dir_name = pdir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let ns = read_manifest(pdir)
        .map(|(n, _)| n)
        .unwrap_or_else(|_| dir_name.clone());

    // ---- 快照撤销（装载时实际注册的内容）----
    let snap = read_snapshot(&dir_name);
    let mut removed_skills = Vec::new();
    let mut removed_tools = Vec::new();
    let mut removed_servers = Vec::new();
    if let Some(snap) = &snap {
        // 先移除更长的名字（前缀匹配语义下，"ns/old_extra" 先于 "ns/old" 撤销，
        // 避免短前缀误伤同前缀的长名技能）
        let mut skill_names = snap.skills.clone();
        skill_names.sort();
        skill_names.dedup();
        for name in skill_names.iter().rev() {
            removed_skills.extend(skill_service.remove_by_prefix(name));
        }
        let owners = if ns == dir_name {
            vec![format!("plugin:{ns}")]
        } else {
            vec![format!("plugin:{ns}"), format!("plugin:{dir_name}")]
        };
        for name in &snap.tools {
            for owner in &owners {
                if tool_system.unregister_tool_if_owner(name, owner) {
                    removed_tools.push(name.clone());
                }
            }
        }
        for id in &snap.mcp_ids {
            if mcp_manager.remove_server(id).await.is_ok() {
                removed_servers.push(id.clone());
            }
        }
    }

    // ---- 磁盘展开撤销（无快照的历史装载兜底，幂等）----
    if ns != dir_name {
        removed_skills.extend(skill_service.remove_by_prefix(&format!("{ns}/")));
    }
    removed_skills.extend(skill_service.remove_by_prefix(&format!("{dir_name}/")));
    let disk_tools: Vec<String> = tool_defs_on_disk(pdir)
        .into_iter()
        .map(|d| d.name)
        .collect();
    let owners = if ns == dir_name {
        vec![format!("plugin:{ns}")]
    } else {
        vec![format!("plugin:{ns}"), format!("plugin:{dir_name}")]
    };
    for name in &disk_tools {
        for owner in &owners {
            if tool_system.unregister_tool_if_owner(name, owner) {
                removed_tools.push(name.clone());
            }
        }
    }
    let mut disk_servers = mcp_manager.remove_plugin_servers(&ns).await;
    if ns != dir_name {
        disk_servers.extend(mcp_manager.remove_plugin_servers(&dir_name).await);
    }
    removed_servers.extend(disk_servers);
    tracing::info!(
        "[Plugins] 已卸载插件 {} 的运行时贡献：{} 条技能、{} 个工具、{} 个 MCP server",
        dir_name,
        removed_skills.len(),
        removed_tools.len(),
        removed_servers.len()
    );
}

/// 装载（或重载）单个插件：先撤销其旧贡献，再按磁盘当前内容注册全部
/// 贡献点，并连接本插件名下待连接的 MCP server。
///
/// `key` 是插件目录名（文件系统主键）。`authorized` 表示本次装载携带用户
/// 授权（create_plugin 的用户预览卡片 / 设置窗口的信任动作）——授权装载
/// 会同时写入信任指纹；普通重载（设置页「重载」按钮）无授权，未信任或
/// 清单已变更的插件会被拒绝。
pub async fn load_one(
    skill_service: &SkillService,
    mcp_manager: &McpManager,
    tool_system: &ToolSystem,
    key: &str,
    authorized: bool,
) -> Result<PluginLoadReport, String> {
    let pdir = plugins_dir().join(key);
    if !valid_plugin_dir_name(key) {
        return Err(format!("插件目录名非法（仅允许字母/数字/-/_）: {key}"));
    }
    if !pdir.is_dir() {
        return Err(format!("插件目录不存在: {}", pdir.display()));
    }
    let (plugin_name, manifest) = read_manifest(&pdir)
        .map_err(|(_, reason)| format!("插件 {key} 清单不可用: {reason}"))?;
    let fingerprint = cached_fingerprint(&pdir, &manifest);

    // 信任门禁：授权装载在成功后写入信任；普通重载要求已信任且清单未变更
    let trust = trust_status_of(key, &fingerprint);
    if !authorized && trust != "trusted" {
        return Err(format!(
            "插件 {key} 未信任或清单已变更（{0}），请先在设置 → 插件页信任后重载",
            trust
        ));
    }

    // 重载语义：先撤销旧贡献（含 manifest 已损坏的情况——尽力按目录布局卸载）
    unload_contributions(skill_service, mcp_manager, tool_system, &pdir).await;

    let mut report = PluginLoadReport::default();
    report.plugins.push(plugin_name.clone());

    // 技能：命名空间注册（幂等）
    let mut registered_skills = Vec::new();
    for (skill_name, description, keywords, body) in
        expanded_skills(&pdir, &plugin_name, &manifest.skills)
    {
        skill_service.replace_or_register(
            crate::skills::Skill::global(skill_name.clone(), description, body)
                .with_keywords(keywords),
        );
        report.skills.push(skill_name.clone());
        registered_skills.push(skill_name);
    }

    // 工具：防影子化（重载路径上旧贡献已卸载，has_tool 命中的只可能是
    // 内置/自建/其他插件的工具）+ AST 审计批量过检
    let mut registered_tools = Vec::new();
    let defs = expanded_tools(&pdir, &manifest.tools);
    let scripts: Vec<String> = defs.iter().map(|d| d.script.clone()).collect();
    let scripts_clone = scripts.clone();
    let audits = tokio::task::spawn_blocking(move || {
        crate::tools::ps_audit::audit_scripts_blocking(&scripts_clone)
    })
    .await
    .map_err(|e| format!("审计任务执行失败: {e}"))?
    .map_err(|e| format!("工具脚本审计不可用: {e}"))?;
    for (def, audit) in defs.into_iter().zip(audits) {
        if tool_system.has_tool(&def.name) {
            tracing::warn!(
                "[Plugins] 插件 {} 工具「{}」与已注册工具重名，跳过",
                plugin_name,
                def.name
            );
            push_diag_event(format!(
                "插件 {plugin_name} 工具「{}」与已注册工具重名，跳过（不得影子化）",
                def.name
            ));
            continue;
        }
        if let Some(summary) = audit.summary() {
            return Err(format!(
                "插件 {plugin_name} 工具「{}」脚本审计未通过：{}",
                def.name,
                summary.replace('\n', " | ")
            ));
        }
        let name = def.name.clone();
        tool_system.register_tool_with_owner(
            std::sync::Arc::new(DynamicTool::new(def)),
            Some(format!("plugin:{plugin_name}")),
        );
        report.tools.push(name.clone());
        registered_tools.push(name);
    }

    // MCP：合并（归属标记）后逐个连接
    let mut added = mcp_manager.merge_plugin_servers(&manifest.mcp_servers, &plugin_name);
    let config_by_id: HashMap<String, McpServerConfig> = mcp_manager
        .load_configs()
        .into_iter()
        .map(|c| (c.id.clone(), c))
        .collect();
    for id in &added {
        let Some(config) = config_by_id.get(id).cloned() else { continue };
        match mcp_manager.add_server(config).await {
            Ok(tools) => {
                tracing::info!(
                    "[Plugins] 插件 {} 的 MCP server [{}] 已连接，注册 {} 个工具",
                    plugin_name,
                    id,
                    tools.len()
                );
                report.mcp_servers.push(id.clone());
            }
            Err(e) => {
                tracing::warn!("[Plugins] 插件 {} 的 MCP server [{}] 连接失败: {e}", plugin_name, id)
            }
        }
    }

    // JS 贡献：装载（或重载）该插件的 JS 运行时，重建动态协议注册表
    let js_report = js_host::reload_one(key);
    report.protocols.extend(js_report.protocols);
    for (_plugin, full_name, description, body) in &js_report.skills {
        skill_service.replace_or_register(crate::skills::Skill::global(
            full_name.clone(),
            description.clone(),
            body.clone(),
        ));
        report.skills.push(full_name.clone());
        registered_skills.push(full_name.clone());
    }
    for (_plugin, config) in &js_report.mcp {
        let merged = mcp_manager.merge_plugin_servers(std::slice::from_ref(config), &plugin_name);
        if let Some(loaded) = merged.first().cloned() {
            // 合并成功即归属本插件，无论连接成败都记入快照（卸载时按 id 撤销）
            if !added.contains(&loaded) {
                added.push(loaded.clone());
            }
            if let Some(cfg) = mcp_manager
                .load_configs()
                .into_iter()
                .find(|c| c.id == loaded)
            {
                match mcp_manager.add_server(cfg).await {
                    Ok(_) => report.mcp_servers.push(loaded.clone()),
                    Err(e) => tracing::warn!(
                        "[Plugins] 插件 {} 的 JS MCP server [{}] 连接失败: {e}",
                        plugin_name,
                        loaded
                    ),
                }
            }
        }
    }
    // JS 工具：注册进 ToolSystem（执行路由回插件 worker），防影子化同文件工具
    for (plugin, name, description, schema) in &js_report.tools {
        if tool_system.has_tool(name) {
            tracing::warn!(
                "[Plugins] 插件 {} 的 JS 工具「{}」与已注册工具重名，跳过",
                plugin,
                name
            );
            continue;
        }
        let Some(params) = crate::tools::custom_tools::sanitize_parameters(schema) else {
            tracing::warn!(
                "[Plugins] 插件 {} 的 JS 工具「{}」参数 schema 非 object，跳过",
                plugin,
                name
            );
            continue;
        };
        tool_system.register_tool_with_owner(
            std::sync::Arc::new(js_host::JsPluginTool::new(
                plugin.clone(),
                name.clone(),
                description.clone(),
                params,
            )),
            Some(format!("plugin:{plugin_name}")),
        );
        report.tools.push(name.clone());
        registered_tools.push(name.clone());
    }
    for skip in &js_report.skipped {
        report.skipped.push(format!("js: {skip}"));
    }

    // 装载成功 → 原子替换贡献快照 + 写入信任（授权路径）
    store_snapshot(
        key,
        PluginContribSnapshot {
            skills: registered_skills,
            tools: registered_tools,
            mcp_ids: added,
            js: !js_report.plugins.is_empty(),
        },
    );
    if authorized {
        record_trust(key, &fingerprint)?;
        invalidate_fingerprint_cache();
    }

    tracing::info!(
        "[Plugins] 插件 {} v{} 已装载：{} 条技能、{} 个工具、{} 个 MCP server、{} 个 JS 协议",
        plugin_name,
        manifest.version,
        report.skills.len(),
        report.tools.len(),
        report.mcp_servers.len(),
        report.protocols.len()
    );
    set_last_report(&report);
    Ok(report)
}

/// 卸载单个插件的运行时贡献（不动磁盘文件；重启后按目录重新装载）。
pub async fn unload_one(
    skill_service: &SkillService,
    mcp_manager: &McpManager,
    tool_system: &ToolSystem,
    key: &str,
) -> Result<(), String> {
    let pdir = plugins_dir().join(key);
    if !valid_plugin_dir_name(key) || !pdir.is_dir() {
        return Err(format!("插件目录不存在: {key}"));
    }
    if BUILTIN_PLUGIN_DIRS.contains(&key) {
        return Err(format!("内置插件 {key} 不可卸载（它是播种体系的一部分）"));
    }
    unload_contributions(skill_service, mcp_manager, tool_system, &pdir).await;
    js_host::unload_one(key);
    drop_snapshot(key);
    Ok(())
}

/// 删除插件：撤销运行时贡献并移除目录。内置插件禁删。
pub async fn delete_plugin(
    skill_service: &SkillService,
    mcp_manager: &McpManager,
    tool_system: &ToolSystem,
    key: &str,
) -> Result<(), String> {
    let pdir = plugins_dir().join(key);
    if !valid_plugin_dir_name(key) || !pdir.is_dir() {
        return Err(format!("插件目录不存在: {key}"));
    }
    if BUILTIN_PLUGIN_DIRS.contains(&key) {
        return Err(format!(
            "内置插件 {key} 不可删除——改内置的正确方式是复制为新插件再改"
        ));
    }
    unload_contributions(skill_service, mcp_manager, tool_system, &pdir).await;
    js_host::unload_one(key);
    drop_snapshot(key);
    std::fs::remove_dir_all(&pdir)
        .map_err(|e| format!("删除插件目录 {} 失败: {e}", pdir.display()))?;
    tracing::info!("[Plugins] 插件 {key} 已删除");
    Ok(())
}

/// 插件草稿——`create_plugin` 元工具的落盘载荷。
#[derive(Debug, Clone)]
pub struct PluginDraft {
    pub name: String,
    pub version: String,
    pub description: String,
    /// 技能文件（文件名 → 正文），落盘到 `skills/` 子目录
    pub skills: Vec<(String, String)>,
    /// 可执行工具定义（落盘到 `tools/` 子目录，一工具一文件）
    pub tools: Vec<CustomToolDef>,
    /// MCP server 声明（写入 plugin.json）
    pub mcp_servers: Vec<McpServerConfig>,
    /// 供应商预设（落盘为 providers.json）
    pub providers: Vec<ProviderPresetData>,
    /// 云端嵌入预设（落盘为 embedding-providers.json）
    pub embeddings: Vec<EmbeddingProviderPresetData>,
}

/// 校验并原子落盘一个插件草稿，返回插件目录路径。
///
/// 原子性：先写入同级的 `.<name>.tmp` 临时目录，全部成功后替换正式目录
/// （更新场景先移除旧目录——运行时贡献的撤销由调用方在落盘前完成）。
/// 校验失败或任一写入失败时不触碰正式目录。
pub fn write_plugin_files(draft: &PluginDraft) -> Result<PathBuf, String> {
    // ---- 全量校验（先校验后写盘，写不出坏数据）----
    let name = draft.name.trim();
    if !valid_plugin_dir_name(name) {
        return Err(format!("插件名非法（仅允许字母/数字/-/_）: {name}"));
    }
    if BUILTIN_PLUGIN_DIRS.contains(&name) {
        return Err(format!(
            "内置插件 {name} 禁止覆盖——改内置的正确方式是复制为新插件再改"
        ));
    }
    if draft.version.trim().is_empty() {
        return Err("version 是必填项（如 \"1.0.0\"）".into());
    }
    // 技能文件名：去扩展名后作技能名，须安全（防路径穿越与命名空间污染）
    for (fname, body) in &draft.skills {
        let stem = fname.trim().trim_end_matches(".md");
        if stem.is_empty()
            || !stem
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!(
                "技能文件名非法（仅允许字母/数字/-/_，如 \"my_skill.md\"）: {fname}"
            ));
        }
        if body.trim().is_empty() {
            return Err(format!("技能 {fname} 正文为空"));
        }
    }
    // 工具定义：复用自建工具的全部校验规则
    for tool in &draft.tools {
        if !crate::tools::custom_tools::is_valid_tool_name(&tool.name) {
            return Err(format!(
                "工具名仅允许 ASCII 字母、数字、下划线、连字符（1-64 字符）: {}",
                tool.name
            ));
        }
        if tool.description.trim().is_empty() {
            return Err(format!("工具 {} 缺 description", tool.name));
        }
        if tool.script.trim().is_empty() {
            return Err(format!("工具 {} 缺 script", tool.name));
        }
        if let Some(frag) = crate::tools::custom_tools::forbidden_fragment_in(&tool.script) {
            return Err(format!(
                "工具 {} 的脚本包含破坏性片段「{frag}」，已被拒绝",
                tool.name
            ));
        }
        if crate::tools::custom_tools::sanitize_parameters(&tool.parameters).is_none() {
            return Err(format!("工具 {} 的 parameters 必须是 object JSON Schema", tool.name));
        }
        // AST 审计（阻塞批量）：create_plugin 落盘前拦截破坏性脚本
        let audits = crate::tools::ps_audit::audit_scripts_blocking(
            &draft.tools.iter().map(|t| t.script.clone()).collect::<Vec<_>>(),
        )?;
        for (tool, audit) in draft.tools.iter().zip(&audits) {
            if let Some(summary) = audit.summary() {
                return Err(format!(
                    "工具 {} 脚本审计未通过：{}",
                    tool.name,
                    summary.replace('\n', " | ")
                ));
            }
        }
    }
    // 工具名互相重复
    let mut seen = std::collections::HashSet::new();
    for tool in &draft.tools {
        if !seen.insert(tool.name.clone()) {
            return Err(format!("草稿内工具名重复: {}", tool.name));
        }
    }
    // MCP 声明：id 安全字符 + command 非空 + id 不重复
    let mut mcp_ids = std::collections::HashSet::new();
    for cfg in &draft.mcp_servers {
        if cfg.id.trim().is_empty()
            || !cfg
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!("MCP server id 非法（仅允许字母/数字/-/_）: {}", cfg.id));
        }
        if !mcp_ids.insert(cfg.id.clone()) {
            return Err(format!("草稿内 MCP server id 重复: {}", cfg.id));
        }
        if cfg.command.trim().is_empty() {
            return Err(format!("MCP server {} 缺 command", cfg.id));
        }
    }
    // 供应商预设：id/provider_type 校验对齐 upsert 规则
    for preset in &draft.providers {
        if preset.id.trim().is_empty()
            || !preset
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!("供应商预设 id 非法: {}", preset.id));
        }
        if preset.provider_type.trim().is_empty() {
            return Err(format!("供应商预设 {} 缺 provider_type", preset.id));
        }
    }
    for preset in &draft.embeddings {
        validate_embedding_preset(preset)?;
    }

    // ---- 落盘（临时目录 → 替换正式目录）----
    let base = plugins_dir();
    std::fs::create_dir_all(&base).map_err(|e| format!("创建插件根目录失败: {e}"))?;
    let tmp = base.join(format!(".{name}.tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("skills"))
        .and_then(|_| std::fs::create_dir_all(tmp.join("tools")))
        .map_err(|e| format!("创建临时插件目录失败: {e}"))?;

    // plugin.json（tools 声明固定指向 tools/*.json；providers 固定文件名）
    let manifest = serde_json::json!({
        "name": name,
        "version": draft.version.trim(),
        "description": draft.description.trim(),
        "skills": if draft.skills.is_empty() { Vec::<String>::new() } else { vec!["skills/*.md".to_string()] },
        "tools": if draft.tools.is_empty() { Vec::<String>::new() } else { vec!["tools/*.json".to_string()] },
        "mcp_servers": draft.mcp_servers,
        "providers": if draft.providers.is_empty() { serde_json::Value::Null } else { serde_json::json!("providers.json") },
        "embeddings": if draft.embeddings.is_empty() { serde_json::Value::Null } else { serde_json::json!("embedding-providers.json") },
    });
    let manifest = {
        // providers 为 null 时移除键（Option 语义）
        let mut m = manifest;
        if m.get("providers").map(|v| v.is_null()).unwrap_or(false) {
            m.as_object_mut().unwrap().remove("providers");
        }
        if m.get("embeddings").map(|v| v.is_null()).unwrap_or(false) {
            m.as_object_mut().unwrap().remove("embeddings");
        }
        m
    };
    std::fs::write(
        tmp.join("plugin.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|e| format!("序列化 plugin.json 失败: {e}"))?,
    )
    .map_err(|e| format!("写 plugin.json 失败: {e}"))?;

    for (fname, body) in &draft.skills {
        std::fs::write(tmp.join("skills").join(fname.trim()), body)
            .map_err(|e| format!("写技能文件 {fname} 失败: {e}"))?;
    }
    for tool in &draft.tools {
        std::fs::write(
            tmp.join("tools").join(format!("{}.json", tool.name)),
            serde_json::to_vec_pretty(tool)
                .map_err(|e| format!("序列化工具 {} 失败: {e}", tool.name))?,
        )
        .map_err(|e| format!("写工具文件 {} 失败: {e}", tool.name))?;
    }
    if !draft.providers.is_empty() {
        std::fs::write(
            tmp.join("providers.json"),
            serde_json::to_vec_pretty(&draft.providers)
                .map_err(|e| format!("序列化 providers.json 失败: {e}"))?,
        )
        .map_err(|e| format!("写 providers.json 失败: {e}"))?;
    }
    if !draft.embeddings.is_empty() {
        std::fs::write(
            tmp.join("embedding-providers.json"),
            serde_json::to_vec_pretty(&draft.embeddings)
                .map_err(|e| format!("序列化 embedding-providers.json 失败: {e}"))?,
        )
        .map_err(|e| format!("写 embedding-providers.json 失败: {e}"))?;
    }

    // 三段式原子替换（final→backup, tmp→final, 删 backup），复用 utils::fs::write_atomic_dir
    let final_dir = base.join(name);
    crate::utils::fs::write_atomic_dir(&final_dir, &tmp)
        .map_err(|e| format!("落盘插件目录失败（已回滚）: {e}"))?;

    tracing::info!(
        "[Plugins] 插件 {name} v{} 已落盘（{} 条技能、{} 个工具、{} 个 MCP server、{} 条 LLM 预设、{} 条嵌入预设）",
        draft.version.trim(),
        draft.skills.len(),
        draft.tools.len(),
        draft.mcp_servers.len(),
        draft.providers.len(),
        draft.embeddings.len()
    );
    Ok(final_dir)
}

/// 简化 glob：支持 `*.md` 后缀匹配与目录递归两种模式（够用即可，不引依赖）
///
/// - `skills/*.md` → 插件目录下 skills/ 一层内的 .md 文件
/// - `skills` 或 `skills/**` → 递归收集该子目录全部 .md
fn glob_files(base: &Path, pattern: &str) -> Vec<PathBuf> {
    if !is_safe_manifest_path(pattern) {
        tracing::warn!("[Plugins] 拒绝越界技能路径声明: {}", pattern);
        return Vec::new();
    }
    let pattern = pattern.trim().trim_start_matches("./");
    let (dir_part, suffix_glob) = match pattern.rsplit_once('/') {
        Some((d, s)) => (d.to_string(), s.to_string()),
        None => (String::new(), pattern.to_string()),
    };
    let recursive = suffix_glob == "**" || suffix_glob.is_empty();
    let want_md = suffix_glob == "*.md" || recursive;
    if !want_md {
        return Vec::new();
    }
    let root = if dir_part.is_empty() {
        base.to_path_buf()
    } else {
        base.join(dir_part)
    };
    if !root.is_dir() || !is_safe_plugin_path(base, &root) {
        return Vec::new();
    }
    let mut out = Vec::new();
    collect_md(base, &root, recursive, &mut out);
    out.sort();
    out
}

fn collect_md(base: &Path, dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                if is_safe_plugin_path(base, &path) {
                    collect_md(base, &path, recursive, out);
                }
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md")
            && is_safe_plugin_path(base, &path)
        {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parse_and_compare() {
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.0"), Some((1, 0, 0)));
        assert_eq!(parse_version(" 2.0.10 "), Some((2, 0, 10)));
        assert_eq!(parse_version("abc"), None);
        assert!(parse_version("1.9.9").unwrap() < parse_version("2.0.0").unwrap());
    }

    #[test]
    fn builtin_providers_json_parses() {
        // 内置 providers.json 必须能反序列化为预设数组（防字段拼写/类型漂移）
        let rows: Vec<ProviderPresetData> = serde_json::from_str(BUILTIN_PLUGIN_PROVIDERS)
            .expect("内置 providers.json 应可解析");
        assert!(!rows.is_empty());
        for p in &rows {
            assert!(
                !p.id.trim().is_empty()
                    && p.id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "预设 id 含非法字符: {}",
                p.id
            );
            assert!(!p.provider_type.is_empty(), "预设 {} 缺 provider_type", p.id);
        }
        // 内置数据不含 custom（「自定义」是前端 UI 概念，由内置兜底提供）
        assert!(!rows.iter().any(|p| p.id == "custom"));
    }

    #[test]
    fn builtin_manifest_declares_providers() {
        let manifest: PluginManifest =
            serde_json::from_str(BUILTIN_PLUGIN_MANIFEST).expect("内置 plugin.json 应可解析");
        assert_eq!(manifest.name, "llm-providers");
        assert!(manifest.providers.is_some());
        assert!(manifest.embeddings.is_some());
        assert!(!manifest.skills.is_empty());
    }

    #[test]
    fn builtin_embedding_providers_json_parses() {
        let rows: Vec<EmbeddingProviderPresetData> =
            serde_json::from_str(BUILTIN_PLUGIN_EMBEDDINGS)
                .expect("内置 embedding-providers.json 应可解析");
        assert!(!rows.is_empty());
        // 内置数据必须逐条通过运行时校验，否则会被静默丢弃
        for p in &rows {
            assert!(
                is_valid_embedding_preset(p),
                "内置嵌入预设 {} 未通过校验",
                p.id
            );
            validate_embedding_preset(p).unwrap_or_else(|e| panic!("预设 {} 非法: {e}", p.id));
        }
        // id 不可重复：前端按下拉选中项反查厂商，重复会让选择结果不确定
        let mut ids: Vec<&str> = rows.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "内置嵌入预设 id 必须唯一");
        // 每个厂商至少一个模型，且模型名在厂商内唯一
        for p in &rows {
            assert!(!p.models.is_empty(), "预设 {} 没有模型", p.id);
            let mut models: Vec<&str> = p.models.iter().map(|m| m.model.as_str()).collect();
            models.sort_unstable();
            let before = models.len();
            models.dedup();
            assert_eq!(before, models.len(), "预设 {} 的模型名重复", p.id);
        }
    }

    /// 能力字段的落盘/读取必须闭环：dimensionParam 是适配器拼请求体的依据，
    /// 写错（比如把 Voyage 的 output_dimension 写成 dimensions）会直接 400。
    #[test]
    fn embedding_preset_capabilities_roundtrip() {
        let row: EmbeddingProviderPresetData = serde_json::from_str(
            r#"{
                "id": "vendor",
                "provider": "Vendor",
                "endpoint": "https://api.vendor.com/v1",
                "dimensionParam": "output_dimension",
                "needsApiKey": false,
                "models": [{ "model": "embed-v2", "dimension": 1024, "maxBatch": 16 }]
            }"#,
        )
        .expect("厂商级预设应可解析");
        assert_eq!(row.dimension_param.as_deref(), Some("output_dimension"));
        assert!(!row.needs_api_key);
        assert_eq!(row.models[0].max_batch, Some(16));

        // 缺省：不下发维度参数、需要 Key（旧数据无这两个字段时的兜底语义）
        let legacy: EmbeddingProviderPresetData = serde_json::from_str(
            r#"{"id":"v","provider":"V","endpoint":"https://x/v1","models":[{"model":"m","dimension":8}]}"#,
        )
        .expect("缺省字段应可解析");
        assert!(legacy.dimension_param.is_none());
        assert!(legacy.needs_api_key);

        // 非法参数名必须被拒（防止把任意字段塞进请求体）
        let mut bad = legacy.clone();
        bad.dimension_param = Some("dims".into());
        assert!(validate_embedding_preset(&bad).is_err());
    }

    #[test]
    fn manifest_paths_stay_inside_plugin() {
        assert!(!is_safe_manifest_path("../outside/*.json"));
        assert!(!is_safe_manifest_path("..\\outside\\x.js"));
        assert!(!is_safe_manifest_path("C:\\outside\\x.js"));
        assert!(is_safe_manifest_path("skills/**/*.md"));
    }

    /// 指纹必须锚定声明文件的内容：清单不变、只改工具脚本内容也要触发 changed。
    /// （信任模型的闭环——清单只声明 glob 路径，执行体在文件内容里）
    #[test]
    fn fingerprint_anchors_declared_file_content() {
        let dir = std::env::temp_dir().join(format!("vivian-fp-{}", uuid::Uuid::new_v4()));
        let tools = dir.join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"fp-test","version":"1.0.0","tools":["tools/*.json"]}"#,
        )
        .unwrap();
        let tool_def = |script: &str| {
            format!(
                r#"{{"name":"fp_tool","description":"t","parameters":{{"type":"object"}},"script":"{}","created_at":0}}"#,
                script
            )
        };
        std::fs::write(tools.join("fp_tool.json"), tool_def("Write-Output 1")).unwrap();
        let manifest_a: PluginManifest =
            serde_json::from_str(&std::fs::read_to_string(dir.join("plugin.json")).unwrap()).unwrap();
        let fp_a = manifest_fingerprint(&dir, &manifest_a);

        // 清单不变，只改脚本内容 → 指纹必须变化
        std::fs::write(tools.join("fp_tool.json"), tool_def("Write-Output 2")).unwrap();
        let fp_b = manifest_fingerprint(&dir, &manifest_a);
        assert_ne!(fp_a, fp_b, "声明文件内容变更必须产生不同指纹");

        // 新增声明文件（第二个工具）→ 指纹必须变化
        std::fs::write(
            tools.join("fp_tool2.json"),
            tool_def("Write-Output 3").replace("fp_tool", "fp_tool2"),
        )
        .unwrap();
        let fp_c = manifest_fingerprint(&dir, &manifest_a);
        assert_ne!(fp_b, fp_c, "新增声明文件必须产生不同指纹");

        // 内容回滚 → 指纹回到原值（确定性）
        std::fs::remove_file(tools.join("fp_tool2.json")).unwrap();
        std::fs::write(tools.join("fp_tool.json"), tool_def("Write-Output 1")).unwrap();
        let fp_d = manifest_fingerprint(&dir, &manifest_a);
        assert_eq!(fp_a, fp_d, "文件内容回滚后指纹应回到原值");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn glob_flat_and_recursive() {
        let dir = std::env::temp_dir().join(format!("vivian-plugin-{}", uuid::Uuid::new_v4()));
        let skills = dir.join("skills");
        let nested = skills.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(skills.join("a.md"), "A").unwrap();
        std::fs::write(nested.join("b.md"), "B").unwrap();
        std::fs::write(skills.join("c.txt"), "C").unwrap();

        // *.md 只取一层
        let flat = glob_files(&dir, "skills/*.md");
        assert_eq!(flat.len(), 1);
        assert!(flat[0].ends_with("a.md"));

        // ** 递归
        let deep = glob_files(&dir, "skills/**");
        assert_eq!(deep.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 构造测试用插件目录（一份 providers.json + plugin.json）
    fn test_plugin_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vivian-upsert-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("providers.json"),
            r#"[{"id":"demo","providerType":"openai","endpoint":"https://example.com/v1","defaultModel":"m1","mainModels":[]}]"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"llm-providers","version":"1.2.3","description":"t"}"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn upsert_replaces_existing_and_bumps_version() {
        let dir = test_plugin_dir();
        let preset = ProviderPresetData {
            id: "demo".into(),
            label_key: None,
            label: None,
            provider_type: "chat_completions".into(),
            endpoint: "https://example.com/v2".into(),
            default_model: "m2".into(),
            main_models: vec!["m2".into()],
            context_window: Some(256000),
            suggested_max_tokens: Some(8192),
            needs_secret: None,
            needs_app_id: None,
            console_url: None,
            protocols: None,
            verified_at: None, // 调用方不传——应由系统时间写入
            verified_source: Some("https://docs.example.com".into()),
        };
        let (row, version, is_new) = upsert_provider_preset_at(&dir, preset).expect("upsert 应成功");
        assert!(!is_new);
        assert_eq!(version, "1.2.4");
        assert_eq!(row.provider_type, "chat_completions");
        // verified_at 由系统时间写入（格式 YYYY-MM-DD），不为空且不信任调用方
        let verified = row.verified_at.expect("verified_at 应被写入");
        let vb = verified.as_bytes();
        assert_eq!(verified.len(), 10);
        assert_eq!(vb[4], b'-');
        assert_eq!(vb[7], b'-');
        assert_eq!(row.verified_source.as_deref(), Some("https://docs.example.com"));

        // 落盘验证：单行替换（不是追加），且带元数据
        let disk: Vec<ProviderPresetData> =
            serde_json::from_str(&std::fs::read_to_string(dir.join("providers.json")).unwrap())
                .unwrap();
        assert_eq!(disk.len(), 1);
        assert_eq!(disk[0].endpoint, "https://example.com/v2");
        assert_eq!(disk[0].verified_at.as_deref(), Some(verified.as_str()));
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("plugin.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["version"].as_str().unwrap(), "1.2.4");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_appends_new_id() {
        let dir = test_plugin_dir();
        let preset = ProviderPresetData {
            id: "new-vendor".into(),
            label_key: None,
            label: Some("New Vendor".into()),
            provider_type: "openai".into(),
            endpoint: "https://new.example.com".into(),
            default_model: "nv1".into(),
            main_models: vec![],
            context_window: None,
            suggested_max_tokens: None,
            needs_secret: None,
            needs_app_id: None,
            console_url: None,
            protocols: None,
            verified_at: None,
            verified_source: None,
        };
        let (row, version, is_new) = upsert_provider_preset_at(&dir, preset).expect("upsert 应成功");
        assert!(is_new);
        assert_eq!(row.id, "new-vendor");
        assert_eq!(version, "1.2.4");

        let disk: Vec<ProviderPresetData> =
            serde_json::from_str(&std::fs::read_to_string(dir.join("providers.json")).unwrap())
                .unwrap();
        assert_eq!(disk.len(), 2);
        assert!(disk.iter().any(|p| p.id == "demo"));
        assert!(disk.iter().any(|p| p.id == "new-vendor"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_stamps_today_via_system_clock() {
        // verified_at 必须由本地系统时间写入——调用方传入的旧日期被覆盖
        let dir = test_plugin_dir();
        let mut preset = ProviderPresetData {
            id: "demo".into(),
            label_key: None,
            label: None,
            provider_type: "openai".into(),
            endpoint: String::new(),
            default_model: String::new(),
            main_models: vec![],
            context_window: None,
            suggested_max_tokens: None,
            needs_secret: None,
            needs_app_id: None,
            console_url: None,
            protocols: None,
            verified_at: Some("1999-01-01".into()),
            verified_source: None,
        };
        let (row, _, _) = upsert_provider_preset_at(&dir, preset).expect("upsert 应成功");
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        assert_eq!(row.verified_at.as_deref(), Some(today.as_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_rejects_invalid_id_and_type() {
        let dir = test_plugin_dir();
        let bad_id = ProviderPresetData {
            id: "bad id!".into(),
            label_key: None,
            label: None,
            provider_type: "openai".into(),
            endpoint: String::new(),
            default_model: String::new(),
            main_models: vec![],
            context_window: None,
            suggested_max_tokens: None,
            needs_secret: None,
            needs_app_id: None,
            console_url: None,
            protocols: None,
            verified_at: None,
            verified_source: None,
        };
        assert!(upsert_provider_preset_at(&dir, bad_id).is_err());

        let bad_type = ProviderPresetData {
            id: "demo".into(),
            label_key: None,
            label: None,
            provider_type: "  ".into(),
            endpoint: String::new(),
            default_model: String::new(),
            main_models: vec![],
            context_window: None,
            suggested_max_tokens: None,
            needs_secret: None,
            needs_app_id: None,
            console_url: None,
            protocols: None,
            verified_at: None,
            verified_source: None,
        };
        assert!(upsert_provider_preset_at(&dir, bad_type).is_err());
        // 失败的调用不产生副作用：数据与版本均未变
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("plugin.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["version"].as_str().unwrap(), "1.2.3");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
