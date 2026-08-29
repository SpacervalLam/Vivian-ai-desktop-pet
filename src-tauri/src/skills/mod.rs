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
use crate::utils::path;

/// 单个技能定义。
#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    pub name: String,
    /// 一句话描述（用于列表/语义匹配）
    pub description: String,
    /// 技能正文（注入 prompt 的能力片段）
    pub body: String,
    /// 作用域：`None` = 全局（所有角色可见）；`Some(char_id)` = 仅该角色可见
    pub scope: Option<String>,
}

impl Skill {
    pub fn global(name: impl Into<String>, description: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            body: body.into(),
            scope: None,
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
            scope: Some(char_id.to_string()),
        }
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

/// 内置风格预设技能名。
///
/// 既是技能来源标识（管理面板区分 builtin），也用于防护：`create_skill` 工具
/// 不允许覆盖这些出厂技能，用户/智能体自建技能须避开该名单。
pub const BUILTIN_SKILL_NAMES: &[&str] = &[
    "default_style",
    "lively_style",
    "healing_style",
    "focused_style",
    "sweet_style",
];

/// 内置技能来源：风格预设（全局）。
fn builtin_skills() -> Vec<Skill> {
    [
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
    .collect()
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

    /// 按名称/描述子串匹配（Top-N）。
    pub fn search(&self, query: &str, n: usize) -> Vec<Skill> {
        let q = query.to_lowercase();
        let mut hits: Vec<Skill> = self
            .inner
            .skills
            .read()
            .iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&q)
                    || s.description.to_lowercase().contains(&q)
                    || s.body.to_lowercase().contains(&q)
            })
            .cloned()
            .collect();
        hits.truncate(n);
        hits
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
            "## 可用技能\n{}\n（如需使用某项技能，调用 use_skill 工具获取其完整指引后照做。\
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

    /// 从目录加载技能（*.md 文件），实现目录化/热加载。
    ///
    /// 文件支持可选 front-matter 头（`name` / `description` / 其余字段忽略），
    /// 正文紧随其后；无 front-matter 时以文件名（去扩展名）为技能名，正文首行为描述。
    /// 同名技能会原子替换（先移除旧再注册），因此热加载只需在目录变更后重复调用。
    /// 返回成功装载的技能名列表。
    pub fn load_from_dir(&self, dir: &std::path::Path) -> Vec<String> {
        let mut loaded = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("[SkillService] 读取技能目录 {} 失败: {}", dir.display(), e);
                return loaded;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("[SkillService] 读取 {} 失败: {}", path.display(), e);
                    continue;
                }
            };
            let (name, description, body) = parse_skill_file(&content, &path);
            if body.trim().is_empty() {
                continue;
            }
            // 原子替换同名技能
            let mut skills = self.inner.skills.write();
            skills.retain(|s| s.name != name);
            skills.push(Skill::global(name.clone(), description, body));
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

/// 目录指纹：`(文件名, 修改时间毫秒)` 列表，用于廉价变更检测。
fn dir_fingerprint(dir: &std::path::Path) -> Vec<(String, u128)> {
    let mut fp: Vec<(String, u128)> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
                .filter_map(|e| {
                    let mtime = e
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis())?;
                    Some((e.file_name().to_string_lossy().into_owned(), mtime))
                })
                .collect()
        })
        .unwrap_or_default();
    fp.sort();
    fp
}

/// 解析一份技能文件：优先提取 `--- name/description ---` front-matter，
/// 否则回退到文件名（技能名）+ 首行（描述）。
pub(crate) fn parse_skill_file(content: &str, path: &std::path::Path) -> (String, String, String) {
    let mut name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("skill")
        .to_string();
    let mut description = String::new();

    let trimmed = content.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            let fm = &rest[..end];
            for line in fm.lines() {
                if let Some(v) = line.strip_prefix("name:") {
                    name = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("description:") {
                    description = v.trim().to_string();
                }
            }
            let body = rest[end + 4..].trim().to_string();
            return (name, description, body);
        }
    }

    if let Some(first) = content.lines().next() {
        let f = first.trim();
        if !f.is_empty() {
            description = f.to_string();
        }
    }
    (name, description, content.to_string())
}