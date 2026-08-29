//! 系统工具 - open_application, close_application, take_screenshot, screenshot_analyze

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::Manager;

use crate::providers::base::LLMRequest;
use crate::state::AppState;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};
use crate::types::response::{ChatMessage, MessageImage};
use crate::utils::process::silent_command;

/// 全局 AppHandle（由 lib.rs setup 注入，用于读取 AppState 中的 ModelRouter / Config）
static APP_HANDLE: Lazy<RwLock<Option<tauri::AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用一次）
pub fn set_app_handle(handle: tauri::AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// open_application 工具 - 启动应用程序
///
/// 支持多种解析路径（按优先级回退）：
/// 1. 路径形式输入（含 `\` / `/` / 盘符前缀）→ 直接 spawn
/// 2. Application Registry 缓存（历史成功记录）
/// 3. App Paths 注册表（Windows "运行" 框机制，直接映射 exe→路径）
/// 4. `where.exe` 在 PATH 中查找（自动尝试补 `.exe`）
/// 5. 扫描常见安装目录（Program Files / Program Files (x86) / %LOCALAPPDATA%\Programs，深度 3）
/// 6. 扫描 Start Menu `.lnk` 快捷方式（增强版：匹配 .lnk 名 + target exe 名）
/// 7. Windows 注册表 Uninstall 键
/// 8. UWP 应用：`Get-StartApps` 解析 AppID，用 `Start-Process shell:AppsFolder:<AppID>` 启动
pub struct OpenApplicationTool;

impl OpenApplicationTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OpenApplicationTool {
    fn default() -> Self {
        Self::new()
    }
}

/// 清空应用解析缓存（恢复出厂设置时调用）
#[cfg(target_os = "windows")]
pub fn clear_app_registry() {
    windows_resolver::clear_registry();
}

/// 清空应用解析缓存（非 Windows 平台空操作）
#[cfg(not(target_os = "windows"))]
pub fn clear_app_registry() {}

#[cfg(target_os = "windows")]
mod windows_resolver {
    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;

    use crate::utils::process::silent_command;

    const DANGEROUS_EXES: &[&str] = &[
        "cmd.exe", "powershell.exe", "pwsh.exe", "wscript.exe", "cscript.exe",
        "mshta.exe", "regsvr32.exe", "rundll32.exe", "regedit.exe", "reg.exe",
        "net.exe", "net1.exe", "schtasks.exe", "wmic.exe", "bitsadmin.exe",
        "certutil.exe", "msiexec.exe",
    ];

    /// 中文应用名 → 英文可执行文件名别名表（弥补 LLM 传中文名或英文名时的匹配缺口）
    const APP_ALIASES: &[(&str, &str)] = &[
        // 腾讯系
        ("qq音乐", "qqmusic"), ("qqmusic", "qqmusic"),
        ("微信", "wechat"), ("wechat", "wechat"),
        ("qq", "qq"), ("腾讯qq", "qq"),
        ("腾讯视频", "qqlive"), ("qqlive", "qqlive"),
        ("腾讯会议", "wemeetapp"), ("wemeet", "wemeetapp"),
        ("企业微信", "wxwork"), ("wxwork", "wxwork"),
        ("qq浏览器", "qqbrowser"), ("qqbrowser", "qqbrowser"),
        // 浏览器
        ("谷歌浏览器", "chrome"), ("chrome", "chrome"),
        ("火狐", "firefox"), ("firefox", "firefox"),
        ("edge", "msedge"), ("微软edge", "msedge"),
        // 办公 / 生产力
        ("钉钉", "dingtalk"), ("dingtalk", "dingtalk"),
        ("飞书", "feishu"), ("feishu", "feishu"), ("lark", "feishu"),
        ("wps", "wps"),
        ("notion", "notion"),
        ("obsidian", "obsidian"),
        // 音乐 / 娱乐
        ("网易云音乐", "cloudmusic"), ("cloudmusic", "cloudmusic"),
        ("spotify", "spotify"),
        ("哔哩哔哩", "bilibili"), ("b站", "bilibili"),
        // 开发工具
        ("vscode", "code"), ("visual studio code", "code"),
        ("visual studio", "devenv"), ("vs2022", "devenv"),
        ("idea", "idea64"), ("intellij", "idea64"),
        ("pycharm", "pycharm64"), ("webstorm", "webstorm64"),
        ("记事本", "notepad"), ("计算器", "calc"), ("画图", "mspaint"),
        ("资源管理器", "explorer"), ("任务管理器", "taskmgr"),
        ("cmd", "cmd"), ("powershell", "powershell"),
        // 通讯
        ("discord", "discord"),
        ("telegram", "telegram"),
        ("line", "linemessenger"),
        // 游戏平台
        ("steam", "steam"),
        ("wegame", "wegame"),
        ("epic", "epicgameslauncher"), ("epic games", "epicgameslauncher"),
        // 游戏
        ("原神", "yuanshen"), ("genshin", "genshinimpact"), ("genshin impact", "genshinimpact"),
        ("崩坏星穹铁道", "startrail"), ("星穹铁道", "startrail"),
        ("明日方舟", "arknights"),
        ("pubg", "tslgame"),
        ("永劫无间", "naraka"),
        // 实用工具
        ("winrar", "winrar"),
        ("7zip", "7zfm"), ("7-zip", "7zfm"),
        ("迅雷", "thunder"), ("thunder", "thunder"),
    ];

    /// 判断两个字符串是否存在合理的包含关系（带长度守卫）
    ///
    /// 要求较短串长度 >= 较长串的 60%，防止 "qq"(2) 误匹配 "qq音乐"(4)
    /// 而 "code"(4) 仍可匹配 "vscode"(6)（比例 0.67）
    fn is_fuzzy_superset(longer: &str, shorter: &str) -> bool {
        if shorter.is_empty() || longer.is_empty() {
            return false;
        }
        if shorter == longer {
            return true;
        }
        let ratio = shorter.chars().count() as f64 / longer.chars().count() as f64;
        ratio >= 0.6 && longer.contains(shorter)
    }

    fn is_dangerous_exe_name(name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        DANGEROUS_EXES.iter().any(|d| lower == *d || lower.ends_with(&format!("\\{}", d)) || lower.ends_with(&format!("/{}", d)))
    }

    /// 解析后的可启动目标
    #[derive(Debug, Clone)]
    pub enum AppTarget {
        /// 普通可执行文件（.exe）
        Exe(PathBuf),
        /// Start Menu 快捷方式（.lnk）
        Lnk(PathBuf),
        /// UWP 应用 ID（形如 `Microsoft.WindowsCalculator_8wekyb3d8bbwe!App`）
        Uwp(String),
    }

    impl AppTarget {
        pub fn describe(&self) -> String {
            match self {
                AppTarget::Exe(p) => format!("exe:{}", p.display()),
                AppTarget::Lnk(p) => format!("lnk:{}", p.display()),
                AppTarget::Uwp(appid) => format!("uwp:{}", appid),
            }
        }

        pub fn type_tag(&self) -> &'static str {
            match self {
                AppTarget::Exe(_) => "exe",
                AppTarget::Lnk(_) => "lnk",
                AppTarget::Uwp(_) => "uwp",
            }
        }
    }

    // ── 评分体系 ──

    /// 名称匹配质量
    #[derive(Debug, Clone, Copy)]
    enum MatchQuality {
        Exact,
        Fuzzy,
    }

    /// 带评分的解析候选
    #[derive(Debug, Clone)]
    pub struct ResolvedCandidate {
        pub target: AppTarget,
        /// 置信度 [0.0, 1.0+]，越高越确定
        pub score: f64,
        /// 发现来源标签
        pub source: &'static str,
        /// 路径/ID 描述
        pub path_desc: String,
    }

    /// auto-launch 最低置信度阈值
    const AUTO_LAUNCH_MIN_SCORE: f64 = 0.75;
    /// 最高分与次高分之间的最小差距
    const AUTO_LAUNCH_MIN_GAP: f64 = 0.12;

    /// 判断是否应该自动启动（score 达标 + gap 达标 + 无失败记录）
    pub fn should_auto_launch(candidates: &[ResolvedCandidate]) -> bool {
        if candidates.is_empty() {
            return false;
        }
        let top = &candidates[0];
        let second_score = candidates.get(1).map(|c| c.score).unwrap_or(0.0);
        let gap = top.score - second_score;
        if top.score < AUTO_LAUNCH_MIN_SCORE || gap < AUTO_LAUNCH_MIN_GAP {
            return false;
        }
        // 检查缓存中是否有失败记录
        let reg = load_registry();
        !reg.entries.values().any(|e| e.path == top.path_desc && e.failure_count > 0)
    }

    /// 计算候选置信度：层基础分 × 名称匹配系数
    fn score_candidate(layer: &str, quality: MatchQuality) -> f64 {
        let base = match layer {
            "cache" => 0.95,
            "app_paths" => 0.85,
            "path" => 0.75,
            "common_dirs" => 0.70,
            "shortcut" => 0.72,
            "registry" => 0.68,
            "uwp" => 0.60,
            _ => 0.50,
        };
        let mult = match quality {
            MatchQuality::Exact => 1.0,
            MatchQuality::Fuzzy => 0.6,
        };
        base * mult
    }

    /// 从路径文件名推断匹配质量
    fn infer_match_quality(path: &std::path::Path, candidate_name: &str) -> MatchQuality {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if stem.to_lowercase() == candidate_name.to_lowercase() {
                return MatchQuality::Exact;
            }
        }
        MatchQuality::Fuzzy
    }

    // ── Application Registry（应用记忆缓存）──

    /// 单个缓存条目
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct AppRegistryEntry {
        /// 解析后的路径 / AppID
        path: String,
        /// 目标类型: "exe" / "lnk" / "uwp"
        target_type: String,
        /// 已知别名（小写，去空格）
        aliases: Vec<String>,
        /// 最后成功时间（Unix 秒）
        last_success: i64,
        /// 成功启动次数
        #[serde(default)]
        success_count: u32,
        /// 失败次数
        #[serde(default)]
        failure_count: u32,
        /// 最后使用时间（Unix 秒）
        #[serde(default)]
        last_used: i64,
    }

    /// 应用注册表（磁盘持久化）
    #[derive(Debug, Clone, Serialize, Deserialize, Default)]
    struct AppRegistry {
        /// key = 规范化后的应用名（小写、去空格）
        entries: HashMap<String, AppRegistryEntry>,
    }

    fn registry_path() -> PathBuf {
        crate::utils::path::get_user_data_dir().join("app_registry.json")
    }

    fn load_registry() -> AppRegistry {
        let path = registry_path();
        crate::utils::fs::load_json_or_backup(&path).unwrap_or_default()
    }

    fn save_registry(reg: &AppRegistry) {
        let path = registry_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(reg) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// 规范化应用名：小写 + 去空格 + 去 .exe 后缀
    fn normalize_name(name: &str) -> String {
        let n = name.to_lowercase();
        let n = n.trim().to_string();
        if n.ends_with(".exe") {
            n[..n.len() - 4].to_string()
        } else {
            n
        }
    }

    /// 从缓存查找（命中时更新使用统计）
    fn find_in_cache(name: &str) -> Option<AppTarget> {
        let mut reg = load_registry();
        let norm = normalize_name(name);
        let mut found_key: Option<String> = None;
        // 精确匹配 key
        if reg.entries.contains_key(&norm) {
            found_key = Some(norm);
        } else {
            // 别名匹配
            for (key, entry) in &reg.entries {
                if entry.aliases.iter().any(|a| *a == norm) {
                    found_key = Some(key.clone());
                    break;
                }
            }
        }
        if let Some(key) = found_key {
            let target = {
                if let Some(entry) = reg.entries.get_mut(&key) {
                    entry.success_count = entry.success_count.saturating_add(1);
                    entry.last_used = chrono::Utc::now().timestamp();
                    entry_to_target(entry)
                } else {
                    None
                }
            };
            if target.is_some() {
                save_registry(&reg);
            }
            return target;
        }
        None
    }

    fn entry_to_target(entry: &AppRegistryEntry) -> Option<AppTarget> {
        match entry.target_type.as_str() {
            "exe" => {
                let p = PathBuf::from(&entry.path);
                if p.exists() { Some(AppTarget::Exe(p)) } else { None }
            }
            "lnk" => {
                let p = PathBuf::from(&entry.path);
                if p.exists() { Some(AppTarget::Lnk(p)) } else { None }
            }
            "uwp" => Some(AppTarget::Uwp(entry.path.clone())),
            _ => None,
        }
    }

    /// 写入缓存（成功解析后调用）
    fn cache_result(name: &str, target: &AppTarget) {
        let mut reg = load_registry();
        let norm = normalize_name(name);
        let (path, target_type) = match target {
            AppTarget::Exe(p) => (p.display().to_string(), "exe".to_string()),
            AppTarget::Lnk(p) => (p.display().to_string(), "lnk".to_string()),
            AppTarget::Uwp(id) => (id.clone(), "uwp".to_string()),
        };
        // 收集别名
        let mut aliases = vec![norm.clone()];
        for &(cn, en) in APP_ALIASES {
            let cn_n = normalize_name(cn);
            let en_n = normalize_name(en);
            if cn_n == norm || en_n == norm {
                if !aliases.contains(&cn_n) { aliases.push(cn_n); }
                if !aliases.contains(&en_n) { aliases.push(en_n); }
            }
        }
        let now = chrono::Utc::now().timestamp();
        reg.entries.insert(norm, AppRegistryEntry {
            path,
            target_type,
            aliases,
            last_success: now,
            success_count: 0,
            failure_count: 0,
            last_used: now,
        });
        save_registry(&reg);
    }

    /// 记录启动成功（更新缓存中的统计）
    pub fn record_success(target: &AppTarget) {
        let mut reg = load_registry();
        let target_path = match target {
            AppTarget::Exe(p) | AppTarget::Lnk(p) => p.display().to_string(),
            AppTarget::Uwp(id) => id.clone(),
        };
        let mut found_key: Option<String> = None;
        for (key, entry) in &reg.entries {
            if entry.path == target_path {
                found_key = Some(key.clone());
                break;
            }
        }
        if let Some(key) = found_key {
            let mut updated = false;
            {
                if let Some(entry) = reg.entries.get_mut(&key) {
                    entry.success_count = entry.success_count.saturating_add(1);
                    entry.last_used = chrono::Utc::now().timestamp();
                    entry.last_success = entry.last_used;
                    updated = true;
                }
            }
            if updated {
                save_registry(&reg);
            }
        }
    }

    /// 记录启动失败（标记为不可靠）
    pub fn record_failure(target: &AppTarget) {
        let mut reg = load_registry();
        let target_path = match target {
            AppTarget::Exe(p) | AppTarget::Lnk(p) => p.display().to_string(),
            AppTarget::Uwp(id) => id.clone(),
        };
        let mut found_key: Option<String> = None;
        for (key, entry) in &reg.entries {
            if entry.path == target_path {
                found_key = Some(key.clone());
                break;
            }
        }
        if let Some(key) = found_key {
            let mut updated = false;
            {
                if let Some(entry) = reg.entries.get_mut(&key) {
                    entry.failure_count = entry.failure_count.saturating_add(1);
                    entry.last_used = chrono::Utc::now().timestamp();
                    updated = true;
                }
            }
            if updated {
                save_registry(&reg);
            }
        }
    }

    /// 清空应用解析缓存（恢复出厂设置时调用）
    pub fn clear_registry() {
        let path = registry_path();
        match std::fs::remove_file(&path) {
            Ok(()) => tracing::info!("[factory_reset] 应用解析缓存已清空: {}", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!("[factory_reset] 应用解析缓存不存在，跳过清空");
            }
            Err(e) => tracing::warn!("[factory_reset] 清空应用解析缓存失败: {e}"),
        }
    }

    // ── 别名展开 ──

    /// 将输入名通过别名表展开为多个候选名
    fn expand_aliases(name: &str) -> Vec<String> {
        let norm = normalize_name(name);
        let mut candidates = vec![norm.clone()];
        for &(cn, en) in APP_ALIASES {
            let cn_n = normalize_name(cn);
            let en_n = normalize_name(en);
            if cn_n == norm && !candidates.contains(&en_n) {
                candidates.push(en_n.clone());
            }
            if en_n == norm && !candidates.contains(&cn_n) {
                candidates.push(cn_n);
            }
        }
        candidates
    }

    // ── 主入口 ──

    /// 判断输入是否为路径形式（含分隔符或盘符前缀）
    pub fn is_path_like(s: &str) -> bool {
        s.contains('\\') || s.contains('/') || (s.len() >= 2 && s.as_bytes()[1] == b':')
    }

    /// 主入口：把应用名解析为带评分的候选列表
    ///
    /// 查找顺序（所有层均执行，不提前返回）：
    /// 0. Application Registry 缓存（最快路径，历史成功记录）
    /// 0.5. App Paths 注册表（Windows "运行" 框机制，exe→路径直接映射）
    /// 1. where.exe 查 PATH
    /// 2. 常见安装目录扫描（depth=3）
    /// 3. Start Menu 快捷方式（增强版：匹配 .lnk 名 + target exe 名）
    /// 4. Windows 注册表 Uninstall 键
    /// 5. UWP 应用
    pub fn resolve(application: &str) -> Result<Vec<ResolvedCandidate>, &'static str> {
        if is_path_like(application) {
            let path = PathBuf::from(application);
            if let Some(file_name) = path.file_name().and_then(|f| f.to_str()) {
                if is_dangerous_exe_name(file_name) {
                    return Err("出于安全考虑，禁止直接启动 cmd/powershell/regedit 等系统程序");
                }
            }
            return Ok(vec![ResolvedCandidate {
                target: AppTarget::Exe(path),
                score: 1.0,
                source: "direct_path",
                path_desc: application.to_string(),
            }]);
        }
        let lower = application.to_ascii_lowercase();
        if is_dangerous_exe_name(&lower) || lower == "cmd" || lower == "powershell" || lower == "pwsh" {
            return Err("出于安全考虑，禁止直接启动 cmd/powershell/regedit 等系统程序");
        }

        let name_candidates = expand_aliases(application);
        let mut results: Vec<ResolvedCandidate> = Vec::new();

        // Layer 0: Application Registry 缓存（最快路径）
        for c in &name_candidates {
            if let Some(t) = find_in_cache(c) {
                let desc = t.describe();
                results.push(ResolvedCandidate {
                    target: t,
                    score: score_candidate("cache", MatchQuality::Exact),
                    source: "cache",
                    path_desc: desc,
                });
            }
        }

        // Layer 0.5: App Paths 注册表（Windows "运行" 框机制，极快）
        for c in &name_candidates {
            if let Some(p) = find_via_app_paths(c) {
                if let Some(file_name) = p.file_name().and_then(|f| f.to_str()) {
                    if is_dangerous_exe_name(file_name) {
                        continue;
                    }
                }
                let quality = infer_match_quality(&p, c);
                let desc = p.display().to_string();
                let target = AppTarget::Exe(p);
                cache_result(application, &target);
                results.push(ResolvedCandidate {
                    target,
                    score: score_candidate("app_paths", quality),
                    source: "app_paths",
                    path_desc: desc,
                });
            }
        }

        // Layer 1: where.exe 查 PATH
        for c in &name_candidates {
            if let Some(p) = find_via_where(c) {
                if let Some(file_name) = p.file_name().and_then(|f| f.to_str()) {
                    if is_dangerous_exe_name(file_name) {
                        continue;
                    }
                }
                let quality = infer_match_quality(&p, c);
                let desc = p.display().to_string();
                let target = AppTarget::Exe(p);
                cache_result(application, &target);
                results.push(ResolvedCandidate {
                    target,
                    score: score_candidate("path", quality),
                    source: "path",
                    path_desc: desc,
                });
            }
        }

        // Layer 2: 扫描常见安装目录
        for c in &name_candidates {
            if let Some(p) = find_in_common_dirs(c) {
                let quality = infer_match_quality(&p, c);
                let desc = p.display().to_string();
                let target = AppTarget::Exe(p);
                cache_result(application, &target);
                results.push(ResolvedCandidate {
                    target,
                    score: score_candidate("common_dirs", quality),
                    source: "common_dirs",
                    path_desc: desc,
                });
            }
        }

        // Layer 3: Start Menu 快捷方式（增强版）
        for c in &name_candidates {
            if let Some(p) = find_shortcut_enhanced(c) {
                let quality = infer_match_quality(&p, c);
                let desc = p.display().to_string();
                let target = AppTarget::Lnk(p);
                cache_result(application, &target);
                results.push(ResolvedCandidate {
                    target,
                    score: score_candidate("shortcut", quality),
                    source: "shortcut",
                    path_desc: desc,
                });
            }
        }

        // Layer 4: Windows 注册表 Uninstall 键
        for c in &name_candidates {
            if let Some(p) = find_via_registry(c) {
                let quality = infer_match_quality(&p, c);
                let desc = p.display().to_string();
                let target = AppTarget::Exe(p);
                cache_result(application, &target);
                results.push(ResolvedCandidate {
                    target,
                    score: score_candidate("registry", quality),
                    source: "registry",
                    path_desc: desc,
                });
            }
        }

        // Layer 5: UWP 应用
        for c in &name_candidates {
            if let Some(appid) = find_uwp_app(c) {
                let app_lower = appid.to_lowercase();
                let c_lower = c.to_lowercase();
                let quality = if app_lower.contains(&c_lower) || c_lower.contains(&app_lower) {
                    MatchQuality::Exact
                } else {
                    MatchQuality::Fuzzy
                };
                let desc = appid.clone();
                let target = AppTarget::Uwp(appid);
                cache_result(application, &target);
                results.push(ResolvedCandidate {
                    target,
                    score: score_candidate("uwp", quality),
                    source: "uwp",
                    path_desc: desc,
                });
            }
        }

        // 去重（同一路径只保留最高分）
        let mut seen = HashSet::new();
        let mut deduped: Vec<ResolvedCandidate> = Vec::new();
        for r in results {
            let key = r.path_desc.clone();
            if !seen.contains(&key) {
                seen.insert(key);
                deduped.push(r);
            }
        }

        // 应用历史记录奖励
        let reg = load_registry();
        for r in &mut deduped {
            for entry in reg.entries.values() {
                if entry.path == r.path_desc {
                    let bonus = (entry.success_count as f64 + 1.0).ln() * 0.05
                        - (entry.failure_count as f64 + 1.0).ln() * 0.10;
                    r.score = (r.score + bonus).max(0.0);
                    break;
                }
            }
        }

        // 按 score 降序排列
        deduped.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        let top_score = deduped.first().map(|r| r.score).unwrap_or(0.0);
        tracing::info!(
            target: "app_resolver",
            query = %application,
            candidates_found = deduped.len(),
            top_score = top_score,
            top_source = deduped.first().map(|r| r.source).unwrap_or("none"),
            "resolver_pipeline_complete"
        );

        Ok(deduped)
    }

    /// 用 `where.exe` 在 PATH 中查找可执行文件（自动尝试补 `.exe`）
    fn find_via_where(name: &str) -> Option<PathBuf> {
        let name_lower = name.to_lowercase();
        let candidates: Vec<String> = if name_lower.ends_with(".exe") {
            vec![name.to_string()]
        } else {
            vec![format!("{}.exe", name), name.to_string()]
        };
        for candidate in candidates {
            let output = silent_command("where.exe").arg(&candidate).output().ok()?;
            if !output.status.success() {
                continue;
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(first_line) = stdout.lines().next() {
                let path = PathBuf::from(first_line.trim());
                if path.exists() {
                    return Some(path);
                }
            }
        }
        None
    }

    /// 通过 App Paths 注册表查找应用（Windows 自身的 "运行" 框机制）
    ///
    /// 读取 `HKCU\Software\Microsoft\Windows\CurrentVersion\App Paths\<exe>` 和
    /// `HKLM\Software\Microsoft\Windows\CurrentVersion\App Paths\<exe>` 的默认值，
    /// 该值即为应用的完整安装路径。速度极快（单次 `reg query`），覆盖大多数规范安装的应用。
    fn find_via_app_paths(name: &str) -> Option<PathBuf> {
        let name_lower = name.to_lowercase();
        let exe_name = if name_lower.ends_with(".exe") {
            name.to_string()
        } else {
            format!("{}.exe", name)
        };

        let reg_keys = [
            format!(
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\App Paths\{}",
                exe_name
            ),
            format!(
                r"HKLM\Software\Microsoft\Windows\CurrentVersion\App Paths\{}",
                exe_name
            ),
        ];

        for key in &reg_keys {
            let output = match silent_command("reg")
                .arg("query")
                .arg(key)
                .arg("/ve")
                .output()
            {
                Ok(o) => o,
                Err(_) => continue,
            };
            if !output.status.success() {
                continue;
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            // 格式: "    (Default)    REG_SZ    C:\Program Files\App\app.exe"
            for line in stdout.lines() {
                if let Some((_, value)) = parse_reg_sz_line(line.trim()) {
                    let p = PathBuf::from(value.trim());
                    if p.exists() && p.extension().map_or(false, |e| e == "exe") {
                        return Some(p);
                    }
                }
            }
        }
        None
    }

    /// 扫描常见安装目录（深度 2，匹配 `<Publisher>\<App>\<App>.exe`）
    fn find_in_common_dirs(name: &str) -> Option<PathBuf> {
        let name_lower = name.to_lowercase();
        let name_with_exe = if name_lower.ends_with(".exe") {
            name.to_string()
        } else {
            format!("{}.exe", name)
        };
        let name_with_exe_lower = name_with_exe.to_lowercase();

        let mut search_roots: Vec<PathBuf> = Vec::new();
        if let Ok(pf) = std::env::var("ProgramFiles") {
            if !pf.is_empty() {
                search_roots.push(PathBuf::from(pf));
            }
        }
        if let Ok(pf) = std::env::var("ProgramFiles(x86)") {
            if !pf.is_empty() {
                search_roots.push(PathBuf::from(pf));
            }
        }
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            if !local_app_data.is_empty() {
                search_roots.push(PathBuf::from(local_app_data).join("Programs"));
            }
        }
        // 额外扫描 APPDATA（部分绿色版 / 用户安装的应用在此）
        if let Ok(app_data) = std::env::var("APPDATA") {
            if !app_data.is_empty() {
                search_roots.push(PathBuf::from(app_data));
            }
        }

        for root in &search_roots {
            if let Some(found) =
                search_dir_for_exe(root, &name_with_exe_lower, &name_lower, 3)
            {
                return Some(found);
            }
        }
        None
    }

    /// 递归搜索目录中的 .exe 文件（精确名优先，其次名称包含匹配）
    fn search_dir_for_exe(
        dir: &std::path::Path,
        exact_name_lower: &str,
        name_lower: &str,
        max_depth: u32,
    ) -> Option<PathBuf> {
        if max_depth == 0 {
            return None;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return None,
        };
        let mut fuzzy_hits: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) =
                    search_dir_for_exe(&path, exact_name_lower, name_lower, max_depth - 1)
                {
                    return Some(found);
                }
            } else {
                let file_name = match path.file_name().and_then(|s| s.to_str()) {
                    Some(s) => s.to_lowercase(),
                    None => continue,
                };
                // 精确匹配（带 .exe 后缀）立即返回
                if file_name == exact_name_lower {
                    return Some(path);
                }
                // 名称包含匹配（去掉 .exe 后缀做包含比较），暂存
                if file_name.ends_with(".exe") {
                    let stem = &file_name[..file_name.len() - 4];
                    if stem == name_lower || stem.contains(name_lower) {
                        fuzzy_hits.push(path);
                    }
                }
            }
        }
        fuzzy_hits.into_iter().next()
    }

    // ── Start Menu 快捷方式扫描（增强版） ──

    /// 增强版快捷方式查找：
    /// 1. 先按 .lnk 文件名匹配（原有逻辑）
    /// 2. 不命中时，用 PowerShell 批量读取所有 .lnk 的 target exe 名，按 exe 名匹配
    fn find_shortcut_enhanced(name: &str) -> Option<PathBuf> {
        let name_lower = name.to_lowercase();

        let mut search_roots: Vec<PathBuf> = Vec::new();
        if let Ok(app_data) = std::env::var("APPDATA") {
            if !app_data.is_empty() {
                search_roots.push(
                    PathBuf::from(&app_data)
                        .join("Microsoft")
                        .join("Windows")
                        .join("Start Menu")
                        .join("Programs"),
                );
            }
        }
        search_roots.push(
            PathBuf::from("C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs"),
        );

        // Phase 1: 按 .lnk 文件名匹配（快速路径）
        for root in &search_roots {
            if let Some(found) = search_dir_for_lnk(root, &name_lower, 4) {
                return Some(found);
            }
        }

        // Phase 2: 读取所有 .lnk 的 target，按 exe 名匹配
        // 使用 PowerShell 一次性获取所有快捷方式信息
        let lnk_targets = match resolve_all_shortcut_targets(&search_roots) {
            Some(t) => t,
            None => return None,
        };

        let name_norm = normalize_name(name);
        for (lnk_path, target_exe) in &lnk_targets {
            let exe_stem = PathBuf::from(target_exe)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if exe_stem.is_empty() {
                continue;
            }
            // 精确匹配 exe stem
            if exe_stem == name_norm {
                return Some(lnk_path.clone());
            }
            // 包含匹配（带长度守卫）
            if is_fuzzy_superset(&exe_stem, &name_norm) || is_fuzzy_superset(&name_norm, &exe_stem) {
                return Some(lnk_path.clone());
            }
        }

        None
    }

    /// 按 .lnk 文件名搜索（深度 4，含子文件夹）
    fn search_dir_for_lnk(dir: &std::path::Path, name_lower: &str, max_depth: u32) -> Option<PathBuf> {
        if max_depth == 0 {
            return None;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return None,
        };
        let mut fuzzy_hits: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = search_dir_for_lnk(&path, name_lower, max_depth - 1) {
                    return Some(found);
                }
            } else {
                let file_name = match path.file_name().and_then(|s| s.to_str()) {
                    Some(s) => s.to_lowercase(),
                    None => continue,
                };
                if file_name.ends_with(".lnk") {
                    let stem = &file_name[..file_name.len() - 4];
                    if stem == name_lower {
                        return Some(path);
                    }
                    // 双向包含（带长度守卫）：防止短名 "qq" 误匹配 "qq音乐"
                    if is_fuzzy_superset(stem, name_lower) || is_fuzzy_superset(name_lower, stem) {
                        fuzzy_hits.push(path);
                    }
                }
            }
        }
        fuzzy_hits.into_iter().next()
    }

    /// 用 PowerShell 批量读取指定目录下所有 .lnk 文件的 target path
    /// 返回 Vec<(lnk_path, target_exe_path)>
    fn resolve_all_shortcut_targets(roots: &[PathBuf]) -> Option<Vec<(PathBuf, String)>> {
        let root_paths: Vec<String> = roots
            .iter()
            .map(|r| format!("'{}'", r.display().to_string().replace('\'', "''")))
            .collect();
        let root_array = root_paths.join(",");

        // PowerShell 脚本：遍历目录下所有 .lnk，用 WScript.Shell 解析 target
        let ps_script = format!(
            r#"$shell = New-Object -ComObject WScript.Shell
$roots = @({root_array})
foreach ($root in $roots) {{
    if (Test-Path $root) {{
        Get-ChildItem -Path $root -Filter '*.lnk' -Recurse -ErrorAction SilentlyContinue | ForEach-Object {{
            try {{
                $shortcut = $shell.CreateShortcut($_.FullName)
                $target = $shortcut.TargetPath
                if ($target -and $target.EndsWith('.exe')) {{
                    Write-Output "$($_.FullName)|$target"
                }}
            }} catch {{}}
        }}
    }}
}}"#
        );

        let output = silent_command("powershell")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(&ps_script)
            .output()
            .ok()?;

        if !output.status.success() {
            tracing::debug!(
                "[open_application] PowerShell shortcut 解析失败: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return None;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut results = Vec::new();
        for line in stdout.lines() {
            if let Some((lnk, target)) = line.split_once('|') {
                let lnk_path = PathBuf::from(lnk.trim());
                let target_exe = target.trim().to_string();
                if lnk_path.exists() && !target_exe.is_empty() {
                    results.push((lnk_path, target_exe));
                }
            }
        }
        Some(results)
    }

    // ── Windows 注册表 Uninstall 键查询 ──

    /// 通过注册表 Uninstall 键查找已安装应用
    /// 读取 DisplayName / InstallLocation / DisplayIcon，匹配应用名后定位 exe
    fn find_via_registry(name: &str) -> Option<PathBuf> {
        let name_lower = name.to_lowercase();
        let name_norm = normalize_name(name);

        let registry_paths = [
            r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*",
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*",
        ];

        for reg_path in &registry_paths {
            let output = match silent_command("reg")
                .arg("query")
                .arg(reg_path)
                .arg("/s")
                .output()
            {
                Ok(o) => o,
                Err(_) => continue,
            };
            if !output.status.success() {
                continue;
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(found) = parse_registry_output(&stdout, &name_lower, &name_norm) {
                return Some(found);
            }
        }
        None
    }

    /// 解析 `reg query /s` 输出，按 DisplayName 匹配后提取 InstallLocation 或 DisplayIcon
    fn parse_registry_output(output: &str, name_lower: &str, name_norm: &str) -> Option<PathBuf> {
        // reg query /s 输出格式：每个子键之间用空行分隔
        // 每个子键下有 REG_SZ 值
        let mut current_display_name: Option<String> = None;
        let mut current_install_location: Option<String> = None;
        let mut current_display_icon: Option<String> = None;
        let mut candidates: Vec<PathBuf> = Vec::new();

        for line in output.lines() {
            let trimmed = line.trim();

            // 空行 = 子键分隔，处理前一个子键的结果
            if trimmed.is_empty() {
                if let Some(found) = try_match_registry_entry(
                    &current_display_name,
                    &current_install_location,
                    &current_display_icon,
                    name_lower,
                    name_norm,
                ) {
                    candidates.push(found);
                }
                current_display_name = None;
                current_install_location = None;
                current_display_icon = None;
                continue;
            }

            // 解析 REG_SZ 行：形如 "    DisplayName    REG_SZ    QQ音乐"
            if let Some((key, value)) = parse_reg_sz_line(trimmed) {
                match key {
                    "DisplayName" => current_display_name = Some(value),
                    "InstallLocation" => current_install_location = Some(value),
                    "DisplayIcon" => current_display_icon = Some(value),
                    _ => {}
                }
            }
        }

        // 处理最后一个子键
        if let Some(found) = try_match_registry_entry(
            &current_display_name,
            &current_install_location,
            &current_display_icon,
            name_lower,
            name_norm,
        ) {
            candidates.push(found);
        }

        candidates.into_iter().next()
    }

    /// 解析 "Key    REG_SZ    Value" 格式的行
    fn parse_reg_sz_line(line: &str) -> Option<(&str, String)> {
        let parts: Vec<&str> = line.splitn(2, "REG_SZ").collect();
        if parts.len() != 2 {
            return None;
        }
        let key = parts[0].trim();
        let value = parts[1].trim().to_string();
        if key.is_empty() || value.is_empty() {
            return None;
        }
        Some((key, value))
    }

    /// 检查注册表条目是否匹配目标应用，匹配则返回 exe 路径
    fn try_match_registry_entry(
        display_name: &Option<String>,
        install_location: &Option<String>,
        display_icon: &Option<String>,
        name_lower: &str,
        name_norm: &str,
    ) -> Option<PathBuf> {
        let dn = display_name.as_ref()?;
        let dn_lower = dn.to_lowercase();

        // DisplayName 匹配：精确、包含、或规范化后匹配
        let matched = dn_lower == *name_lower
            || dn_lower.contains(name_lower)
            || name_lower.contains(&dn_lower)
            || normalize_name(dn) == *name_norm;

        if !matched {
            return None;
        }

        // 优先用 DisplayIcon（通常直接指向 exe）
        if let Some(icon) = display_icon {
            // DisplayIcon 可能是 "C:\path\app.exe,0" 格式，去掉逗号后的索引
            let icon_path = icon.split(',').next().unwrap_or(icon).trim();
            let p = PathBuf::from(icon_path);
            if p.exists() && p.extension().map_or(false, |e| e == "exe") {
                return Some(p);
            }
        }

        // 其次用 InstallLocation + 搜索 exe
        if let Some(loc) = install_location {
            let loc = loc.trim().trim_end_matches('\\');
            if loc.is_empty() {
                return None;
            }
            let loc_path = PathBuf::from(loc);
            if loc_path.is_dir() {
                // 在 InstallLocation 下搜索与 DisplayName 或 name 匹配的 exe
                let exe_name_from_display = normalize_name(dn);
                if let Some(found) = search_dir_for_exe(
                    &loc_path,
                    &format!("{}.exe", name_norm),
                    name_norm,
                    3,
                ) {
                    return Some(found);
                }
                if exe_name_from_display != *name_norm {
                    if let Some(found) = search_dir_for_exe(
                        &loc_path,
                        &format!("{}.exe", exe_name_from_display),
                        &exe_name_from_display,
                        3,
                    ) {
                        return Some(found);
                    }
                }
                // 兜底：目录下任意 exe（如果只有一个）
                let mut exes: Vec<PathBuf> = Vec::new();
                if let Ok(entries) = std::fs::read_dir(&loc_path) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.extension().map_or(false, |e| e == "exe") {
                            exes.push(p);
                            if exes.len() > 3 { break; }
                        }
                    }
                }
                if exes.len() == 1 {
                    return Some(exes.into_iter().next().unwrap());
                }
            }
        }

        None
    }

    /// 通过 `Get-StartApps` 查找 UWP 应用，返回匹配的 AppID
    fn find_uwp_app(name: &str) -> Option<String> {
        let name_lower = name.to_lowercase();
        let output = silent_command("powershell")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg("Get-StartApps | ConvertTo-Json")
            .output()
            .ok()?;
        if !output.status.success() {
            tracing::debug!(
                "[open_application] Get-StartApps 失败: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let apps: Vec<Value> = if stdout.trim_start().starts_with('[') {
            serde_json::from_str(&stdout).unwrap_or_default()
        } else {
            serde_json::from_str::<Value>(&stdout)
                .ok()
                .map(|v| vec![v])
                .unwrap_or_default()
        };
        fn is_safe_appid(s: &str) -> bool {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '!'))
        }
        for app in &apps {
            if let Some(n) = app.get("Name").and_then(|v| v.as_str()) {
                if n.to_lowercase() == name_lower {
                    if let Some(appid) = app.get("AppID").and_then(|v| v.as_str()) {
                        if appid.contains('!') && is_safe_appid(appid) {
                            return Some(appid.to_string());
                        }
                    }
                }
            }
        }
        for app in &apps {
            if let Some(n) = app.get("Name").and_then(|v| v.as_str()) {
                if n.to_lowercase().contains(&name_lower) {
                    if let Some(appid) = app.get("AppID").and_then(|v| v.as_str()) {
                        if appid.contains('!') && is_safe_appid(appid) {
                            return Some(appid.to_string());
                        }
                    }
                }
            }
        }
        None
    }
}

