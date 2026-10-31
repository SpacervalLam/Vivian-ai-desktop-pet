//! 技能服务（ctx.skills）—— 可复用微技能的注册与组织
//!
//! 技能是**作用域内可注册、可卸载**的
//! (名称, 描述, 内容) 三元组，角色/插件可按需装载，也可在卸载时用 [`Disposer`]
//! 可逆移除。技能本身不携带执行逻辑，只承载"该做什么/怎么做"的提示词片段，
//! 由上层（prompt 注入 / planner）消费。
//!
//! 内置技能来自现有的风格预设（`load_style_preset`），作为全局技能种子；
//! 动态注册的技能可指定作用域（`Some(char_id)`）实现按角色隔离。

use std::sync::Arc;

use parking_lot::RwLock;
use serde::Serialize;

use crate::cordis::Disposer;
use crate::tools::discovery::{DiscoverableTool, ToolSearchIndex};
use crate::utils::path;

pub mod validate;

/// 单个技能定义。
#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    pub name: String,
    /// 一句话描述（用于列表/语义匹配）
    pub description: String,
    /// 技能正文（注入 prompt 的能力片段）
    pub body: String,
    /// 检索关键词（仅用于 BM25 召回，不出现在 prompt 列表里）
    pub keywords: Vec<String>,
    /// 作用域：`None` = 全局（所有角色可见）；`Some(char_id)` = 仅该角色可见
    pub scope: Option<String>,
    /// 技能来源路径：目录化技能为其目录，单文件技能为其 `.md` 文件；内置技能为 `None`。
    pub path: Option<String>,
    /// 技能目录内的支撑资源相对路径（如 `references/notes.md`）。不随正文加载，
    /// 需要时由模型按路径自取，实现渐进披露。
    pub resources: Vec<String>,
}

impl Skill {
    pub fn global(name: impl Into<String>, description: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            body: body.into(),
            keywords: Vec::new(),
            scope: None,
            path: None,
            resources: Vec::new(),
        }
    }
    pub fn scoped(
        char_id: &str,
        name: impl Into<String>,
        description: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            body: body.into(),
            keywords: Vec::new(),
            scope: Some(char_id.to_string()),
            path: None,
            resources: Vec::new(),
        }
    }
    /// 附加检索关键词（builder）。
    pub fn with_keywords(mut self, keywords: Vec<String>) -> Self {
        self.keywords = keywords;
        self
    }
    /// 记录技能来源路径（builder）。
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
    /// 记录技能支撑资源相对路径（builder）。
    pub fn with_resources(mut self, resources: Vec<String>) -> Self {
        self.resources = resources;
        self
    }
}

struct SkillInner {
    skills: RwLock<Vec<Skill>>,
}

/// 技能服务：注册表 + 可逆注册 + 查询/匹配。
///
/// 通过 `crate::cordis::Service` 的 blanket impl（`Send + Sync + 'static`）即可
/// 注册进运行时，无需手写 impl。
#[derive(Clone)]
pub struct SkillService {
    inner: Arc<SkillInner>,
}

/// 内置风格预设技能名（设置页不展示，仅作风格注入与管理面板区分 builtin 用）。
pub const BUILTIN_STYLE_SKILLS: &[&str] = &[
    "default_style",
    "lively_style",
    "healing_style",
    "focused_style",
    "sweet_style",
];

/// 内置指导型技能名（可在设置页展示，但同样不可被 create_skill / save_skill 覆盖）。
pub const BUILTIN_GUIDE_SKILLS: &[&str] = &["review-agent", "skill-creator"];

/// 全部内置技能名。既是技能来源标识（管理面板区分 builtin），也用于防护：
/// `create_skill` / `save_skill` 不允许覆盖这些出厂技能，自建技能须避开该名单。
pub const BUILTIN_SKILL_NAMES: &[&str] = &[
    "default_style",
    "lively_style",
    "healing_style",
    "focused_style",
    "sweet_style",
    "review-agent",
    "skill-creator",
];

/// 内置指导型技能的嵌入正文（编译期嵌入，随二进制分发，无需额外打包配置）。
const BUILTIN_REVIEW_AGENT_DOC: &str = include_str!("../../builtin_skills/review-agent/SKILL.md");
const BUILTIN_SKILL_CREATOR_DOC: &str = include_str!("../../builtin_skills/skill-creator/SKILL.md");

