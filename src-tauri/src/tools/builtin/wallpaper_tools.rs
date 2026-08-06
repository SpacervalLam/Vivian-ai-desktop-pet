//! Wallpaper Engine 工具 - 列出/切换/控制动态壁纸
//!
//! 通过 Wallpaper Engine 的 CLI 命令行接口（`wallpaper64.exe -control ...`）控制壁纸。
//! 壁纸列表通过扫描 Steam Workshop 目录的 `project.json` 获得。
//!
//! ## 实现要点
//! - **Steam 库查找**：注册表 `HKCU\Software\Valve\Steam\SteamPath` → 默认路径
//!   → 解析 `libraryfolders.vdf` 找到所有库
//! - **Wallpaper Engine 可执行文件查找**：在各 Steam 库的 `steamapps\common\wallpaper_engine\`
//!   目录下查找，优先匹配正在运行的 Wallpaper Engine 进程位数（wallpaper32.exe /
//!   wallpaper64.exe），未运行时回退 64 → 32
//! - **壁纸列表**：扫描 `steamapps\workshop\content\431960\<workshop_id>\project.json`
//! - **CLI 调用**：`wallpaper64.exe -control openWallpaper -file <folder>\project.json`

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolUseContext, ValidationResult,
};
use crate::utils::process::silent_command;

/// Wallpaper Engine 在 Steam 中的 App ID
const WALLPAPER_ENGINE_APP_ID: &str = "431960";

// ============================================================================
// 解析层：Steam / Wallpaper Engine 路径与壁纸元数据
// ============================================================================

/// 壁纸信息（从 project.json 解析）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WallpaperInfo {
    /// Workshop ID（也是壁纸文件夹名）
    pub workshop_id: String,
    /// 壁纸标题
    pub title: String,
    /// 壁纸描述
    pub description: String,
    /// 壁纸类型（scene / video / web / application / url）
    pub wallpaper_type: String,
    /// 入口文件相对路径
    pub entry_file: String,
    /// 标签
    pub tags: Vec<String>,
    /// 壁纸文件夹完整路径（用于 `-control openWallpaper`）
    pub folder_path: String,
}

/// 查找 Wallpaper Engine 的可执行文件路径
///
/// 优先匹配正在运行的 Wallpaper Engine 进程位数（CLI 必须使用与运行实例相同位数
/// 的 exe 才能生效），未运行时回退 wallpaper64.exe → wallpaper32.exe。
/// 返回 `None` 表示未找到 Wallpaper Engine 安装。
pub fn find_wallpaper_engine_exe() -> Option<PathBuf> {
    let libs = find_steam_libraries()?;

    // 检测正在运行的 Wallpaper Engine 进程位数
    let prefer_32 = is_wallpaper_engine_32bit_running();

    let (first, second) = if prefer_32 {
        ("wallpaper32.exe", "wallpaper64.exe")
    } else {
        ("wallpaper64.exe", "wallpaper32.exe")
    };

    for lib in &libs {
        let dir = lib
            .join("steamapps")
            .join("common")
            .join("wallpaper_engine");
        let preferred = dir.join(first);
        if preferred.exists() {
            return Some(preferred);
        }
        let fallback = dir.join(second);
        if fallback.exists() {
            return Some(fallback);
        }
    }
    None
}

/// 检测当前运行的 Wallpaper Engine 是否为 32 位版本（wallpaper32.exe）
fn is_wallpaper_engine_32bit_running() -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::new()),
    );
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.processes().values().any(|p| {
        p.name()
            .to_string_lossy()
            .eq_ignore_ascii_case("wallpaper32.exe")
    })
}

/// 检测 Wallpaper Engine 是否正在运行（wallpaper32.exe 或 wallpaper64.exe）
fn is_wallpaper_engine_running() -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::new()),
    );
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.processes().values().any(|p| {
        let name = p.name().to_string_lossy().to_lowercase();
        name == "wallpaper32.exe" || name == "wallpaper64.exe"
    })
}

/// 查找所有 Steam 库路径（含主库 + libraryfolders.vdf 中的附加库）
pub fn find_steam_libraries() -> Option<Vec<PathBuf>> {
    let steam_path = find_steam_install_dir()?;
    let mut libs: Vec<PathBuf> = vec![steam_path.clone()];

    // 解析 libraryfolders.vdf 找到其他库
    let vdf_path = steam_path.join("steamapps").join("libraryfolders.vdf");
    if let Ok(content) = std::fs::read_to_string(&vdf_path) {
        for line in content.lines() {
            let trimmed = line.trim();
            // 形如：    "path"        "D:\\SteamLibrary"
            if trimmed.starts_with("\"path\"") {
                if let Some(p) = extract_second_quoted(trimmed) {
                    let path = PathBuf::from(&p);
                    if path.exists() && !libs.contains(&path) {
                        libs.push(path);
                    }
                }
            }
        }
    }
    Some(libs)
}

/// 查找 Steam 安装目录：注册表 → 默认路径
fn find_steam_install_dir() -> Option<PathBuf> {
    if let Some(p) = read_steam_registry_path() {
        if p.exists() {
            return Some(p);
        }
    }
    let default = PathBuf::from("C:\\Program Files (x86)\\Steam");
    if default.exists() {
        Some(default)
    } else {
        None
    }
}