#[async_trait]
impl Tool for OpenApplicationTool {
    fn name(&self) -> &str {
        "open_application"
    }

    fn description(&self) -> &str {
        "Launch an application. Pass the app name (English or Chinese) or a full path. \
         Common Chinese names are auto-mapped (e.g. QQ音乐→QQMusic, 微信→WeChat, 钉钉→DingTalk, 原神→Yuanshen). \
         Search order: app cache → App Paths registry → PATH → install directories → Start Menu shortcuts (with target resolution) \
         → Windows Registry → UWP apps. If not found, the error includes suggestions for fallback actions."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "启动应用程序。传入应用名（中英文均可）或完整路径。\
         常见中文名会自动映射（如 QQ音乐→QQMusic、微信→WeChat、钉钉→DingTalk、原神→Yuanshen）。\
         查找顺序：应用缓存 → App Paths注册表 → PATH → 安装目录 → 开始菜单快捷方式（含目标解析）\
         → Windows 注册表 → UWP 应用。未找到时错误信息会包含建议的备选操作。",
            "ja" => "アプリケーションを起動する。アプリ名（英語または中国語）または完全パスを渡す。\
         一般的な中国語名は自動マッピングされる（例：QQ音楽→QQMusic、微信→WeChat、原神→Yuanshen）。\
         検索順序：アプリキャッシュ → App Pathsレジストリ → PATH → インストールディレクトリ → スタートメニューショートカット\
         → Windowsレジストリ → UWPアプリ。見つからない場合はフォールバックの提案を含むエラーが返される。",
            _ => self.description(),
        }
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Open a URL (use open_url instead)",
            "Open a folder (use open_folder instead)",
            "Bring an already open window to the foreground (use system keyboard shortcuts instead)",
        ]
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "application": {
                    "type": "string",
                    "description": "Application name (English or Chinese, e.g. notepad / chrome / QQ音乐 / 微信) \
                     or a full path (e.g. C:\\\\Windows\\\\System32\\\\notepad.exe). \
                     Chinese names are auto-mapped to English executable names for common apps."
                },
                "args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Command-line arguments to pass to the application"
                }
            },
            "required": ["application"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "application": {
                        "type": "string",
                        "description": "应用名称（中英文均可，例如 notepad / chrome / QQ音乐 / 微信）\
                         或完整路径（例如 C:\\\\Windows\\\\System32\\\\notepad.exe）。\
                         常见中文应用名会自动映射为英文可执行文件名。"
                    },
                    "args": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "传递给应用程序的命令行参数"
                    }
                },
                "required": ["application"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "application": {
                        "type": "string",
                        "description": "アプリケーション名（英語または中国語、例：notepad / chrome / QQ音楽 / 微信）\
                         または完全パス（例：C:\\\\Windows\\\\System32\\\\notepad.exe）。\
                         一般的な中国語アプリ名は英語の実行ファイル名に自動マッピングされる。"
                    },
                    "args": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "アプリケーションに渡すコマンドライン引数"
                    }
                },
                "required": ["application"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let app = match input.get("application").and_then(|v| v.as_str()) {
            Some(p) if !p.is_empty() => p.to_string(),
            _ => return ValidationResult::failure("application 是必填项", 2),
        };
        // 仅对路径形式做存在性检查；纯应用名交给 call 阶段解析
        #[cfg(target_os = "windows")]
        {
            if windows_resolver::is_path_like(&app) && !std::path::Path::new(&app).exists() {
                return ValidationResult::failure(format!("应用不存在: {}", app), 200);
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            if app.contains('\\') || app.contains('/') {
                if !std::path::Path::new(&app).exists() {
                    return ValidationResult::failure(
                        format!("应用不存在: {}", app),
                        200,
                    );
                }
            }
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(
        &self,
        input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        let app = input
            .get("application")
            .and_then(|v| v.as_str())
            .unwrap_or("未知应用");
        PermissionResult::ask(format!("想要启动应用「{}」", app))
    }

    async fn call(&self, args: Value, _context: &ToolUseContext) -> ToolResult {
        let application = args
            .get("application")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let cli_args: Vec<String> = args
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        #[cfg(target_os = "windows")]
        {
            let app_for_resolve = application.clone();
            let resolve_result = tokio::task::spawn_blocking(move || -> Result<Vec<windows_resolver::ResolvedCandidate>, &'static str> {
                windows_resolver::resolve(&app_for_resolve)
            }).await;

            let (target, resolved_desc, type_tag, score, source) = match resolve_result {
                Ok(Ok(candidates)) if candidates.is_empty() => {
                    tracing::info!(
                        target: "app_resolver",
                        query = %application,
                        action = "not_found",
                        "resolver_no_candidates"
                    );
                    return ToolResult::standard_error(
                        &format!("未找到应用: {}", application),
                        Some("AppNotFound"),
                        Some(json!({
                            "application": application,
                            "searched": ["cache", "app_paths", "path", "common_dirs", "shortcut", "registry", "uwp"],
                            "suggestions": [
                                "尝试使用 open_url 打开该应用的网页版",
                                "如果知道应用的完整路径，可以直接传 application 参数为完整路径",
                                "尝试用 get_running_processes 检查应用是否已在运行",
                            ],
                        })),
                    );
                }
                Ok(Ok(candidates)) => {
                    let top = &candidates[0];
                    let second_score = candidates.get(1).map(|c| c.score).unwrap_or(0.0);
                    let confidence_gap = top.score - second_score;

                    let should_auto_launch = windows_resolver::should_auto_launch(&candidates);

                    if should_auto_launch {
                        tracing::info!(
                            target: "app_resolver",
                            query = %application,
                            score = top.score,
                            gap = confidence_gap,
                            source = top.source,
                            action = "auto_launch",
                            "resolver_auto_launch"
                        );
                        let desc = top.target.describe();
                        let tag = top.target.type_tag();
                        let s = top.score;
                        let src = top.source;
                        let target = top.target.clone();
                        (target, desc, tag, s, src)
                    } else {
                        // Phase 3: LLM escalation — 置信度不足，返回候选列表让 LLM 决策
                        tracing::info!(
                            target: "app_resolver",
                            query = %application,
                            top_score = top.score,
                            gap = confidence_gap,
                            candidate_count = candidates.len(),
                            action = "escalate_to_llm",
                            "resolver_escalate"
                        );
                        let candidate_list: Vec<Value> = candidates.iter().take(5).map(|c| {
                            // 直接给出可重新传给 open_application 的纯路径/ID（去掉 exe:/lnk:/uwp: 前缀），
                            // LLM 可据此直接重试调用，绕过消歧判定
                            let raw_path = match &c.target {
                                windows_resolver::AppTarget::Exe(p) => p.display().to_string(),
                                windows_resolver::AppTarget::Lnk(p) => p.display().to_string(),
                                windows_resolver::AppTarget::Uwp(appid) => appid.clone(),
                            };
                            json!({
                                "path": raw_path,
                                "score": (c.score * 100.0).round() / 100.0,
                                "source": c.source,
                                "type": c.target.type_tag(),
                            })
                        }).collect();

                        if top.score >= 0.75 {
                            return ToolResult::standard_error(
                                &format!("找到多个可能的应用，无法确定用户想要哪一个，请从 candidates 中选择最匹配的 path 直接重新调用 open_application"),
                                Some("AppUncertain"),
                                Some(json!({
                                    "application": application,
                                    "status": "uncertain",
                                    "top_score": (top.score * 100.0).round() / 100.0,
                                    "confidence_gap": (confidence_gap * 100.0).round() / 100.0,
                                    "candidates": candidate_list,
                                    "next_action": "从 candidates 数组中选择最匹配的 path 字段，直接用该 path 作为 application 参数重新调用 open_application（不要再次使用模糊名称）",
                                })),
                            );
                        } else {
                            return ToolResult::standard_error(
                                &format!("未找到足够可信的应用匹配: {}", application),
                                Some("AppNotFound"),
                                Some(json!({
                                    "application": application,
                                    "status": "low_confidence",
                                    "top_score": (top.score * 100.0).round() / 100.0,
                                    "candidates": candidate_list,
                                    "searched": ["cache", "app_paths", "path", "common_dirs", "shortcut", "registry", "uwp"],
                                    "next_action": [
                                        "expand_search",
                                        "ask_user",
                                        "use_web_version",
                                    ],
                                })),
                            );
                        }
                    }
                }
                Ok(Err(reason)) => {
                    return ToolResult::standard_error(
                        reason,
                        Some("SecurityBlocked"),
                        Some(json!({
                            "application": application,
                        })),
                    );
                }
                Err(_) => {
                    return ToolResult::standard_error(
                        "应用解析失败",
                        Some("ResolveError"),
                        None,
                    );
                }
            };

            let spawn_result = match &target {
                windows_resolver::AppTarget::Exe(path) => {
                    let mut cmd = silent_command(path);
                    cmd.args(&cli_args);
                    cmd.spawn()
                }
                windows_resolver::AppTarget::Lnk(path) => {
                    let mut cmd = silent_command("cmd");
                    cmd.arg("/c").arg("start").arg("").arg(path);
                    for a in &cli_args {
                        cmd.arg(a);
                    }
                    cmd.spawn()
                }
                windows_resolver::AppTarget::Uwp(appid) => {
                    let mut cmd = silent_command("powershell");
                    cmd.arg("-NoProfile")
                        .arg("-NonInteractive")
                        .arg("-Command")
                        .arg(format!("Start-Process shell:AppsFolder:{}", appid));
                    cmd.spawn()
                }
            };

            return match spawn_result {
                Ok(child) => {
                    let pid = child.id();
                    windows_resolver::record_success(&target);
                    tracing::info!(
                        target: "app_resolver",
                        query = %application,
                        resolved = %resolved_desc,
                        score = score,
                        source = source,
                        pid = pid,
                        "app_launch_success"
                    );
                    ToolResult::standard_success(
                        &format!("已启动应用: {}", application),
                        Some(json!({
                            "application": application,
                            "resolved_target": resolved_desc,
                            "target_type": type_tag,
                            "confidence": (score * 100.0).round() / 100.0,
                            "source": source,
                            "args": cli_args,
                            "pid": pid,
                        })),
                    )
                }
                Err(e) => {
                    windows_resolver::record_failure(&target);
                    tracing::warn!(
                        target: "app_resolver",
                        query = %application,
                        resolved = %resolved_desc,
                        error = %e,
                        "app_launch_failure"
                    );
                    ToolResult::standard_error(
                        &format!("启动应用失败: {}", e),
                        Some("AppLaunchFailed"),
                        Some(json!({
                            "application": application,
                            "resolved_target": resolved_desc,
                            "target_type": type_tag,
                            "error": e.to_string(),
                        })),
                    )
                }
            };
        }

        #[cfg(not(target_os = "windows"))]
        {
            if application.contains(';') || application.contains('&') || application.contains('|')
                || application.contains('`') || application.contains('$') || application.contains('(')
                || application.starts_with("sudo") {
                return ToolResult::standard_error(
                    "出于安全考虑，应用名包含非法字符",
                    Some("SecurityBlocked"),
                    None,
                );
            }
            let mut cmd = silent_command(&application);
            cmd.args(&cli_args);
            match cmd.spawn() {
                Ok(child) => {
                    let pid = child.id();
                    ToolResult::standard_success(
                        &format!("已启动应用: {}", application),
                        Some(json!({
                            "application": application,
                            "args": cli_args,
                            "pid": pid,
                        })),
                    )
                }
                Err(e) => ToolResult::standard_error(
                    &format!("启动应用失败: {}", e),
                    Some("AppLaunchFailed"),
                    Some(json!({
                        "application": application,
                        "error": e.to_string(),
                    })),
                ),
            }
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    /// 始终全量加载（核心工具）
    fn always_load(&self) -> bool {
        true
    }

    /// 搜索提示
    fn search_hint(&self) -> &str {
        "launch application"
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Shell
    }
}

/// close_application 工具 - 关闭应用程序
pub struct CloseApplicationTool;

impl CloseApplicationTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CloseApplicationTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for CloseApplicationTool {
    fn name(&self) -> &str {
        "close_application"
    }

    fn description(&self) -> &str {
        "Close a running application. Can be closed by process name or PID; uses taskkill for graceful shutdown by default."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "关闭正在运行的应用程序。可按进程名或 PID 关闭；默认使用 taskkill 进行优雅关闭。",
            "ja" => "実行中のアプリケーションを閉じる。プロセス名または PID で閉じることができる；デフォルトでは taskkill を使用してグレースフルシャットダウンする。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "application": {
                    "type": "string",
                    "description": "Application name (matched by process name)"
                },
                "pid": {
                    "type": "integer",
                    "description": "Process ID (preferred)",
                    "minimum": 1
                },
                "force": {
                    "type": "boolean",
                    "description": "Whether to force terminate, default false"
                }
            }
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "application": {
                        "type": "string",
                        "description": "应用程序名称（按进程名匹配）"
                    },
                    "pid": {
                        "type": "integer",
                        "description": "进程 ID（推荐）",
                        "minimum": 1
                    },
                    "force": {
                        "type": "boolean",
                        "description": "是否强制终止，默认 false"
                    }
                }
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "application": {
                        "type": "string",
                        "description": "アプリケーション名（プロセス名でマッチング）"
                    },
                    "pid": {
                        "type": "integer",
                        "description": "プロセス ID（推奨）",
                        "minimum": 1
                    },
                    "force": {
                        "type": "boolean",
                        "description": "強制終了するかどうか、デフォルト false"
                    }
                }
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let has_name = input
            .get("application")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        let has_pid = input.get("pid").and_then(|v| v.as_u64()).is_some();
        if !has_name && !has_pid {
            return ValidationResult::failure("必须提供 application 或 pid 之一", 2);
        }
        let mut data = input.clone();
        if data.get("force").is_none() {
            data["force"] = json!(false);
        }
        ValidationResult::success(Some(data))
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::ask("关闭应用程序需要用户确认")
    }

    async fn call(&self, args: Value, _context: &ToolUseContext) -> ToolResult {
        let application = args.get("application").and_then(|v| v.as_str()).map(|s| s.to_string());
        let pid = args.get("pid").and_then(|v| v.as_u64()).map(|p| p as u32);
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

        #[cfg(target_os = "windows")]
        {
            let app_for_closure = application.clone();
            let result = tokio::task::spawn_blocking(move || -> std::io::Result<std::process::Output> {
                let mut cmd = silent_command("taskkill");
                if force {
                    cmd.arg("/F");
                }
                match (pid, app_for_closure.as_deref()) {
                    (Some(p), _) => {
                        cmd.arg("/PID").arg(p.to_string());
                    }
                    (None, Some(name)) => {
                        cmd.arg("/IM").arg(name);
                    }
                    _ => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "未指定要关闭的目标",
                        ));
                    }
                }
                cmd.output()
            })
            .await;

            match result {
                Ok(Ok(output)) => {
                    if output.status.success() {
                        ToolResult::standard_success(
                            "已发送关闭命令",
                            Some(json!({
                                "application": application,
                                "pid": pid,
                                "force": force,
                                "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                            })),
                        )
                    } else {
                        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                        ToolResult::standard_error(
                            &format!("关闭应用失败: {}", stderr),
                            Some("CloseAppFailed"),
                            Some(json!({
                                "application": application,
                                "pid": pid,
                                "stderr": stderr,
                            })),
                        )
                    }
                }
                Ok(Err(e)) => {
                    if e.kind() == std::io::ErrorKind::InvalidInput {
                        ToolResult::standard_error(
                            "未指定要关闭的目标",
                            Some("InvalidInput"),
                            None,
                        )
                    } else {
                        ToolResult::standard_error(
                            &format!("执行 taskkill 失败: {}", e),
                            Some("CloseAppFailed"),
                            None,
                        )
                    }
                }
                Err(e) => {
                    ToolResult::standard_error(
                        &format!("关闭应用任务失败: {}", e),
                        Some("CloseAppFailed"),
                        None,
                    )
                }
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = force;
            ToolResult::standard_error(
                "close_application 在当前平台未实现",
                Some("NotImplemented"),
                None,
            )
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn is_destructive(&self) -> bool {
        true
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Shell
    }
}

/// take_screenshot 工具 - 截屏
pub struct TakeScreenshotTool;

impl TakeScreenshotTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TakeScreenshotTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TakeScreenshotTool {
    fn name(&self) -> &str {
        "take_screenshot"
    }

    fn description(&self) -> &str {
        "Capture the current screen, save it as a PNG file, and copy it to the clipboard. Returns the file path."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "截取当前屏幕，保存为 PNG 文件，并自动复制到系统剪贴板。返回文件路径。",
            "ja" => "現在の画面をキャプチャし、PNG ファイルとして保存し、クリップボードにコピーする。ファイルパスを返す。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "output_path": {
                    "type": "string",
                    "description": "Screenshot save path (optional; defaults to the temp directory)"
                }
            }
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "output_path": {
                        "type": "string",
                        "description": "截图保存路径（可选；默认为临时目录）"
                    }
                }
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "output_path": {
                        "type": "string",
                        "description": "スクリーンショットの保存パス（任意；デフォルトは一時ディレクトリ）"
                    }
                }
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, _input: &Value, _context: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::ask("take_screenshot 涉及屏幕截取，需要用户确认")
    }

    async fn call(&self, args: Value, _context: &ToolUseContext) -> ToolResult {
        let output_path = args
            .get("output_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                let mut p = crate::utils::path::get_user_data_dir();
                p.push("screenshots");
                let _ = std::fs::create_dir_all(&p);
                p.push(format!(
                    "screenshot_{}.png",
                    chrono::Local::now().format("%Y%m%d_%H%M%S")
                ));
                p.to_string_lossy().to_string()
            });

        let path_obj = std::path::Path::new(&output_path);
        if !output_path.ends_with(".png") {
            return ToolResult::standard_error(
                "输出路径必须以 .png 结尾",
                Some("ScreenshotFailed"),
                None,
            );
        }
        if output_path.contains("..") {
            return ToolResult::standard_error(
                "输出路径包含非法字符 ..",
                Some("ScreenshotFailed"),
                None,
            );
        }
        if let Some(parent) = path_obj.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }

        #[cfg(target_os = "windows")]
        {
            use std::fs;

            fn is_safe_path_for_ps(p: &str) -> bool {
                p.chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '\\' | ':' | ' ' | '/'))
            }
            if !is_safe_path_for_ps(&output_path) {
                return ToolResult::standard_error(
                    &format!("输出路径含非法字符（拒绝反引号/$/括号等以避免命令注入）: {}", output_path),
                    Some("ScreenshotFailed"),
                    None,
                );
            }

            let escaped_path = output_path.replace('\'', "''");
            let ps_script = format!(
                r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bmp = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
$graphics = [System.Drawing.Graphics]::FromImage($bmp)
$graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$bmp.Save('{}', [System.Drawing.Imaging.ImageFormat]::Png)
[System.Windows.Forms.Clipboard]::SetImage($bmp)
$graphics.Dispose()
$bmp.Dispose()
"#,
                escaped_path
            );

            let ps_script_for_task = ps_script.clone();
            let output_path_for_task = output_path.clone();
            let task_result = tokio::task::spawn_blocking(move || {
                let mut cmd = silent_command("powershell");
                cmd.arg("-STA")
                    .arg("-NoProfile")
                    .arg("-NonInteractive")
                    .arg("-Command")
                    .arg(&ps_script_for_task);
                match cmd.output() {
                    Ok(output) => {
                        if output.status.success()
                            && std::path::Path::new(&output_path_for_task).exists()
                        {
                            let size = fs::metadata(&output_path_for_task)
                                .map(|m| m.len())
                                .unwrap_or(0);
                            ToolResult::standard_success(
                                "截屏成功，已复制到剪贴板",
                                Some(json!({
                                    "output_path": output_path_for_task,
                                    "size_bytes": size,
                                    "in_clipboard": true,
                                })),
                            )
                        } else {
                            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                            ToolResult::standard_error(
                                &format!("截屏失败: {}", stderr),
                                Some("ScreenshotFailed"),
                                None,
                            )
                        }
                    }
                    Err(e) => ToolResult::standard_error(
                        &format!("启动 PowerShell 失败: {}", e),
                        Some("ScreenshotFailed"),
                        None,
                    ),
                }
            })
            .await;

            match task_result {
                Ok(result) => return result,
                Err(e) => {
                    return ToolResult::standard_error(
                        &format!("截屏任务执行失败: {}", e),
                        Some("ScreenshotFailed"),
                        None,
                    );
                }
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = path_obj;
            ToolResult::standard_error(
                "截屏功能在当前平台未实现",
                Some("ScreenshotFailed"),
                None,
            )
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    /// 始终全量加载（核心工具）
    fn always_load(&self) -> bool {
        true
    }

    /// 搜索提示
    fn search_hint(&self) -> &str {
        "take screenshot"
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }
}