/// 内置技能来源：风格预设（全局）+ 两枚指导型技能（全局）。
fn builtin_skills() -> Vec<Skill> {
    let mut skills: Vec<Skill> = [
        ("default_style", "默认说话风格", "default"),
        ("lively_style", "活泼风格", "lively"),
        ("healing_style", "治愈风格", "healing"),
        ("focused_style", "专注风格", "focused"),
        ("sweet_style", "甜美风格", "sweet"),
    ]
    .into_iter()
    .map(|(name, desc, key)| {
        let body = crate::persona::prompt_render::load_style_preset(key);
        Skill::global(name, desc, if body.is_empty() { format!("风格：{desc}") } else { body.to_string() })
    })
    .collect();
    // 指导型技能只把方法论正文注入上下文（front-matter 由解析器剥离）。
    for (name, description, doc) in [
        (
            "review-agent",
            "对指定代码改动做只读、缺陷优先的审查，输出可执行发现",
            BUILTIN_REVIEW_AGENT_DOC,
        ),
        (
            "skill-creator",
            "创建或更新技能：范围控制、渐进披露、目录解剖与自检",
            BUILTIN_SKILL_CREATOR_DOC,
        ),
    ] {
        let parsed = parse_skill_doc(doc, name);
        skills.push(Skill::global(name, description, parsed.body).with_keywords(parsed.keywords));
    }
    skills
}