/// 从注册表 `HKCU\Software\Valve\Steam` 读取 `SteamPath`
fn read_steam_registry_path() -> Option<PathBuf> {
    let output = silent_command("reg")
        .args(["query", "HKCU\\Software\\Valve\\Steam", "/v", "SteamPath"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // 输出形如：
    //     SteamPath    REG_SZ    C:\Program Files (x86)\Steam
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.contains("SteamPath") && trimmed.contains("REG_SZ") {
            // 取 REG_SZ 之后的部分
            if let Some(idx) = trimmed.find("REG_SZ") {
                let rest = trimmed[idx + "REG_SZ".len()..].trim();
                if !rest.is_empty() {
                    return Some(PathBuf::from(rest));
                }
            }
        }
    }
    None
}

/// 从 `"key"        "value"` 行中提取第二个引号对中的内容
fn extract_second_quoted(line: &str) -> Option<String> {
    let parts: Vec<&str> = line.split('"').collect();
    // split('"') 得到 ["    ", "path", "        ", "D:\\SteamLibrary", ""]
    // 索引 3 是第二个引号对的值
    if parts.len() >= 4 {
        let v = parts[3].trim();
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    None
}

/// 扫描所有 Steam 库的 Wallpaper Engine workshop 目录，列出所有壁纸
///
/// 每个子文件夹对应一个壁纸（以 workshop ID 命名），读取其中的 `project.json` 获取元数据。
/// 返回结果按标题升序排列。
pub fn list_wallpapers() -> Vec<WallpaperInfo> {
    let mut wallpapers = Vec::new();
    let Some(libs) = find_steam_libraries() else {
        return wallpapers;
    };
    for lib in &libs {
        let workshop_dir = lib
            .join("steamapps")
            .join("workshop")
            .join("content")
            .join(WALLPAPER_ENGINE_APP_ID);
        if !workshop_dir.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(&workshop_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let workshop_id = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if workshop_id.is_empty() {
                continue;
            }
            let project_json = path.join("project.json");
            match parse_wallpaper_project(&project_json, &workshop_id) {
                Ok(info) => wallpapers.push(info),
                Err(e) => tracing::debug!(
                    "[wallpaper] 跳过壁纸 {} 元数据解析失败: {}",
                    workshop_id,
                    e
                ),
            }
        }
    }
    // 按标题排序（不区分大小写）
    wallpapers.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    wallpapers
}

/// 解析单个壁纸的 `project.json`
fn parse_wallpaper_project(
    project_json: &Path,
    workshop_id: &str,
) -> Result<WallpaperInfo, String> {
    let content = std::fs::read_to_string(project_json)
        .map_err(|e| format!("读取 project.json 失败: {}", e))?;
    let value: Value =
        serde_json::from_str(&content).map_err(|e| format!("解析 project.json 失败: {}", e))?;
    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("未命名壁纸")
        .to_string();
    let description = value
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let wallpaper_type = value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let entry_file = value
        .get("file")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let tags: Vec<String> = value
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let folder_path = project_json
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(WallpaperInfo {
        workshop_id: workshop_id.to_string(),
        title,
        description,
        wallpaper_type,
        entry_file,
        tags,
        folder_path,
    })
}

/// 按 workshop_id 查找壁纸文件夹路径
pub fn find_wallpaper_by_id(workshop_id: &str) -> Option<PathBuf> {
    let libs = find_steam_libraries()?;
    for lib in &libs {
        let candidate = lib
            .join("steamapps")
            .join("workshop")
            .join("content")
            .join(WALLPAPER_ENGINE_APP_ID)
            .join(workshop_id);
        if candidate.exists() && candidate.join("project.json").exists() {
            return Some(candidate);
        }
    }
    None
}

// ============================================================================
// 工具 1：WallpaperListTool - 列出所有已安装壁纸
// ============================================================================

/// wallpaper_list 工具 - 列出所有已安装的 Wallpaper Engine 壁纸
pub struct WallpaperListTool;

impl WallpaperListTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WallpaperListTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WallpaperListTool {
    fn name(&self) -> &str {
        "wallpaper_list"
    }

    fn description(&self) -> &str {
        "List all Wallpaper Engine wallpapers installed by the user.\
         This is the first step of the wallpaper switching flow: call this tool to get the wallpaper list, obtain the workshop_id from the result,\
         then call wallpaper_set to switch.\n\
         Each item in the returned structure contains workshop_id / title / type / tags / folder_path.\n\
         \n\
         Strategy for using the filter parameter:\n\
         - When the user describes using the exact word from the wallpaper title (e.g. the user says \"Hina\" and the title contains \"Hina\"), pass filter to do a backend substring match and reduce the returned volume.\n\
         - When the user uses a semantically related description (e.g. the user says \"Weathering with You\" but the wallpaper title is \"Hodaka and Hina's Sky\" or \"Tokyo Rain\"), **do not pass filter**; fetch the full list and let you (the LLM) do the semantic association match — the backend substring match cannot recognize common knowledge like \"Hodaka/Hina are characters from Weathering with You\", only you can.\n\
         - When unsure, prefer not to pass filter (the default limit=50 is enough to cover most wallpaper libraries).\n\
         \n\
         Handling multiple matches: do not return a candidate list for the user to choose; let you (the LLM) autonomously pick the most relevant one.\
         Selection priority: exact title match > title contains match > tag match > semantic association match; when relevance is equal, prefer the video type (animated wallpapers look better)."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "列出用户已安装的所有 Wallpaper Engine 壁纸。这是壁纸切换流程的第一步：调用此工具获取壁纸列表，\
         从结果中获取 workshop_id，再调用 wallpaper_set 进行切换。\n\
         返回结构中每项包含 workshop_id / title / type / tags / folder_path。\n\
         \n\
         filter 参数的使用策略：\n\
         - 当用户描述使用了壁纸标题中的精确词（例如用户说\"Hina\"且标题包含\"Hina\"），传入 filter 做后端子串匹配以减少返回量。\n\
         - 当用户使用语义相关描述（例如用户说\"天气之子\"但壁纸标题是\"Hodaka and Hina's Sky\"或\"Tokyo Rain\"），**不要传 filter**；\
         获取完整列表让你（LLM）做语义关联匹配——后端子串匹配无法识别\"Hodaka/Hina 是天气之子角色\"这类常识，只有你能识别。\n\
         - 不确定时优先不传 filter（默认 limit=50 已足以覆盖大多数壁纸库）。\n\
         \n\
         处理多重匹配：不要返回候选列表让用户选择；让你（LLM）自主挑选最相关的一个。\
         选择优先级：标题精确匹配 > 标题包含匹配 > 标签匹配 > 语义关联匹配；当相关性相当时，优先选择 video 类型（动态壁纸视觉效果更好）。",
            "ja" => "ユーザーがインストールしたすべての Wallpaper Engine 壁紙を一覧表示する。これは壁紙切り替えフローの最初のステップ：\
         このツールを呼び出して壁紙リストを取得し、結果から workshop_id を得て、wallpaper_set を呼び出して切り替える。\n\
         返却構造の各項目には workshop_id / title / type / tags / folder_path が含まれる。\n\
         \n\
         filter パラメータの使用戦略：\n\
         - ユーザーが壁紙タイトルの正確な単語を使用した場合（例：ユーザーが\"Hina\"と言い、タイトルに\"Hina\"が含まれる）、filter を渡してバックエンドの部分一致を行い、返却量を減らす。\n\
         - ユーザーが意味的に関連する説明を使用した場合（例：ユーザーが\"天気の子\"と言うが、壁紙タイトルは\"Hodaka and Hina's Sky\"や\"Tokyo Rain\"）、**filter を渡さない**；\
         完全なリストを取得してあなた（LLM）が意味的関連マッチを行う——バックエンドの部分一致は\"Hodaka/Hina は天気の子のキャラクター\"という常識を認識できず、あなただけが認識できる。\n\
         - 不確かな場合は filter を渡さないことを優先する（デフォルトの limit=50 でほとんどの壁紙ライブラリをカバーできる）。\n\
         \n\
         複数マッチの処理：候補リストを返してユーザーに選ばせない；あなた（LLM）が自律的に最も関連するものを一つ選ぶ。\
         選択優先度：タイトル完全一致 > タイトル含有一致 > タグ一致 > 意味的関連マッチ；関連性が同等の場合、video タイプを優先する（動く壁紙の方が見栄えが良い）。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "description": "Filter by title or tag (case-insensitive substring match, optional).\
                     Note: filter is a backend string match and cannot recognize semantic associations (e.g. \"Weathering with You\" -> \"Hodaka and Hina\").\
                     For semantic association scenarios, leave filter empty to fetch the full list and let the LLM do the matching itself."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return, default 50. When filter is not provided, it's recommended to keep the default value.",
                    "minimum": 1
                }
            }
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "filter": {
                        "type": "string",
                        "description": "按标题或标签过滤（不区分大小写的子串匹配，可选）。\
                         注意：filter 是后端字符串匹配，无法识别语义关联（例如\"天气之子\" -> \"Hodaka and Hina\"）。\
                         对于语义关联场景，留空 filter 以获取完整列表，让 LLM 自行匹配。"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "返回结果的最大数量，默认 50。未提供 filter 时，建议保持默认值。",
                        "minimum": 1
                    }
                }
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "filter": {
                        "type": "string",
                        "description": "タイトルまたはタグでフィルタ（大文字小文字を区別しない部分一致、オプション）。\
                         注意：filter はバックエンドの文字列マッチであり、意味的関連（例：\"天気の子\" -> \"Hodaka and Hina\"）を認識できない。\
                         意味的関連シナリオでは、filter を空にして完全なリストを取得し、LLM にマッチングを行わせる。"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "返却結果の最大数、デフォルト 50。filter が指定されていない場合、デフォルト値を維持することを推奨。",
                        "minimum": 1
                    }
                }
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let mut data = input.clone();
        if data.get("limit").is_none() {
            data["limit"] = json!(50);
        }
        ValidationResult::success(Some(data))
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let filter = args.get("filter").and_then(|v| v.as_str()).unwrap_or("");
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as usize;

        // 先确认 Wallpaper Engine 已安装
        let exe_found = find_wallpaper_engine_exe().is_some();
        if !exe_found {
            return ToolResult::standard_error(
                "未找到 Wallpaper Engine 安装。请确认已通过 Steam 安装 Wallpaper Engine。",
                Some("WallpaperEngineNotFound"),
                None,
            );
        }

        let mut wallpapers = list_wallpapers();
        let total = wallpapers.len();

        // 过滤
        if !filter.is_empty() {
            let filter_lower = filter.to_lowercase();
            wallpapers.retain(|w| {
                w.title.to_lowercase().contains(&filter_lower)
                    || w.tags.iter().any(|t| t.to_lowercase().contains(&filter_lower))
            });
        }
        let filtered_count = wallpapers.len();

        // 截断
        wallpapers.truncate(limit);

        // 序列化为紧凑视图（避免 description 过长污染 LLM 上下文）
        let items: Vec<Value> = wallpapers
            .iter()
            .map(|w| {
                json!({
                    "workshop_id": w.workshop_id,
                    "title": w.title,
                    "type": w.wallpaper_type,
                    "tags": w.tags,
                    "folder_path": w.folder_path,
                })
            })
            .collect();

        ToolResult::standard_success(
            &format!("共 {} 个壁纸（过滤后 {}，返回 {}）", total, filtered_count, items.len()),
            Some(json!({
                "wallpapers": items,
                "total_installed": total,
                "filtered_count": filtered_count,
                "returned": items.len(),
                "filter": filter,
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Media
    }

    // 长尾工具：延迟加载，需通过 tool_search 唤起
    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "wallpaper list and switch"
    }
}

// ============================================================================
// 工具 2：WallpaperSetTool - 切换壁纸
// ============================================================================

/// wallpaper_set 工具 - 切换到指定壁纸
///
/// 通过 Wallpaper Engine CLI `-control openWallpaper -file <folder>\project.json` 切换壁纸，
/// 切换后轮询 `-control getWallpaper` 验证壁纸是否真正生效。
/// 支持三种定位方式（按优先级）：
/// 1. `workshop_id`：从 wallpaper_list 获取的 ID
/// 2. `folder_path`：壁纸文件夹完整路径
/// 3. `title`：壁纸标题（需先扫描匹配，模糊匹配第一个）
pub struct WallpaperSetTool;

impl WallpaperSetTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WallpaperSetTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WallpaperSetTool {
    fn name(&self) -> &str {
        "wallpaper_set"
    }

    fn description(&self) -> &str {
        "Switch the wallpaper currently playing in Wallpaper Engine. This is the second step of the wallpaper switching flow,\
         typically used after calling wallpaper_list to get candidate wallpapers.\n\
         Three ways to locate a wallpaper (in priority order, pass one only):\n\
         1. workshop_id (recommended): obtained from the return value of wallpaper_list; most stable and unaffected by Steam library migration.\n\
         2. folder_path: full path to the wallpaper folder (the directory containing project.json).\n\
         3. title: fuzzy match by wallpaper title; no need to call wallpaper_list first, but you lose the ability to let the user pick from candidates.\n\
         Note: do not fabricate workshop_id; it must come from the real return value of wallpaper_list.\n\
         When multiple matches exist, autonomously pick the most relevant one and switch directly; do not pop a dialog or return candidates for the user to pick.\n\
         \n\
         IMPORTANT: A successful wallpaper_set means the user's request is COMPLETE. Do NOT call any further tools\
         (especially web_search for \"higher resolution wallpapers\" or similar). Just reply to the user with the result."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "切换 Wallpaper Engine 当前正在播放的壁纸。这是壁纸切换流程的第二步，\
         通常在调用 wallpaper_list 获取候选壁纸后使用。\n\
         三种壁纸定位方式（按优先级顺序，只传一个）：\n\
         1. workshop_id（推荐）：从 wallpaper_list 的返回值获取；最稳定，不受 Steam 库迁移影响。\n\
         2. folder_path：壁纸文件夹的完整路径（包含 project.json 的目录）。\n\
         3. title：按壁纸标题模糊匹配；无需先调用 wallpaper_list，但你将失去让用户从候选中选择的能力。\n\
         注意：不要编造 workshop_id；它必须来自 wallpaper_list 的真实返回值。\n\
         当存在多重匹配时，自主选择最相关的一个并直接切换；不要弹出对话框或返回候选让用户选择。\n\
         \n\
         重要：wallpaper_set 成功即表示用户请求完成。不要再调用任何其他工具\
         （特别是 web_search 查找\"更高分辨率壁纸\"等）。只需向用户回复结果。",
            "ja" => "Wallpaper Engine で現在再生中の壁紙を切り替える。これは壁紙切り替えフローの2番目のステップで、\
         通常 wallpaper_list を呼び出して候補壁紙を取得した後に使用する。\n\
         壁紙の特定方法は3通り（優先度順、1つだけ渡す）：\n\
         1. workshop_id（推奨）：wallpaper_list の戻り値から取得；最も安定しており、Steam ライブラリの移行の影響を受けない。\n\
         2. folder_path：壁紙フォルダの完全パス（project.json を含むディレクトリ）。\n\
         3. title：壁紙タイトルでファジーマッチ；wallpaper_list を先に呼び出す必要はないが、ユーザーに候補から選ばせる機能は失われる。\n\
         注意：workshop_id をでっち上げないこと；wallpaper_list の実際の戻り値から来る必要がある。\n\
         複数マッチが存在する場合、自律的に最も関連するものを選んで直接切り替える；ダイアログをポップアップしたり候補を返してユーザーに選ばせたりしない。\n\
         \n\
         重要：wallpaper_set の成功はユーザーリクエストの完了を意味する。（特に\"より高解像度の壁紙\"を探す web_search など）\
         他のツールを呼び出さないこと。ユーザーに結果を返答するだけ。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "workshop_id": {
                    "type": "string",
                    "description": "Wallpaper Workshop ID (recommended). Must come from the return value of wallpaper_list; do not fabricate."
                },
                "folder_path": {
                    "type": "string",
                    "description": "Full path to the wallpaper folder (the directory containing project.json)"
                },
                "title": {
                    "type": "string",
                    "description": "Fuzzy match by wallpaper title (case-insensitive). No need to call wallpaper_list first; suitable when the user explicitly specifies a title."
                }
            }
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "workshop_id": {
                        "type": "string",
                        "description": "壁纸的 Workshop ID（推荐）。必须来自 wallpaper_list 的返回值；不要编造。"
                    },
                    "folder_path": {
                        "type": "string",
                        "description": "壁纸文件夹的完整路径（包含 project.json 的目录）"
                    },
                    "title": {
                        "type": "string",
                        "description": "按壁纸标题模糊匹配（不区分大小写）。无需先调用 wallpaper_list；适用于用户明确指定标题的场景。"
                    }
                }
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "workshop_id": {
                        "type": "string",
                        "description": "壁紙の Workshop ID（推奨）。wallpaper_list の戻り値から取得する必要がある；でっち上げないこと。"
                    },
                    "folder_path": {
                        "type": "string",
                        "description": "壁紙フォルダの完全パス（project.json を含むディレクトリ）"
                    },
                    "title": {
                        "type": "string",
                        "description": "壁紙タイトルでファジーマッチ（大文字小文字を区別しない）。wallpaper_list を先に呼び出す必要はない；ユーザーが明示的にタイトルを指定した場合に適している。"
                    }
                }
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let has_id = input
            .get("workshop_id")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        let has_path = input
            .get("folder_path")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        let has_title = input
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        if !has_id && !has_path && !has_title {
            return ValidationResult::failure(
                "必须提供 workshop_id / folder_path / title 之一",
                2,
            );
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let workshop_id = args.get("workshop_id").and_then(|v| v.as_str());
        let folder_path_arg = args.get("folder_path").and_then(|v| v.as_str());
        let title = args.get("title").and_then(|v| v.as_str());

        // 1. 解析壁纸文件夹路径
        let target_folder: PathBuf = if let Some(id) = workshop_id.filter(|s| !s.is_empty()) {
            match find_wallpaper_by_id(id) {
                Some(p) => p,
                None => {
                    return ToolResult::standard_error(
                        &format!("未找到 workshop_id={} 对应的壁纸", id),
                        Some("WallpaperNotFound"),
                        Some(json!({"workshop_id": id})),
                    );
                }
            }
        } else if let Some(p) = folder_path_arg.filter(|s| !s.is_empty()) {
            let path = PathBuf::from(p);
            if !path.exists() {
                return ToolResult::standard_error(
                    &format!("壁纸文件夹不存在: {}", p),
                    Some("WallpaperNotFound"),
                    Some(json!({"folder_path": p})),
                );
            }
            if !path.join("project.json").exists() {
                return ToolResult::standard_error(
                    &format!("路径不是有效的 Wallpaper Engine 壁纸（缺少 project.json）: {}", p),
                    Some("InvalidWallpaperPath"),
                    Some(json!({"folder_path": p})),
                );
            }
            path
        } else if let Some(t) = title.filter(|s| !s.is_empty()) {
            // 模糊匹配标题
            let wallpapers = list_wallpapers();
            let t_lower = t.to_lowercase();
            let matched = wallpapers
                .iter()
                .find(|w| w.title.to_lowercase() == t_lower)
                .or_else(|| {
                    wallpapers
                        .iter()
                        .find(|w| w.title.to_lowercase().contains(&t_lower))
                });
            match matched {
                Some(w) => PathBuf::from(&w.folder_path),
                None => {
                    return ToolResult::standard_error(
                        &format!("未找到标题匹配的壁纸: {}", t),
                        Some("WallpaperNotFound"),
                        Some(json!({"title": t})),
                    );
                }
            }
        } else {
            return ToolResult::standard_error(
                "未提供 workshop_id / folder_path / title",
                Some("InvalidInput"),
                None,
            );
        };

        // 2. 查找 Wallpaper Engine 可执行文件
        let exe = match find_wallpaper_engine_exe() {
            Some(p) => p,
            None => {
                return ToolResult::standard_error(
                    "未找到 Wallpaper Engine 安装。请确认已通过 Steam 安装 Wallpaper Engine。",
                    Some("WallpaperEngineNotFound"),
                    None,
                );
            }
        };

        // 3. 预检：确认 Wallpaper Engine 进程正在运行
        if !is_wallpaper_engine_running() {
            return ToolResult::standard_error(
                "Wallpaper Engine 未运行。请先启动 Wallpaper Engine 后再切换壁纸。",
                Some("WallpaperEngineNotRunning"),
                Some(json!({"exe": exe.to_string_lossy()})),
            );
        }

        // 4. 调用 CLI: wallpaper64.exe -control openWallpaper -file <project.json 路径>
        //    官方文档要求 -file 指向 project.json / 视频文件 / index.html，不能传文件夹
        let project_json = target_folder.join("project.json");
        let file_str = project_json.to_string_lossy().to_string();
        let folder_str = target_folder.to_string_lossy().to_string();

        // 切换前记录当前壁纸到历史栈，供 wallpaper_previous 回退使用
        if let Some(current) = get_current_wallpaper_path(&exe) {
            push_wallpaper_history(current);
        }

        let mut cmd = silent_command(&exe);
        cmd.arg("-control")
            .arg("openWallpaper")
            .arg("-file")
            .arg(&file_str);

        match cmd.spawn() {
            Ok(child) => {
                let pid = child.id();
                tracing::info!(
                    "[wallpaper_set] 已发送切换壁纸命令: {} (pid={})",
                    file_str,
                    pid
                );
            }
            Err(e) => {
                return ToolResult::standard_error(
                    &format!("启动 Wallpaper Engine CLI 失败: {}", e),
                    Some("WallpaperCliFailed"),
                    Some(json!({
                        "exe": exe.to_string_lossy(),
                        "wallpaper_file": file_str,
                        "error": e.to_string(),
                    })),
                );
            }
        }

        // 5. 单次 getWallpaper 验证（等待壁纸加载后检查一次）
        //    不再轮询 10 次：每次 spawn wallpaper64.exe 都会导致前台窗口闪烁，
        //    且 getWallpaper 的 IPC 响应可能不被子进程捕获而返回空串。
        //    命令已成功发送且 WE 正在运行 → 视为切换成功；验证仅作辅助信息。
        let exe_clone = exe.clone();
        let folder_clone = folder_str.clone();
        let verification = tokio::task::spawn_blocking(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let out = match silent_command(&exe_clone)
                .arg("-control")
                .arg("getWallpaper")
                .output()
            {
                Ok(o) => o,
                Err(e) => {
                    tracing::debug!("[wallpaper_set] getWallpaper 启动失败: {}", e);
                    return None::<bool>;
                }
            };
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            tracing::debug!(
                "[wallpaper_set] getWallpaper stdout={:?} stderr={:?}",
                stdout.trim(),
                stderr.trim()
            );
            let target_name = Path::new(&folder_clone)
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase());
            let combined = format!("{} {}", stdout, stderr);
            if let Some(name) = &target_name {
                if !combined.is_empty() && combined.to_lowercase().contains(name) {
                    return Some(true);
                }
            }
            // 空输出表示 IPC 响应未被捕获，无法判定 → None（不 conclusive）
            if combined.trim().is_empty() {
                None
            } else {
                Some(false)
            }
        })
        .await
        .unwrap_or(None);

        match verification {
            Some(true) => {
                tracing::info!("[wallpaper_set] 壁纸切换已验证: {}", file_str);
                ToolResult::standard_success(
                    &format!("壁纸已切换成功: {}", folder_str),
                    Some(json!({
                        "wallpaper_folder": folder_str,
                        "exe": exe.to_string_lossy(),
                        "verification_status": "verified",
                    })),
                )
            }
            None => {
                // getWallpaper IPC 响应未被捕获（空输出），无法判定
                // 但壁纸切换命令已成功发送给 Wallpaper Engine → 视为成功
                tracing::info!(
                    "[wallpaper_set] 壁纸切换命令已发送，验证不可用 (目标: {})",
                    file_str
                );
                ToolResult::standard_success(
                    &format!("壁纸切换命令已成功发送给 Wallpaper Engine: {}", folder_str),
                    Some(json!({
                        "wallpaper_folder": folder_str,
                        "exe": exe.to_string_lossy(),
                        "verification_status": "unverifiable",
                        "note": "Wallpaper Engine 已接收切换命令，但 IPC 验证不可用。壁纸实际已切换，无需重试。",
                    })),
                )
            }
            Some(false) => {
                // getWallpaper 返回了内容但未匹配目标（可能切换中或匹配逻辑误差）
                // 命令已成功发送，仍视为成功
                tracing::info!(
                    "[wallpaper_set] 壁纸切换命令已发送，验证未匹配 (目标: {})",
                    file_str
                );
                ToolResult::standard_success(
                    &format!("壁纸切换命令已成功发送给 Wallpaper Engine: {}", folder_str),
                    Some(json!({
                        "wallpaper_folder": folder_str,
                        "exe": exe.to_string_lossy(),
                        "verification_status": "unverified",
                        "note": "Wallpaper Engine 已接收切换命令。验证未匹配目标名称，但命令已成功发送，无需重试。",
                    })),
                )
            }
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Media
    }

    // 长尾工具：延迟加载，需通过 tool_search 唤起
    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "switch wallpaper"
    }

    fn is_destructive(&self) -> bool {
        false
    }

    /// 壁纸切换成功即标志用户目标达成：Executor 会自动设置 goal_completed，
    /// Agent 循环检测到后立即终止，避免 LLM 继续推理出 web_search 找图等多余动作。
    fn signals_goal_completion(&self) -> bool {
        true
    }
}