// ============================================================================
// ScreenshotAnalyzeTool - 截屏并送视觉理解
// ============================================================================

/// screenshot_analyze 工具 - 截屏并识图
///
/// 与 `take_screenshot` 并列的姊妹工具：截取当前屏幕后**不保存、不复制剪贴板**，
/// 直接将 PNG base64 送入视觉理解流程（`vision_describe` 任务路由），
/// 返回 LLM 对屏幕内容的客观描述 + 角色口吻回应。
///
/// 适用场景：用户让你"看看屏幕"/"看一下这个界面"/"我屏幕上显示什么"等
/// 需要视觉上下文才能回答的情况。需要保存截图文件请用 `take_screenshot`。
pub struct ScreenshotAnalyzeTool;

impl ScreenshotAnalyzeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ScreenshotAnalyzeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ScreenshotAnalyzeTool {
    fn name(&self) -> &str {
        "screenshot_analyze"
    }

    fn description(&self) -> &str {
        "Capture the current screen and send it to a vision-capable LLM for understanding. \
         Returns a structured description of what's on screen plus a short in-character reply. \
         Does NOT save the image to disk or copy it to the clipboard. \
         Use this when the user asks you to 'look at' / 'see' / 'check' their screen, \
         or when visual context is needed to answer (e.g. '我屏幕上是什么', '帮我看看这个界面')."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "截取当前屏幕并送视觉模型理解，返回对屏幕内容的客观描述和简短角色回应。\
            不保存图片、不复制剪贴板。当用户让你“看看屏幕”/“看一下这个界面”/“我屏幕上显示什么”等\
            需要视觉上下文的场景使用。",
            "ja" => "現在の画面をキャプチャし、視覚モデルに送って理解させる。\
            画面内容の客観的説明と短いキャラクター返信を返す。\
            画像を保存せず、クリップボードにもコピーしない。\
            ユーザーが「画面を見て」「この画面どう思う」など視覚コンテキストを求める場面で使用。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        let _ = lang;
        self.parameters_schema()
    }