impl SkillService {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(SkillInner {
                skills: RwLock::new(builtin_skills()),
            }),
        })
    }

    /// 注册一个技能，返回可逆 [`Disposer`]（drop/作用域卸载时自动移除）。
    pub fn register(&self, skill: Skill) -> Disposer {
        let name = skill.name.clone();
        self.inner.skills.write().push(skill);
        let inner = Arc::downgrade(&self.inner);
        Disposer::new(move || {
            if let Some(inner) = inner.upgrade() {
                inner
                    .skills
                    .write()
                    .retain(|s| s.name != name);
            }
        })
    }

    /// 原子替换注册：同名技能先移除再写入（幂等，供插件装载/热重载复用）。
    ///
    /// 与 [`SkillService::register`] 的区别：register 允许同名共存（追加），
    /// 本方法保证同名唯一——重复装载同一来源的技能时用新版本覆盖旧版本。
    pub fn replace_or_register(&self, skill: Skill) {
        let name = skill.name.clone();
        let mut skills = self.inner.skills.write();
        skills.retain(|s| s.name != name);
        skills.push(skill);
    }

    /// 按**精确名称**移除技能，返回是否确有移除。
    ///
    /// 与 [`SkillService::remove_by_prefix`] 的区别：本方法只删同名项，不会误伤
    /// 同名前缀的插件技能（如 `foo` 不会连带删掉 `foo/bar`）。
    pub fn remove_exact(&self, name: &str) -> bool {
        let mut skills = self.inner.skills.write();
        let before = skills.len();
        skills.retain(|s| s.name != name);
        skills.len() != before
    }

    /// 按名称前缀移除技能，返回被移除的技能名（供插件卸载撤销命名空间贡献）。
    ///
    /// 插件技能注册名为 `<plugin>/<skill>`；卸载插件时按 `<plugin>/` 前缀
    /// 整组撤销。用户目录技能无 `/`，天然不受影响。
    pub fn remove_by_prefix(&self, prefix: &str) -> Vec<String> {
        let mut skills = self.inner.skills.write();
        let mut removed = Vec::new();
        skills.retain(|s| {
            if s.name.starts_with(prefix) {
                removed.push(s.name.clone());
                false
            } else {
                true
            }
        });
        removed
    }

    /// 列出指定角色可见的技能（全局 + 该角色 scoped）。
    pub fn list_for(&self, char_id: &str) -> Vec<Skill> {
        self.inner
            .skills
            .read()
            .iter()
            .filter(|s| s.scope.is_none() || s.scope.as_deref() == Some(char_id))
            .cloned()
            .collect()
    }

    /// 列出所有技能（含 scoped 标记，供管理面板区分）。
    pub fn list_all(&self) -> Vec<Skill> {
        self.inner.skills.read().iter().cloned().collect()
    }

    /// 按名称精确查找。
    pub fn find(&self, name: &str) -> Option<Skill> {
        self.inner.skills.read().iter().find(|s| s.name == name).cloned()
    }

    /// 按角色可见域 + 查询做 BM25 召回（复用 tools::discovery 索引）。
    ///
    /// 检索字段：name / description / keywords（keywords 权重最高，供 create_skill 写入召回线索）。
    /// 返回按相关性降序的 Top-N 技能；query 为空或无可见技能时返回空。
    pub fn search_skills(&self, char_id: &str, query: &str, n: usize) -> Vec<Skill> {
        let visible = self.list_for(char_id);
        if visible.is_empty() || query.trim().is_empty() {
            return Vec::new();
        }
        let descriptors: Vec<DiscoverableTool> = visible
            .iter()
            .map(|s| DiscoverableTool {
                name: s.name.clone(),
                label: s.name.clone(),
                summary: s.description.clone(),
                description: s.description.clone(),
                search_hint: s.keywords.join(" "),
                schema_keys: Vec::new(),
                category: "skill".to_string(),
                layer: s.scope.clone(),
            })
            .collect();
        let index = ToolSearchIndex::build(descriptors);
        index
            .search(query, n)
            .into_iter()
            .filter_map(|(d, _score)| visible.iter().find(|s| s.name == d.name).cloned())
            .collect()
    }

    /// 生成 prompt 注入片段：列出指定角色可见技能的名称+描述（供 LLM 选择使用）。
    pub fn prompt_section(&self, char_id: &str) -> Option<String> {
        let skills = self.list_for(char_id);
        if skills.is_empty() {
            return None;
        }
        let lines: Vec<String> = skills
            .iter()
            .map(|s| format!("- {}：{}", s.name, s.description))
            .collect();
        Some(format!(
            "## 可用技能\n{}\n（如需使用某项技能，调用 use_skill 工具（给 name）获取其完整指引后照做。\
             若不确定该用哪项技能、或当前列表里没看到合适的，调用 use_skill 给 query 用自然语言描述需求，\
             它会按名称/描述/关键词召回最匹配的技能，再用 use_skill(name) 加载选中的那一项。\
             交付技能草稿前可用 use_skill 的 validate 动作自检形态。\
             当你总结出一套值得复用的做法时，可调用 create_skill 把它沉淀为新技能。）",
            lines.join("\n")
        ))
    }

    /// 默认技能目录：`<用户数据目录>/skills`。缺失时由 [`load_default_dir`] 自动创建。
    pub fn default_dir() -> std::path::PathBuf {
        path::get_user_data_dir().join("skills")
    }

    /// 从默认技能目录加载（目录不存在则先创建）。
    ///
    /// 启动时调用装载目录化技能；热加载只需在目录变更后重复调用本方法（或
    /// [`load_from_dir`]），同名技能会原子替换。
    pub fn load_default_dir(&self) -> Vec<String> {
        let dir = Self::default_dir();
        if !dir.exists() {
            if let Err(e) = std::fs::create_dir_all(&dir) {
                tracing::warn!("[SkillService] 创建技能目录 {} 失败: {}", dir.display(), e);
                return Vec::new();
            }
        }
        self.load_from_dir(&dir)
    }

    /// 从目录加载技能，实现目录化/热加载。
    ///
    /// 两种形态并存：
    /// - **目录化技能**：`<name>/SKILL.md`，可带 `scripts/`、`references/`、`assets/`
    ///   支撑资源；技能名兜底取目录名。
    /// - **单文件技能（legacy）**：根级 `<name>.md`；技能名兜底取文件名。
    ///
    /// 同名冲突时目录化技能胜出并告警。同名技能原子替换（先移除旧再注册），
    /// 因此热加载只需在目录变更后重复调用。返回成功装载的技能名列表。
    pub fn load_from_dir(&self, dir: &std::path::Path) -> Vec<String> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("[SkillService] 读取技能目录 {} 失败: {}", dir.display(), e);
                return Vec::new();
            }
        };
        let mut flat: Vec<Skill> = Vec::new();
        let mut folders: Vec<Skill> = Vec::new();
        for entry in entries.flatten() {
            let entry_name = entry.file_name().to_string_lossy().to_string();
            if entry_name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                let skill_md = path.join("SKILL.md");
                if !skill_md.is_file() {
                    tracing::warn!("[SkillService] 跳过无 SKILL.md 的技能目录 {}", path.display());
                    continue;
                }
                let content = match std::fs::read_to_string(&skill_md) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("[SkillService] 读取 {} 失败: {}", skill_md.display(), e);
                        continue;
                    }
                };
                let parsed = parse_skill_doc(&content, &entry_name);
                if parsed.body.trim().is_empty() {
                    continue;
                }
                let resources = collect_skill_resources(&path);
                folders.push(
                    Skill::global(parsed.name, parsed.description, parsed.body)
                        .with_keywords(parsed.keywords)
                        .with_path(path.display().to_string())
                        .with_resources(resources),
                );
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("[SkillService] 读取 {} 失败: {}", path.display(), e);
                        continue;
                    }
                };
                let fallback = path.file_stem().and_then(|s| s.to_str()).unwrap_or("skill");
                let parsed = parse_skill_doc(&content, fallback);
                if parsed.body.trim().is_empty() {
                    continue;
                }
                flat.push(
                    Skill::global(parsed.name, parsed.description, parsed.body)
                        .with_keywords(parsed.keywords)
                        .with_path(path.display().to_string()),
                );
            }
        }
        let mut by_name: std::collections::BTreeMap<String, Skill> =
            std::collections::BTreeMap::new();
        for skill in flat {
            if by_name.contains_key(&skill.name) {
                tracing::warn!("[SkillService] 单文件技能名 {} 重复，保留先到者", skill.name);
                continue;
            }
            by_name.insert(skill.name.clone(), skill);
        }
        for skill in folders {
            if by_name.contains_key(&skill.name) {
                tracing::warn!(
                    "[SkillService] 技能名 {} 同时存在单文件与技能目录，目录化技能胜出",
                    skill.name
                );
            }
            by_name.insert(skill.name.clone(), skill);
        }
        let mut loaded = Vec::with_capacity(by_name.len());
        for (name, skill) in by_name {
            self.replace_or_register(skill);
            loaded.push(name);
        }
        loaded
    }

    /// 启动后台热刷新：定期对比技能目录指纹（文件名 + mtime），变更时自动重载。
    ///
    /// 不引入 notify 等监听依赖，轮询 stat 对比足够轻量（技能目录文件数个位数）。
    pub fn spawn_hot_reload(self: &Arc<Self>, interval: std::time::Duration) {
        let dir = Self::default_dir();
        let mut last = dir_fingerprint(&dir);
        let svc = Arc::clone(self);
        let expected = interval.as_secs_f64();
        tauri::async_runtime::spawn(async move {
            crate::utils::watchdog::register("skills_hot_reload", expected, None);
            loop {
                tokio::time::sleep(interval).await;
                crate::utils::watchdog::beat("skills_hot_reload");
                let current = dir_fingerprint(&dir);
                if current != last {
                    last = current;
                    let loaded = svc.load_default_dir();
                    tracing::info!("[SkillService] 技能目录变更，热重载完成：{:?}", loaded);
                }
            }
        });
    }
}