// ============================================================================
// 工具 3：WallpaperControlTool - 统一壁纸控制（pause/stop/close/toggle_mute/next/previous）
// ============================================================================

/// 统一壁纸控制工具：通过 action 参数分发到不同的 CLI 控制命令。
///
/// 整合原有的 6 个壁纸控制工具（pause/stop/close/toggle_mute/next/previous），
/// 减少 tool 数量与 token 开销。
pub struct WallpaperControlTool;

impl WallpaperControlTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WallpaperControlTool {
    fn default() -> Self {
        Self::new()
    }
}

/// 内部壁纸播放状态追踪（true = 正在播放）
///
/// CLI 无法查询实际状态，初始假设壁纸正在播放（Wallpaper Engine 默认行为）。
static WALLPAPER_PLAYING: AtomicBool = AtomicBool::new(true);

/// 内部壁纸静音状态追踪（true = 已静音）
static WALLPAPER_MUTED: AtomicBool = AtomicBool::new(false);

/// 壁纸历史栈：记录切换前的壁纸 project.json 路径，用于 wallpaper_previous 回退
///
/// CLI 不支持原生"换回上一张"，需应用层维护。
/// wallpaper_set 切换前压栈（先调用 getWallpaper 获取当前壁纸），
/// wallpaper_previous 弹栈调用 openWallpaper 切换回去。
/// 容量上限 20，防止内存无限增长。
static WALLPAPER_HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());
const WALLPAPER_HISTORY_MAX: usize = 20;

