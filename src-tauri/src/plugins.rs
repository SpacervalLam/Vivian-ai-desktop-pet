//! 插件贡献点体系 —— 启动时从 `<用户数据目录>/plugins/` 装载插件声明的能力
//!
//! 支持两类贡献点：
//! - **skills**：插件目录下的 *.md 技能文件，以 `<plugin>/<skill>` 命名注册进
//!   SkillService（命名空间隔离，不与用户技能冲突），对所有角色可见
//! - **mcp_servers**：stdio MCP server 声明，按 id 去重合并进 servers.json
//!   （用户已有的同 id 配置优先，插件不覆盖），由 init_all 统一连接
//!
//! 插件格式：`plugins/<name>/plugin.json`
//! ```json
//! {
//!   "name": "my-plugin",
//!   "version": "1.0.0",
//!   "description": "示例插件",
//!   "skills": ["skills/*.md"],
//!   "mcp_servers": [{ "id": "fs", "name": "文件系统", "command": "npx",
//!                     "args": ["-y", "@modelcontextprotocol/server-filesystem", "."] }]
//! }
//! ```
//!
//! 装载语义（用户/手动配置 > 插件声明）：
//! - MCP id 冲突：保留现有配置，跳过插件声明
//! - 技能命名空间化：`<plugin>/<skill_name>`，与用户目录技能天然无冲突，
//!   同名插件技能重复装载时原子替换（幂等，可热重载复用）

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::skills::SkillService;
use crate::tools::mcp::McpServerConfig;
use crate::tools::McpManager;

/// 插件清单（plugins/<name>/plugin.json）
#[derive(Debug, Clone, Deserialize)]
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
    /// MCP server 声明（按 id 去重合并）
    #[serde(default)]
    mcp_servers: Vec<McpServerConfig>,
}

/// 插件清单条目（设置窗口「插件」页只读盘点用）
#[derive(Debug, Clone, Serialize)]
pub struct PluginInventoryEntry {
    /// 插件名（命名空间前缀）
    pub name: String,
    pub version: String,
    pub description: String,
    /// 贡献的技能（命名空间名，如 `my-plugin/skill_a`）
    pub skills: Vec<String>,
    /// 贡献的 MCP server id
    pub mcp_servers: Vec<String>,
    /// "loaded"（正常装载）或 "skipped"（清单缺失/解析失败/命名非法）
    pub status: String,
    /// 跳过原因（仅 status="skipped" 时非空）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 插件目录路径（便于用户定位/编辑）
    pub dir: String,
}

/// 插件装载结果
#[derive(Debug, Default, Clone)]
pub struct PluginLoadReport {
    /// 成功装载的插件名
    pub plugins: Vec<String>,
    /// 注册的技能（命名空间名）
    pub skills: Vec<String>,
    /// 合并的 MCP server id
    pub mcp_servers: Vec<String>,
    /// 跳过/失败的清单（诊断用）
    pub skipped: Vec<String>,
}

/// 插件根目录：`<用户数据目录>/plugins`
pub fn plugins_dir() -> PathBuf {
    crate::utils::path::get_user_data_dir().join("plugins")
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

/// 展开插件声明的技能 glob，返回 `(命名空间名, 描述, 正文)` 三元组。
fn expanded_skills(
    pdir: &Path,
    plugin_name: &str,
    patterns: &[String],
) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for pattern in patterns {
        for skill_path in glob_files(pdir, pattern) {
            let Ok(text) = std::fs::read_to_string(&skill_path) else {
                continue;
            };
            let (skill_name, description, body) = crate::skills::parse_skill_file(&text, &skill_path);
            if body.trim().is_empty() {
                continue;
            }
            out.push((
                format!("{plugin_name}/{skill_name}"),
                description,
                body,
            ));
        }
    }
    out
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
            Ok((name, manifest)) => out.push(PluginInventoryEntry {
                skills: expanded_skills(&pdir, &name, &manifest.skills)
                    .into_iter()
                    .map(|(n, _, _)| n)
                    .collect(),
                mcp_servers: manifest
                    .mcp_servers
                    .iter()
                    .map(|s| s.id.clone())
                    .collect(),
                name: name.clone(),
                version: manifest.version,
                description: manifest.description,
                status: "loaded".into(),
                reason: None,
                dir: dir_str,
            }),
            Err((name, reason)) => out.push(PluginInventoryEntry {
                name,
                version: String::new(),
                description: String::new(),
                skills: Vec::new(),
                mcp_servers: Vec::new(),
                status: "skipped".into(),
                reason: Some(reason),
                dir: dir_str,
            }),
        }
    }
    out
}

/// 装载全部插件（幂等：技能原子替换、MCP 按 id 去重）
pub fn load_all(skill_service: &SkillService, mcp_manager: &McpManager) -> PluginLoadReport {
    let dir = plugins_dir();
    if !dir.exists() {
        // 首次运行创建目录，用户放入插件即生效（下次启动装载）
        let _ = std::fs::create_dir_all(&dir);
        return PluginLoadReport::default();
    }
    let mut report = PluginLoadReport::default();
    let mut mcp_incoming: Vec<McpServerConfig> = Vec::new();

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

        // 贡献点 1：技能（glob 展开相对插件目录，命名空间注册，同名原子替换幂等）
        for (skill_name, description, body) in expanded_skills(&pdir, &plugin_name, &manifest.skills) {
            skill_service.replace_or_register(crate::skills::Skill::global(
                skill_name.clone(),
                description,
                body,
            ));
            report.skills.push(skill_name);
        }

        // 贡献点 2：MCP servers（先收集，统一去重合并）
        mcp_incoming.extend(manifest.mcp_servers.iter().cloned());
        report.plugins.push(plugin_name.clone());
        tracing::info!(
            "[Plugins] 装载插件 {} v{}（{}）：{} 条技能声明、{} 个 MCP server 声明",
            plugin_name,
            manifest.version,
            manifest.description,
            manifest.skills.len(),
            manifest.mcp_servers.len()
        );
    }

    // MCP 统一合并（现有 id 优先，插件不覆盖用户配置）
    report.mcp_servers = mcp_manager.merge_plugin_servers(&mcp_incoming);
    report
}

/// 简化 glob：支持 `*.md` 后缀匹配与目录递归两种模式（够用即可，不引依赖）
///
/// - `skills/*.md` → 插件目录下 skills/ 一层内的 .md 文件
/// - `skills` 或 `skills/**` → 递归收集该子目录全部 .md
fn glob_files(base: &Path, pattern: &str) -> Vec<PathBuf> {
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
    if !root.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    collect_md(&root, recursive, &mut out);
    out.sort();
    out
}

fn collect_md(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                collect_md(&path, recursive, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