/// 目录指纹：`(相对路径, 修改时间毫秒)` 列表，用于廉价变更检测。
///
/// 有界递归（深度 ≤4、条目 ≤2000），跳过符号链接与 `.` 前缀目录；任何 IO 失败
/// 都只返回已收集部分，不中断热重载线程。覆盖 `SKILL.md` 与子目录内资源的编辑。
fn dir_fingerprint(dir: &std::path::Path) -> Vec<(String, u128)> {
    let mut fingerprint = Vec::new();
    collect_fingerprint(dir, "", &mut fingerprint, 0);
    fingerprint.sort();
    fingerprint
}

fn collect_fingerprint(
    dir: &std::path::Path,
    prefix: &str,
    out: &mut Vec<(String, u128)>,
    depth: usize,
) {
    const MAX_DEPTH: usize = 4;
    const MAX_ENTRIES: usize = 2000;
    if depth > MAX_DEPTH || out.len() >= MAX_ENTRIES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_symlink() {
            continue;
        }
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if file_type.is_dir() {
            collect_fingerprint(&entry.path(), &rel, out, depth + 1);
        } else {
            let mtime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis());
            if let Some(mtime) = mtime {
                out.push((rel, mtime));
            }
        }
        if out.len() >= MAX_ENTRIES {
            return;
        }
    }
}