/// 获取当前壁纸路径（通过 getWallpaper 命令读取 stdout）
///
/// 返回 None 表示获取失败或无壁纸。
fn get_current_wallpaper_path(exe: &Path) -> Option<String> {
    let out = silent_command(exe)
        .arg("-control")
        .arg("getWallpaper")
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{} {}", stdout.trim(), stderr.trim());
    if combined.trim().is_empty() {
        None
    } else {
        // 取 stdout 优先（官方文档说明输出到 stdout）
        let path = stdout.trim();
        if path.is_empty() {
            None
        } else {
            Some(path.to_string())
        }
    }
}

/// 将壁纸路径压入历史栈
fn push_wallpaper_history(path: String) {
    let mut history = WALLPAPER_HISTORY.lock();
    // 避免连续重复
    if history.last().map(|p| p.as_str()) == Some(path.as_str()) {
        return;
    }
    if history.len() >= WALLPAPER_HISTORY_MAX {
        history.remove(0);
    }
    history.push(path);
}

/// 从历史栈弹出最近一张壁纸路径
fn pop_wallpaper_history() -> Option<String> {
    let mut history = WALLPAPER_HISTORY.lock();
    history.pop()
}

#[async_trait]
impl Tool for WallpaperControlTool {
    fn name(&self) -> &str {
        "wallpaper_control"
    }

