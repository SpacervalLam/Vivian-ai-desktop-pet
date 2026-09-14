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
}

impl Skill {
    pub fn global(name: impl Into<String>, description: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            body: body.into(),
            keywords: Vec::new(),
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
            keywords: Vec::new(),
            scope: Some(char_id.to_string()),
        }
    }
    /// 附加检索关键词（builder）。
    pub fn with_keywords(mut self, keywords: Vec<String>) -> Self {
        self.keywords = keywords;
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
            "## 可用技能\n{}\n（如需使用某项技能，调用 use_skill 工具获取其完整指引后照做。\
             若不确定该用哪项技能、或当前列表里没看到合适的，调用 search_skill 用自然语言描述需求，\
             它会按名称/描述/关键词召回最匹配的技能，再用 use_skill 加载选中的那一项。\
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
            let (name, description, keywords, body) = parse_skill_file(&content, &path);
            if body.trim().is_empty() {
                continue;
            }
            // 原子替换同名技能
            let mut skills = self.inner.skills.write();
            skills.retain(|s| s.name != name);
            skills.push(Skill::global(name.clone(), description, body).with_keywords(keywords));
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

/// 解析一份技能文件：优先提取 `--- name/description/keywords ---` front-matter，
/// 否则回退到文件名（技能名）+ 首行（描述）。
/// 返回 `(name, description, keywords, body)`；keywords 按逗号或空白切分。
pub(crate) fn parse_skill_file(
    content: &str,
    path: &std::path::Path,
) -> (String, String, Vec<String>, String) {
    let mut name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("skill")
        .to_string();
    let mut description = String::new();
    let mut keywords: Vec<String> = Vec::new();

    let trimmed = content.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            let fm = &rest[..end];
            for line in fm.lines() {
                if let Some(v) = line.strip_prefix("name:") {
                    name = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("description:") {
                    description = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("keywords:") {
                    keywords = v
                        .split([',', ' ', '\t'])
                        .map(|t| t.trim().to_string())
                        .filter(|t| !t.is_empty())
                        .collect();
                }
            }
            let body = rest[end + 4..].trim().to_string();
            return (name, description, keywords, body);
        }
    }

    if let Some(first) = content.lines().next() {
        let f = first.trim();
        if !f.is_empty() {
            description = f.to_string();
        }
    }
    (name, description, keywords, content.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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