/// 收集技能目录内约定资源子目录（`scripts/`、`references/`、`assets/`）下的相对路径。
///
/// 资源不随技能正文加载，仅作为索引提示，需要时由模型自取。
pub(crate) fn collect_skill_resources(skill_dir: &std::path::Path) -> Vec<String> {
    let mut resources = Vec::new();
    for sub in ["scripts", "references", "assets"] {
        let base = skill_dir.join(sub);
        if base.is_dir() {
            collect_resource_files(&base, sub, &mut resources, 0);
        }
    }
    resources.sort();
    resources
}

fn collect_resource_files(
    dir: &std::path::Path,
    prefix: &str,
    out: &mut Vec<String>,
    depth: usize,
) {
    const MAX_DEPTH: usize = 3;
    const MAX_RESOURCES: usize = 200;
    if depth > MAX_DEPTH || out.len() >= MAX_RESOURCES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_symlink() {
            continue;
        }
        let rel = format!("{prefix}/{name}");
        if file_type.is_dir() {
            collect_resource_files(&entry.path(), &rel, out, depth + 1);
        } else {
            out.push(rel);
        }
        if out.len() >= MAX_RESOURCES {
            return;
        }
    }
}

/// 技能文档解析结果。
pub(crate) struct ParsedSkill {
    pub name: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub body: String,
}

/// 解析一份技能文档。
///
/// front-matter 只认**零缩进**的顶层 `key: value`：缩进行属于嵌套块（如 `metadata:`），
/// 整段忽略；`keywords:` 支持逗号分隔或缩进 `- ` 列表；成对引号会被剥离。
/// `name` 缺失时用 `fallback_name`（目录化技能取目录名、单文件技能取文件名）。
/// 无 front-matter 时首行为描述、全文为正文。
pub(crate) fn parse_skill_doc(content: &str, fallback_name: &str) -> ParsedSkill {
    let mut name = fallback_name.to_string();
    let mut description = String::new();
    let mut keywords: Vec<String> = Vec::new();

    if let Some((front_matter, body)) = split_front_matter(content) {
        parse_front_matter(front_matter, &mut name, &mut description, &mut keywords);
        return ParsedSkill {
            name,
            description,
            keywords,
            body: body.trim().to_string(),
        };
    }

    if let Some(first) = content.lines().next() {
        let f = first.trim();
        if !f.is_empty() {
            description = f.to_string();
        }
    }
    ParsedSkill {
        name,
        description,
        keywords,
        body: content.to_string(),
    }
}

/// 拆分 front-matter 与正文：返回 `(front-matter 内容, 正文)`；无完整头时 `None`。
pub(crate) fn split_front_matter(content: &str) -> Option<(&str, &str)> {
    let rest = content.trim_start().strip_prefix("---")?;
    let close = rest.find("\n---")?;
    Some((&rest[..close], &rest[close + 4..]))
}

/// 取出 front-matter 内容块（不含两侧 `---`）；无完整 front-matter 时返回 `None`。
pub(crate) fn front_matter_block(content: &str) -> Option<&str> {
    split_front_matter(content).map(|(front_matter, _)| front_matter)
}

/// 解析 front-matter：零缩进顶层键生效；缩进行只用于收集 `keywords` 列表项。
fn parse_front_matter(
    front_matter: &str,
    name: &mut String,
    description: &mut String,
    keywords: &mut Vec<String>,
) {
    let mut keywords_list = false;
    for raw in front_matter.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            keywords_list = false;
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if keywords_list {
                if let Some(item) = line.trim().strip_prefix("- ") {
                    push_keyword(item, keywords);
                }
            }
            continue;
        }
        keywords_list = false;
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = unquote(value.trim());
        match key.trim() {
            "name" => *name = value.to_string(),
            "description" => *description = value.to_string(),
            "keywords" => {
                if value.is_empty() {
                    keywords_list = true;
                } else {
                    for token in value.split([',', ' ', '\t']) {
                        push_keyword(token, keywords);
                    }
                }
            }
            _ => {}
        }
    }
}