    async fn validate_input(&self, _input: &Value, _context: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::ask("screenshot_analyze 涉及屏幕截取与视觉理解，需要用户确认")
    }

    async fn call(&self, _args: Value, context: &ToolUseContext) -> ToolResult {
        // 1. 拿 AppHandle → AppState → ModelRouter / Config
        let app_handle = match APP_HANDLE.read().clone() {
            Some(h) => h,
            None => {
                return ToolResult::standard_error(
                    "AppHandle 未注入，无法调用视觉模型",
                    Some("ScreenshotAnalyzeFailed"),
                    None,
                )
            }
        };
        let state = match app_handle.try_state::<std::sync::Arc<AppState>>() {
            Some(s) => s,
            None => {
                return ToolResult::standard_error(
                    "AppState 未初始化",
                    Some("ScreenshotAnalyzeFailed"),
                    None,
                )
            }
        };

        // 2. 检查视觉功能是否启用
        if !state
            .config
            .read()
            .get_typed::<bool>("ai.enable_vision", false)
        {
            return ToolResult::standard_error(
                "视觉功能未启用（ai.enable_vision=false）",
                Some("ScreenshotAnalyzeFailed"),
                None,
            );
        }

        // 3. 拿 ModelRouter（克隆一份避免长生命周期锁）
        let router = {
            let guard = state.model_router.read();
            match guard.as_ref() {
                Some(r) => r.clone(),
                None => {
                    return ToolResult::standard_error(
                        "ModelRouter 未初始化",
                        Some("ScreenshotAnalyzeFailed"),
                        None,
                    )
                }
            }
        };

        // 4. 截屏（临时文件读出后立即删除，不复制剪贴板）
        let png_bytes = match capture_screen_png_bytes().await {
            Ok(b) => b,
            Err(e) => {
                return ToolResult::standard_error(&e, Some("ScreenshotAnalyzeFailed"), None)
            }
        };

        // 5. 送视觉理解流程
        Self::run_vision_describe(&router, &state, png_bytes, context).await
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    /// 非核心高频工具，延迟加载（通过 tool_search 拉取完整 schema）
    fn always_load(&self) -> bool {
        false
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "看屏幕 截图识别 视觉理解"
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "需要把截图保存为文件时使用 take_screenshot，而非本工具",
            "用户只是想截图保存分享时不要调用本工具（会消耗 LLM 配额）",
        ]
    }
}