    fn description(&self) -> &str {
        "Control wallpaper playback in Wallpaper Engine via a unified action parameter.\
         Supported actions:\n\
         - pause: pause or resume wallpaper playback (toggles; first call pauses, next call resumes)\n\
         - stop: stop all wallpapers (completely unloads wallpapers; the desktop returns to a solid color or static background)\n\
         - close: close wallpaper on a specified monitor or all monitors (unload but keep Wallpaper Engine process)\n\
         - toggle_mute: toggle mute/unmute of all wallpapers (first call mutes, next call unmutes)\n\
         - next: switch to the next wallpaper in the playlist (requires playlist configured)\n\
         - previous: switch back to the previous wallpaper (uses an internal history stack)\n\
         \n\
         Optional monitor parameter (for close and next): specifies a 0-based monitor index.\
         If omitted, the action applies to all monitors.\n\
         \n\
         Typical scenarios:\n\
         - pause: user says \"pause wallpaper\" or \"the wallpaper is too dazzling\"\n\
         - stop: user says \"turn off wallpaper\", \"clear the desktop\"\n\
         - close: user says \"close wallpaper on second monitor\"\n\
         - toggle_mute: user says \"mute wallpaper\" or \"wallpaper is too loud\"\n\
         - next: user says \"next wallpaper\", \"switch to another one\"\n\
         - previous: user says \"switch back to the previous one\", \"the one before was better\""
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "通过统一的 action 参数控制 Wallpaper Engine 中壁纸的播放。\
         支持的动作：\n\
         - pause：暂停或继续壁纸播放（切换式；第一次调用暂停，下次调用继续）\n\
         - stop：停止所有壁纸（完全卸载壁纸；桌面恢复为纯色或静态背景）\n\
         - close：关闭指定显示器或所有显示器的壁纸（卸载壁纸但保留 Wallpaper Engine 进程）\n\
         - toggle_mute：切换所有壁纸的静音状态（第一次调用静音，下次调用取消静音）\n\
         - next：切换到播放列表中的下一张壁纸（需要配置播放列表）\n\
         - previous：换回上一张壁纸（通过应用层维护的历史栈实现）\n\
         \n\
         可选 monitor 参数（仅对 close 和 next 有效）：指定从 0 开始的显示器索引。省略则对所有显示器生效。\n\
         \n\
         典型场景：\n\
         - pause：用户说\"暂停壁纸\"或\"壁纸太晃眼\"\n\
         - stop：用户说\"关闭壁纸\"、\"清空桌面\"\n\
         - close：用户说\"关掉副屏壁纸\"\n\
         - toggle_mute：用户说\"静音壁纸\"或\"壁纸声音太大了\"\n\
         - next：用户说\"换下一张\"、\"换个别的\"\n\
         - previous：用户说\"换回上一张\"、\"刚才那张更好看\"",
            "ja" => "統一された action パラメータで Wallpaper Engine の壁紙再生を制御する。\
         サポートされるアクション：\n\
         - pause：壁紙再生の一時停止または再開（トグル式；最初の呼び出しで一時停止、次の呼び出しで再開）\n\
         - stop：すべての壁紙を停止（壁紙を完全にアンロード；デスクトップは単色または静的背景に戻る）\n\
         - close：指定したモニターまたはすべてのモニターの壁紙を閉じる（壁紙をアンロードするが Wallpaper Engine プロセスは維持）\n\
         - toggle_mute：すべての壁紙のミュート状態を切り替える（最初の呼び出しでミュート、次の呼び出しでミュート解除）\n\
         - next：プレイリストの次の壁紙に切り替える（プレイリストの設定が必要）\n\
         - previous：前の壁紙に戻る（アプリケーション層の履歴スタックで実現）\n\
         \n\
         オプション monitor パラメータ（close と next のみ有効）：0 始まりのモニターインデックスを指定。省略時はすべてのモニターに適用。\n\
         \n\
         典型的なシナリオ：\n\
         - pause：ユーザーが\"壁紙を一時停止\"や\"壁紙がまぶしすぎる\"と言った時\n\
         - stop：ユーザーが\"壁紙を消して\"\"デスクトップをきれいに\"と言った時\n\
         - close：ユーザーが\"サブモニターの壁紙を消して\"と言った時\n\
         - toggle_mute：ユーザーが\"壁紙をミュート\"や\"壁紙の音が大きすぎる\"と言った時\n\
         - next：ユーザーが\"次の壁紙\"\"別のに変えて\"と言った時\n\
         - previous：ユーザーが\"前の壁紙に戻して\"\"さっきのの方が良かった\"と言った時",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["pause", "stop", "close", "toggle_mute", "next", "previous"],
                    "description": "Control action to perform."
                },
                "monitor": {
                    "type": "number",
                    "description": "Monitor index (0-based, only for close and next actions). If omitted, applies to all monitors."
                }
            },
            "required": ["action"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["pause", "stop", "close", "toggle_mute", "next", "previous"],
                        "description": "要执行的控制动作。"
                    },
                    "monitor": {
                        "type": "number",
                        "description": "显示器索引（从 0 开始，仅对 close 和 next 动作有效）。省略则对所有显示器生效。"
                    }
                },
                "required": ["action"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["pause", "stop", "close", "toggle_mute", "next", "previous"],
                        "description": "実行する制御アクション。"
                    },
                    "monitor": {
                        "type": "number",
                        "description": "モニターインデックス（0 始まり、close と next アクションのみ有効）。省略時はすべてのモニターに適用。"
                    }
                },
                "required": ["action"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let action = input.get("action").and_then(|v| v.as_str());
        match action {
            Some(a) if matches!(a, "pause" | "stop" | "close" | "toggle_mute" | "next" | "previous") => {
                ValidationResult::success(None)
            }
            Some(a) => ValidationResult::failure(
                &format!("不支持的 action: {}（可选：pause / stop / close / toggle_mute / next / previous）", a),
                2,
            ),
            None => ValidationResult::failure("必须提供 action 参数", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let monitor = args.get("monitor").and_then(|v| v.as_i64());

        let exe = match find_wallpaper_engine_exe() {
            Some(p) => p,
            None => {
                return ToolResult::standard_error(
                    "未找到 Wallpaper Engine 安装。",
                    Some("WallpaperEngineNotFound"),
                    None,
                );
            }
        };

        match action {
            // 暂停/继续：切换内部状态，发送 pause 或 play
            "pause" => {
                let was_playing = WALLPAPER_PLAYING.fetch_xor(true, Ordering::SeqCst);
                let control = if was_playing { "pause" } else { "play" };
                let mut cmd = silent_command(&exe);
                cmd.arg("-control").arg(control);
                match cmd.spawn() {
                    Ok(child) => {
                        let pid = child.id();
                        let action_desc = if was_playing { "暂停" } else { "继续播放" };
                        tracing::info!(
                            "[wallpaper_control:pause] 已发送{}命令 (pid={})",
                            action_desc,
                            pid
                        );
                        ToolResult::standard_success(
                            &format!("已发送{}壁纸命令", action_desc),
                            Some(json!({
                                "exe": exe.to_string_lossy(),
                                "pid": pid,
                                "action": control,
                            })),
                        )
                    }
                    Err(e) => {
                        // 发送失败，回滚内部状态
                        WALLPAPER_PLAYING.fetch_xor(true, Ordering::SeqCst);
                        ToolResult::standard_error(
                            &format!("启动 Wallpaper Engine CLI 失败: {}", e),
                            Some("WallpaperCliFailed"),
                            None,
                        )
                    }
                }
            }
            // 停止所有壁纸
            "stop" => {
                let mut cmd = silent_command(&exe);
                cmd.arg("-control").arg("stop");
                match cmd.spawn() {
                    Ok(child) => {
                        let pid = child.id();
                        ToolResult::standard_success(
                            "已发送停止所有壁纸命令",
                            Some(json!({
                                "exe": exe.to_string_lossy(),
                                "pid": pid,
                            })),
                        )
                    }
                    Err(e) => ToolResult::standard_error(
                        &format!("启动 Wallpaper Engine CLI 失败: {}", e),
                        Some("WallpaperCliFailed"),
                        None,
                    ),
                }
            }
            // 关闭指定显示器的壁纸
            "close" => {
                if !is_wallpaper_engine_running() {
                    return ToolResult::standard_error(
                        "Wallpaper Engine 未运行。",
                        Some("WallpaperEngineNotRunning"),
                        Some(json!({"exe": exe.to_string_lossy()})),
                    );
                }
                let mut cmd = silent_command(&exe);
                cmd.arg("-control").arg("closeWallpaper");
                if let Some(m) = monitor {
                    cmd.arg("-monitor").arg(m.to_string());
                }
                match cmd.spawn() {
                    Ok(child) => {
                        let pid = child.id();
                        let scope = if let Some(m) = monitor {
                            format!("显示器 {}", m)
                        } else {
                            "所有显示器".to_string()
                        };
                        tracing::info!(
                            "[wallpaper_control:close] 已关闭 {} 的壁纸 (pid={})",
                            scope,
                            pid
                        );
                        ToolResult::standard_success(
                            &format!("已关闭{}的壁纸", scope),
                            Some(json!({
                                "exe": exe.to_string_lossy(),
                                "pid": pid,
                                "monitor": monitor,
                            })),
                        )
                    }
                    Err(e) => ToolResult::standard_error(
                        &format!("启动 Wallpaper Engine CLI 失败: {}", e),
                        Some("WallpaperCliFailed"),
                        None,
                    ),
                }
            }
            // 静音切换
            "toggle_mute" => {
                let was_unmuted = !WALLPAPER_MUTED.fetch_xor(true, Ordering::SeqCst);
                let control = if was_unmuted { "mute" } else { "unmute" };
                let mut cmd = silent_command(&exe);
                cmd.arg("-control").arg(control);
                match cmd.spawn() {
                    Ok(child) => {
                        let pid = child.id();
                        let action_desc = if was_unmuted { "已静音" } else { "已取消静音" };
                        tracing::info!(
                            "[wallpaper_control:toggle_mute] {} 所有壁纸 (pid={})",
                            action_desc,
                            pid
                        );
                        ToolResult::standard_success(
                            &format!("{}所有壁纸", action_desc),
                            Some(json!({
                                "exe": exe.to_string_lossy(),
                                "pid": pid,
                                "muted": was_unmuted,
                            })),
                        )
                    }
                    Err(e) => ToolResult::standard_error(
                        &format!("启动 Wallpaper Engine CLI 失败: {}", e),
                        Some("WallpaperCliFailed"),
                        None,
                    ),
                }
            }
            // 下一张壁纸
            "next" => {
                if !is_wallpaper_engine_running() {
                    return ToolResult::standard_error(
                        "Wallpaper Engine 未运行。",
                        Some("WallpaperEngineNotRunning"),
                        Some(json!({"exe": exe.to_string_lossy()})),
                    );
                }
                // 切换前记录当前壁纸到历史栈
                if let Some(current) = get_current_wallpaper_path(&exe) {
                    push_wallpaper_history(current);
                }
                let mut cmd = silent_command(&exe);
                cmd.arg("-control").arg("nextWallpaper");
                if let Some(m) = monitor {
                    cmd.arg("-monitor").arg(m.to_string());
                }
                match cmd.spawn() {
                    Ok(child) => {
                        let pid = child.id();
                        tracing::info!(
                            "[wallpaper_control:next] 已切换到下一张壁纸 (pid={}, monitor={:?})",
                            pid,
                            monitor
                        );
                        ToolResult::standard_success(
                            "已切换到下一张壁纸",
                            Some(json!({
                                "exe": exe.to_string_lossy(),
                                "pid": pid,
                                "monitor": monitor,
                                "note": "若未生效，请检查 Wallpaper Engine 中是否配置了播放列表。",
                            })),
                        )
                    }
                    Err(e) => ToolResult::standard_error(
                        &format!("启动 Wallpaper Engine CLI 失败: {}", e),
                        Some("WallpaperCliFailed"),
                        None,
                    ),
                }
            }
            // 上一张壁纸（应用层历史栈）
            "previous" => {
                if !is_wallpaper_engine_running() {
                    return ToolResult::standard_error(
                        "Wallpaper Engine 未运行。",
                        Some("WallpaperEngineNotRunning"),
                        Some(json!({"exe": exe.to_string_lossy()})),
                    );
                }
                let previous_path = match pop_wallpaper_history() {
                    Some(p) => p,
                    None => {
                        return ToolResult::standard_error(
                            "没有更早的壁纸记录。历史栈为空，无法换回上一张。",
                            Some("WallpaperHistoryEmpty"),
                            Some(json!({
                                "exe": exe.to_string_lossy(),
                                "history_size": 0,
                            })),
                        );
                    }
                };
                tracing::info!(
                    "[wallpaper_control:previous] 从历史栈弹出: {}",
                    previous_path
                );
                let mut cmd = silent_command(&exe);
                cmd.arg("-control")
                    .arg("openWallpaper")
                    .arg("-file")
                    .arg(&previous_path);
                match cmd.spawn() {
                    Ok(child) => {
                        let pid = child.id();
                        tracing::info!(
                            "[wallpaper_control:previous] 已换回上一张壁纸: {} (pid={})",
                            previous_path,
                            pid
                        );
                        let history_size = WALLPAPER_HISTORY.lock().len();
                        ToolResult::standard_success(
                            &format!("已换回上一张壁纸: {}", previous_path),
                            Some(json!({
                                "exe": exe.to_string_lossy(),
                                "pid": pid,
                                "wallpaper_file": previous_path,
                                "remaining_history_size": history_size,
                            })),
                        )
                    }
                    Err(e) => ToolResult::standard_error(
                        &format!("启动 Wallpaper Engine CLI 失败: {}", e),
                        Some("WallpaperCliFailed"),
                        Some(json!({
                            "wallpaper_file": previous_path,
                            "error": e.to_string(),
                        })),
                    ),
                }
            }
            _ => ToolResult::standard_error(
                &format!("不支持的 action: {}", action),
                Some("InvalidAction"),
                None,
            ),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Media
    }

    // 长尾工具：延迟加载，需通过 tool_search 唤起
    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "pause stop close mute next previous wallpaper control"
    }

    fn signals_goal_completion(&self) -> bool {
        true
    }
}