fn push_keyword(token: &str, out: &mut Vec<String>) {
    let token = unquote(token.trim());
    if !token.is_empty() {
        out.push(token.to_string());
    }
}

/// 剥离成对包裹的引号（`"…"` 或 `'…'`）。
fn unquote(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &value[1..value.len() - 1];
        }
    }
    value
}

/// 兼容旧调用点：以文件名为兜底技能名解析。
///
/// 返回 `(name, description, keywords, body)`。
pub(crate) fn parse_skill_file(
    content: &str,
    path: &std::path::Path,
) -> (String, String, Vec<String>, String) {
    let fallback = path.file_stem().and_then(|s| s.to_str()).unwrap_or("skill");
    let parsed = parse_skill_doc(content, fallback);
    (parsed.name, parsed.description, parsed.keywords, parsed.body)
}

/// 技能名合法性：字母、数字、下划线、连字符或中文，≤64 字符，不含空白与路径分隔符。
pub(crate) fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 64
        && name.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || c == '_'
                || c == '-'
                || ('\u{4e00}'..='\u{9fff}').contains(&c)
        })
}

/// 资源相对路径白名单：仅允许 `references/` 或 `scripts/` 下**一层**内的普通文件名。
///
/// 拒绝反斜杠、盘符、`.`/`..`、多层路径、Windows 保留名与尾随 `.`/空格。
pub(crate) fn is_safe_skill_rel_path(path: &str) -> bool {
    if path.is_empty() || path.contains('\\') || path.contains(':') {
        return false;
    }
    let mut parts = path.split('/');
    let root = parts.next().unwrap_or("");
    if root != "references" && root != "scripts" {
        return false;
    }
    let Some(file) = parts.next() else { return false };
    if parts.next().is_some() {
        return false;
    }
    if file.is_empty() || file == "." || file == ".." || file.ends_with('.') || file.ends_with(' ') {
        return false;
    }
    const RESERVED: &[&str] = &[
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
        "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    let stem = file.split('.').next().unwrap_or("").to_ascii_lowercase();
    !RESERVED.contains(&stem.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parse_skill_doc_handles_fallback_nested_metadata_and_list_keywords() {
        let content = "---\nname: my-skill\ndescription: \"一句话说明\"\nmetadata:\n  short-description: 忽略我\nkeywords:\n  - 部署\n  - '上线'\n---\n\n正文\n";
        let parsed = parse_skill_doc(content, "folder-name");
        assert_eq!(parsed.name, "my-skill");
        assert_eq!(parsed.description, "一句话说明");
        assert_eq!(parsed.keywords, vec!["部署".to_string(), "上线".to_string()]);
        assert_eq!(parsed.body, "正文");
        // name 缺失时用兜底名（目录名 / 文件名）
        let no_name = parse_skill_doc("---\ndescription: 无名称\n---\n\n正文\n", "folder-name");
        assert_eq!(no_name.name, "folder-name");
    }

    #[test]
    fn load_from_dir_loads_folder_and_legacy_skills() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("review-helper");
        std::fs::create_dir_all(folder.join("references")).unwrap();
        std::fs::write(
            folder.join("SKILL.md"),
            "---\nname: review-helper\ndescription: 审查\n---\n\n正文A\n",
        )
        .unwrap();
        std::fs::write(folder.join("references").join("rules.md"), "细则").unwrap();
        std::fs::write(dir.path().join("legacy_skill.md"), "首行描述\n正文B\n").unwrap();

        let svc = SkillService::new();
        let loaded = svc.load_from_dir(dir.path());
        assert!(loaded.contains(&"review-helper".to_string()));
        assert!(loaded.contains(&"legacy_skill".to_string()));
        let folder_skill = svc.find("review-helper").unwrap();
        assert_eq!(folder_skill.resources, vec!["references/rules.md".to_string()]);
        assert!(folder_skill.path.is_some());
        assert_eq!(svc.find("legacy_skill").unwrap().body.trim(), "首行描述\n正文B");
    }

    #[test]
    fn folder_skill_wins_over_same_name_legacy_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("dup.md"),
            "---\nname: dup\ndescription: flat\n---\n\nflat-body\n",
        )
        .unwrap();
        let folder = dir.path().join("dup");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("SKILL.md"),
            "---\nname: dup\ndescription: folder\n---\n\nfolder-body\n",
        )
        .unwrap();
        let svc = SkillService::new();
        svc.load_from_dir(dir.path());
        assert_eq!(svc.find("dup").unwrap().body.trim(), "folder-body");
    }

    #[test]
    fn load_from_dir_skips_dot_dirs_and_dirs_without_skill_md() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join(".staging");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("SKILL.md"), "---\nname: hidden\n---\n\nx\n").unwrap();
        let plain = dir.path().join("not-a-skill");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("readme.txt"), "x").unwrap();

        let svc = SkillService::new();
        let loaded = svc.load_from_dir(dir.path());
        assert!(!loaded.contains(&"hidden".to_string()));
        assert!(!loaded.contains(&"not-a-skill".to_string()));
    }

    #[test]
    fn fingerprint_detects_edit_inside_skill_subfolder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("s1");
        std::fs::create_dir_all(folder.join("references")).unwrap();
        std::fs::write(folder.join("SKILL.md"), "a").unwrap();
        let before = dir_fingerprint(dir.path());
        std::fs::write(folder.join("references").join("new.md"), "b").unwrap();
        let after = dir_fingerprint(dir.path());
        assert_ne!(before, after, "子目录内新增资源应改变指纹");
    }

    #[test]
    fn resource_paths_are_restricted_to_one_level_under_scripts_or_references() {
        for ok in ["references/a.md", "scripts/run.py"] {
            assert!(is_safe_skill_rel_path(ok), "{ok}");
        }
        for bad in ["../x", "/x", "assets/a.png", "a/b/c.txt", "references/", "scripts/x/y", "con.txt"] {
            assert!(!is_safe_skill_rel_path(bad), "{bad}");
        }
    }

    #[test]
    fn parse_extracts_keywords_from_front_matter() {
        let content = "---\nname: deploy_helper\ndescription: 部署流程\nkeywords: 部署, 上线 deploy 发布\n---\n\n正文第一行\n正文第二行\n";
        let (name, desc, keywords, body) = parse_skill_file(content, Path::new("ignored.md"));
        assert_eq!(name, "deploy_helper");
        assert_eq!(desc, "部署流程");
        assert!(keywords.contains(&"部署".to_string()));
        assert!(keywords.contains(&"deploy".to_string()));
        assert!(keywords.contains(&"发布".to_string()));
        assert!(body.starts_with("正文第一行"));
    }

    #[test]
    fn parse_without_front_matter_falls_back() {
        let content = "这是首行描述\n更多内容\n";
        let (name, desc, keywords, body) = parse_skill_file(content, Path::new("my_skill.md"));
        assert_eq!(name, "my_skill");
        assert_eq!(desc, "这是首行描述");
        assert!(keywords.is_empty());
        assert!(body.contains("更多内容"));
    }

    #[test]
    fn search_skills_recalls_by_chinese_query() {
        let svc = SkillService::new();
        svc.replace_or_register(
            Skill::global("provider_preset_fix", "核对并更新模型供应商预设", "步骤……")
                .with_keywords(vec![
                    "供应商".into(),
                    "预设".into(),
                    "provider".into(),
                    "核对".into(),
                ]),
        );
        svc.replace_or_register(
            Skill::global("diary_style", "写日记的口吻与结构", "步骤……")
                .with_keywords(vec!["日记".into(), "diary".into()]),
        );

        let hits = svc.search_skills("", "更新供应商预设", 5);
        assert!(!hits.is_empty(), "中文自然语言查询应召回技能");
        assert_eq!(
            hits[0].name, "provider_preset_fix",
            "供应商预设技能应排在首位，实际：{:?}",
            hits.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }
}