impl ScreenshotAnalyzeTool {
    /// 调用 vision_describe 任务路由，让多模态 LLM 理解截图内容
    async fn run_vision_describe(
        router: &crate::providers::router::ModelRouter,
        state: &std::sync::Arc<AppState>,
        png_bytes: Vec<u8>,
        context: &ToolUseContext,
    ) -> ToolResult {
        let image_detail = state
            .config
            .read()
            .get_typed::<String>("ai.image_detail", "auto".to_string());

        // 上下文：注入最近记忆摘要 + 用户当前消息（如有），帮助 LLM 理解用户为何截屏
        let ctx_block = {
            let mut parts: Vec<String> = Vec::new();
            if let Some(summary) = &context.recent_memory_summary {
                if !summary.is_empty() {
                    parts.push(format!("## 最近记忆摘要\n{}", summary));
                }
            }
            if let Some(user_msg) = &context.user_message {
                if !user_msg.is_empty() {
                    parts.push(format!("## 用户当前消息\n{}", user_msg));
                }
            }
            if parts.is_empty() {
                String::new()
            } else {
                format!(
                    "\n\n{}\n\n请结合以上上下文理解这张截图的意图。",
                    parts.join("\n\n")
                )
            }
        };

        match describe_screen_bytes(router, image_detail, png_bytes, &ctx_block).await {
            Ok((description, reply)) => ToolResult::standard_success(
                "截屏并完成视觉理解",
                Some(json!({
                    "description": description,
                    "reply": reply,
                })),
            ),
            Err(e) => ToolResult::standard_error(
                &e,
                Some("ScreenshotAnalyzeFailed"),
                None,
            ),
        }
    }
}

// ============================================================================
// 可复用的截屏/视觉理解原语（主动交互 screen-peek 与 screenshot_analyze 共享）
// ============================================================================

/// 截取当前屏幕为 PNG 字节（仅内存态：临时文件读出后立即删除，不进剪贴板）
///
/// 从 `ScreenshotAnalyzeTool` 抽出的复用实现，供主动截屏观察等旁路流程调用。
pub(crate) async fn capture_screen_png_bytes() -> Result<Vec<u8>, String> {
    #[cfg(target_os = "windows")]
    {
        use std::fs;

        let mut temp_path = std::env::temp_dir();
        temp_path.push(format!(
            "vivian_screenshot_{}.png",
            uuid::Uuid::new_v4().as_simple()
        ));
        let output_path = temp_path.to_string_lossy().to_string();

        fn is_safe_path_for_ps(p: &str) -> bool {
            p.chars().all(|c| {
                c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '\\' | ':' | ' ' | '/')
            })
        }
        if !is_safe_path_for_ps(&output_path) {
            return Err("临时路径含非法字符".to_string());
        }
        let escaped_path = output_path.replace('\'', "''");
        // 不调用 Clipboard::SetImage（与 take_screenshot 的差异）
        let ps_script = format!(
            r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bmp = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
$graphics = [System.Drawing.Graphics]::FromImage($bmp)
$graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$bmp.Save('{}', [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bmp.Dispose()
"#,
            escaped_path
        );

        let ps_script_for_task = ps_script.clone();
        let output_path_for_task = output_path.clone();
        let capture_result = tokio::task::spawn_blocking(move || {
            let mut cmd = silent_command("powershell");
            cmd.arg("-STA")
                .arg("-NoProfile")
                .arg("-NonInteractive")
                .arg("-Command")
                .arg(&ps_script_for_task);
            match cmd.output() {
                Ok(output) => {
                    if output.status.success()
                        && std::path::Path::new(&output_path_for_task).exists()
                    {
                        Ok(output_path_for_task)
                    } else {
                        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                        Err(format!("截屏失败: {}", stderr))
                    }
                }
                Err(e) => Err(format!("启动 PowerShell 失败: {}", e)),
            }
        })
        .await;

        let png_path = match capture_result {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(format!("截屏任务执行失败: {}", e)),
        };

        let png_bytes = match fs::read(&png_path) {
            Ok(b) => b,
            Err(e) => {
                let _ = fs::remove_file(&png_path);
                return Err(format!("读取截图文件失败: {}", e));
            }
        };
        let _ = fs::remove_file(&png_path);
        Ok(png_bytes)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("截屏功能在当前平台未实现".to_string())
    }
}

/// 将屏幕 PNG 字节送视觉理解（vision_describe 任务路由）
///
/// 返回 `(客观描述, 角色口吻回应)`。`ctx_block` 为附加上下文（可传空串），
/// 由调用方决定是否注入最近记忆/用户消息等。
pub(crate) async fn describe_screen_bytes(
    router: &crate::providers::router::ModelRouter,
    image_detail: String,
    png_bytes: Vec<u8>,
    ctx_block: &str,
) -> Result<(String, String), String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    let b64 = STANDARD.encode(&png_bytes);

    let system_prompt = format!(
        "你是图片描述助手。请分析用户截取的屏幕画面，返回严格的 JSON：\n\
        {{\"description\": \"对屏幕内容的客观、详细的中文描述（用于记忆存档，50-150字）\", \
        \"reply\": \"以角色口吻对屏幕内容给出自然的中文回应（20-60字）\"}}\n\
        仅返回 JSON 对象，不要任何其他内容、不要 markdown 代码块。{}",
        ctx_block
    );

    // 防缓存 nonce（部分 Responses API 服务端缓存 key 不区分图片内容）
    let nonce = uuid::Uuid::new_v4().as_simple().to_string();
    let user_text = format!("请描述这张截图。[req:{}]", &nonce[..8]);

    let image = MessageImage {
        media_type: "image/png".to_string(),
        data: b64,
        url: None,
        detail: Some(image_detail),
    };
    let messages = vec![
        ChatMessage::system(&system_prompt),
        ChatMessage::user_with_images(user_text, vec![image]),
    ];

    match router
        .generate(LLMRequest::new("vision_describe", messages))
        .await
    {
        Ok(text) => Ok(parse_vision_response(&text)),
        Err(e) => Err(format!("视觉理解失败: {}", e)),
    }
}

/// 解析 vision_describe LLM 返回的 JSON（{"description":"...","reply":"..."}）
/// 解析失败时退化为：description 与 reply 均使用原始文本。
fn parse_vision_response(raw: &str) -> (String, String) {
    let trimmed = raw.trim();
    let body = if trimmed.starts_with("```") {
        trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
    } else {
        trimmed
    };
    if let Ok(val) = serde_json::from_str::<Value>(body) {
        let description = val
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let reply = val
            .get("reply")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !description.is_empty() || !reply.is_empty() {
            return (description, reply);
        }
    }
    let fallback = raw.trim().to_string();
    (fallback.clone(), fallback)
}
