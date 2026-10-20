//! 技能工具 - use_skill / create_skill
//!
//! 技能目录（<用户数据目录>/skills 的 *.md）默认只注入"名称+描述"到 prompt，
//! LLM 判断某项技能适用时调用 use_skill 获取技能正文，按其指引行动。
//! 这样技能正文不常驻上下文，按需激活，控制 token 开销。
//!
//! create_skill 是自进化闭环的写入侧：智能体把复用做法沉淀为技能文件，
//! 写入即注册（不等热重载），之后会话可通过 use_skill 激活。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};

/// use_skill 工具 - 按名称激活技能，返回技能正文供 LLM 遵循。
pub struct UseSkillTool;

impl UseSkillTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UseSkillTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for UseSkillTool {
    fn name(&self) -> &str {
        "use_skill"
    }

    fn description(&self) -> &str {
        "Activate a skill by name to get its full instructions, then follow them. \
         Available skills are listed in the '## 可用技能' prompt section. \
         Call this when a skill matches the current task, then act according to the returned content."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "按名称激活一项技能，获取其完整指引后照做。可用技能列在 prompt 的「## 可用技能」段落中。\
            当某项技能与当前任务匹配时调用本工具，然后按返回的技能正文行动。",
            "ja" => "スキル名を指定して有効化し、完全な指示を取得して従う。利用可能なスキルは prompt の「## 可用技能」\
            セクションに一覧表示される。現在のタスクに合うスキルがある場合にこのツールを呼び出し、\
            返された内容に沿って行動する。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "用一下那个技能\n调用技能\n跑一下这个技能",
            "en" => "use that skill\nrun this skill\napply the skill",
            "ja" => "そのスキルを使って\nスキルを実行して\n技能を適用して",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Skill name to activate (from the 可用技能 section)"
                }
            },
            "required": ["name"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "要激活的技能名称（来自「可用技能」段落）"
                    }
                },
                "required": ["name"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "有効化するスキル名（「可用技能」セクションから）"
                    }
                },
                "required": ["name"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        match input.get("name").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => ValidationResult::success(Some(json!({ "name": s.trim() }))),
            _ => ValidationResult::failure("name 是必填项", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _context: &ToolUseContext) -> PermissionResult {
        // 只读检索技能正文，无副作用，无需确认
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let char_id = context.char_id.clone();

        let Some(svc) = crate::cordis::global_ctx()
            .and_then(|ctx| ctx.get_service::<crate::skills::SkillService>())
        else {
            return ToolResult::standard_error("技能服务不可用", None, None);
        };

        // 限定当前角色可见的技能（全局 + 该角色 scoped）
        let skill = svc.list_for(&char_id).into_iter().find(|s| s.name == name);
        match skill {
            Some(s) => {
                let resources_hint = if s.resources.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n\n该技能目录内还带有以下支撑资源（当前任务确实需要时再用 read_file 读取，不必全部预读）：\n{}",
                        s.resources
                            .iter()
                            .map(|relative| format!("- {relative}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                };
                ToolResult::standard_success(
                    &format!(
                        "技能「{}」使用指导已加载；它不执行操作、不授予权限，也不安装工具。只使用已注册且获准的能力，缺少能力时说明限制：\n\n{}{}",
                        s.name, s.body, resources_hint
                    ),
                    Some(json!({
                        "name": s.name,
                        "activated": true,
                        "kind": "usage_guidance",
                        "executes": false,
                        "resources": s.resources,
                    })),
                )
            }
            None => {
                // 附带可见技能列表，方便 LLM 纠正名称后重试
                let available: Vec<String> =
                    svc.list_for(&char_id).iter().map(|s| s.name.clone()).collect();
                ToolResult::standard_error(
                    &format!("未找到技能「{}」。当前可用技能：{}", name, available.join("、")),
                    None,
                    None,
                )
            }
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    /// 权限风险等级：读取本地文件 / 持久化数据，无写入
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsRead
    }
}

/// create_skill 工具 - 智能体自主封装可复用技能（能力自进化闭环）。
///
/// 将 (名称, 描述, 正文) 以 front-matter Markdown 写入技能目录
/// `<用户数据目录>/skills/<name>.md`，并立即注册进 SkillService——
/// 无需等待 30s 目录热重载，写入后即可被 use_skill 激活。
///
/// 防护：
/// - 技能名仅允许字母/数字/`_`/`-`/中文（防路径穿越与非法文件名）
/// - 内置风格预设（`BUILTIN_SKILL_NAMES`）不可覆盖
/// - 声明 FsWrite 风险分级，受权限网关审批矩阵约束
pub struct CreateSkillTool;

impl CreateSkillTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CreateSkillTool {
    fn default() -> Self {
        Self::new()
    }
}

/// 解析 `resources` 入参：接受 `[{path, content}]`，校验路径白名单与配额。
///
/// 路径只允许 `references/` 或 `scripts/` 下的一层普通文件名；单文件 ≤256 KiB、
/// 合计 ≤1 MiB、条数 ≤20。
fn parse_resource_inputs(value: Option<&Value>) -> Result<Vec<(String, String)>, String> {
    const MAX_FILES: usize = 20;
    const MAX_FILE_BYTES: usize = 256 * 1024;
    const MAX_TOTAL_BYTES: usize = 1024 * 1024;
    let Some(value) = value else { return Ok(Vec::new()) };
    let Some(items) = value.as_array() else {
        return Err("resources 必须是 [{path, content}] 数组".into());
    };
    if items.len() > MAX_FILES {
        return Err(format!("resources 最多 {MAX_FILES} 个文件"));
    }
    let mut out: Vec<(String, String)> = Vec::with_capacity(items.len());
    let mut total = 0usize;
    for item in items {
        let path = item
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let content = item
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if !crate::skills::is_safe_skill_rel_path(&path) {
            return Err(format!(
                "资源路径「{path}」不合法：仅允许 references/ 或 scripts/ 下的一层文件名"
            ));
        }
        if content.len() > MAX_FILE_BYTES {
            return Err(format!("资源「{path}」超过 256 KiB"));
        }
        total = total.saturating_add(content.len());
        if total > MAX_TOTAL_BYTES {
            return Err("resources 合计超过 1 MiB".into());
        }
        out.push((path, content));
    }
    Ok(out)
}

#[async_trait]
impl Tool for CreateSkillTool {
    fn name(&self) -> &str {
        "create_skill"
    }

    fn description(&self) -> &str {
        "Create or update a reusable skill and register it immediately. New skills are written \
         as a folder `<name>/SKILL.md`; optional text resources go under `references/` or \
         `scripts/`. Use this to distill a reusable procedure into a named skill, so it appears \
         in the '## 可用技能' list and can be activated later via use_skill. Provide keywords \
         (synonyms, trigger phrases, related terms) so search_skill can recall it later. \
         Same-name skills are overwritten."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "创建或更新一项可复用技能，并立即注册。新技能以目录形态写入 `<name>/SKILL.md`，\
            可选文本资源放在 `references/` 或 `scripts/` 下。当你总结出一套值得复用的做法时，\
            把它沉淀为命名技能，技能会出现在「## 可用技能」列表中，之后通过 use_skill 激活。\
            请提供 keywords（同义词、触发短语、相关词），以便之后 search_skill 能用自然语言召回它。\
            同名技能会被覆盖更新。",
            "ja" => "再利用可能なスキルを作成・更新する：Markdown 形式の指示ファイルとしてスキルディレクトリに保存し、即座に登録する。\
            繰り返し使える手順（例：複数ステップのタスクの処理方法）を名前付きスキルとしてまとめると、\
            「## 可用技能」リストに表示され、後から use_skill で有効化できる。\
            keywords（同義語・トリガー句・関連語）を指定すると、後で search_skill が自然言語で呼び出せる。\
            同名スキルは上書きされる。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "把这个做成技能\n新建个技能\n以后都能这么用",
            "en" => "make this into a skill\ncreate a new skill\nsave this as a reusable skill",
            "ja" => "これをスキルにして\n新しいスキルを作って\n再利用できるようにして",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Skill name: letters, digits, '_', '-' or CJK only, no spaces/slashes (used as the .md filename)"
                },
                "description": {
                    "type": "string",
                    "description": "One-line description shown in the 可用技能 list"
                },
                "body": {
                    "type": "string",
                    "description": "Full skill instructions in markdown: what to do and how, written for your future self to follow"
                },
                "keywords": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional retrieval terms (synonyms, trigger phrases, related words) used by search_skill to recall this skill; not shown in the 可用技能 list"
                },
                "layout": {
                    "type": "string",
                    "enum": ["auto", "folder", "flat"],
                    "description": "Storage layout. 'auto' (default) updates an existing flat file, otherwise creates a folder; 'folder' forces <name>/SKILL.md; 'flat' forces <name>.md"
                },
                "resources": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Relative path under references/ or scripts/, single level (e.g. references/notes.md)" },
                            "content": { "type": "string" }
                        },
                        "required": ["path", "content"]
                    },
                    "description": "Optional text resources written alongside SKILL.md (folder layout only); existing files not mentioned are kept"
                }
            },
            "required": ["name", "description", "body"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "技能名：仅字母/数字/下划线/连字符/中文，不含空格与斜杠（将作为 .md 文件名）"
                    },
                    "description": {
                        "type": "string",
                        "description": "一句话描述，显示在「可用技能」列表中"
                    },
                    "body": {
                        "type": "string",
                        "description": "技能完整指引（Markdown）：做什么、怎么做，写给未来的自己照做"
                    },
                    "keywords": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "可选检索词（同义词、触发短语、相关词），供 search_skill 召回此技能；不显示在「可用技能」列表中"
                    },
                    "layout": {
                        "type": "string",
                        "enum": ["auto", "folder", "flat"],
                        "description": "存储布局：auto（默认）已有单文件则续写、否则建目录；folder 强制 <名称>/SKILL.md；flat 强制 <名称>.md"
                    },
                    "resources": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string", "description": "references/ 或 scripts/ 下的一层相对路径（如 references/notes.md）" },
                                "content": { "type": "string" }
                            },
                            "required": ["path", "content"]
                        },
                        "description": "可选文本资源，与 SKILL.md 一起写入（仅目录布局）；未提及的既有文件不会被删除"
                    }
                },
                "required": ["name", "description", "body"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "スキル名：英数字・_・-・漢字のみ、空白・スラッシュ不可（.md ファイル名として使用）"
                    },
                    "description": {
                        "type": "string",
                        "description": "一行説明、「可用技能」リストに表示される"
                    },
                    "body": {
                        "type": "string",
                        "description": "スキルの完全な指示（Markdown）：何を・どうするか、未来の自分のために書く"
                    },
                    "keywords": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "任意の検索語（同義語・トリガー句・関連語）。search_skill がこのスキルを召回するのに使用、「可用技能」リストには表示されない"
                    },
                    "layout": {
                        "type": "string",
                        "enum": ["auto", "folder", "flat"],
                        "description": "保存レイアウト：auto（既定）既存の単一ファイルを更新、無ければフォルダ作成；folder は <名前>/SKILL.md；flat は <名前>.md"
                    },
                    "resources": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string", "description": "references/ または scripts/ 直下の相対パス（例 references/notes.md）" },
                                "content": { "type": "string" }
                            },
                            "required": ["path", "content"]
                        },
                        "description": "SKILL.md と一緒に書き込む任意のテキスト資源（フォルダレイアウトのみ）。未指定の既存ファイルは削除されない"
                    }
                },
                "required": ["name", "description", "body"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let name = input.get("name").and_then(|v| v.as_str()).unwrap_or_default().trim();
        let description = input
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        let body = input.get("body").and_then(|v| v.as_str()).unwrap_or_default();
        if name.is_empty() {
            return ValidationResult::failure("name 是必填项", 2);
        }
        if !crate::skills::is_valid_skill_name(name) {
            return ValidationResult::failure(
                "技能名仅允许字母、数字、下划线、连字符或中文（≤64 字符），禁止空白与路径分隔符",
                2,
            );
        }
        if description.is_empty() {
            return ValidationResult::failure("description 是必填项（一句话说明技能用途）", 2);
        }
        if body.trim().is_empty() {
            return ValidationResult::failure("body 是必填项（技能完整指引）", 2);
        }
        ValidationResult::success(Some(json!({
            "name": name,
            "description": description,
            "body": body
        })))
    }

    async fn check_permissions(&self, _input: &Value, _context: &ToolUseContext) -> PermissionResult {
        // 风险由 risk()=FsWrite 声明，走权限网关审批矩阵；此处无额外拒绝条件
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _context: &ToolUseContext) -> ToolResult {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let body = args
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        // keywords：接受字符串数组，或逗号/空白分隔的字符串；归一为去空 token 列表
        let keywords: Vec<String> = match args.get("keywords") {
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_str())
                .flat_map(|s| s.split([',', ' ', '\t']))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            Some(Value::String(s)) => s
                .split([',', ' ', '\t'])
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
            _ => Vec::new(),
        };

        // 兜底校验（validate_input 已挡一道）
        if !crate::skills::is_valid_skill_name(&name) || description.is_empty() || body.is_empty() {
            return ToolResult::standard_error(
                "参数不合法：name/description/body 均必填；name 仅允许字母、数字、下划线、连字符或中文",
                None,
                None,
            );
        }

        // 内置技能不可覆盖
        if crate::skills::BUILTIN_SKILL_NAMES.contains(&name.as_str()) {
            return ToolResult::standard_error(
                &format!("「{name}」是内置技能，不可覆盖。请换一个技能名。"),
                None,
                None,
            );
        }

        // 可选资源：仅 references/ 或 scripts/ 下的一层文件
        let resources = match parse_resource_inputs(args.get("resources")) {
            Ok(resources) => resources,
            Err(error) => return ToolResult::standard_error(&error, None, None),
        };
        let layout = args.get("layout").and_then(|v| v.as_str()).unwrap_or("auto");
        if !matches!(layout, "auto" | "folder" | "flat") {
            return ToolResult::standard_error("layout 只能是 auto / folder / flat", None, None);
        }

        let Some(svc) = crate::cordis::global_ctx()
            .and_then(|ctx| ctx.get_service::<crate::skills::SkillService>())
        else {
            return ToolResult::standard_error("技能服务不可用", None, None);
        };

        let dir = crate::skills::SkillService::default_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return ToolResult::standard_error(&format!("创建技能目录失败: {e}"), None, None);
        }

        // 布局决策：auto 续写已有单文件，否则建目录（避免同名 flat + folder 双份）
        let flat_file = dir.join(format!("{name}.md"));
        let use_flat = match layout {
            "flat" => true,
            "folder" => false,
            _ => flat_file.is_file(),
        };
        if !use_flat && flat_file.is_file() {
            return ToolResult::standard_error(
                &format!("已存在单文件技能「{name}.md」，与目录布局冲突：请删除它，或改用 layout=\"flat\""),
                None,
                None,
            );
        }
        if use_flat && !resources.is_empty() {
            return ToolResult::standard_error(
                "单文件布局不支持 resources；请改用 layout=\"folder\" 以获得技能目录",
                None,
                None,
            );
        }

        // front-matter 要求单行，description 内的换行替换为空格；keywords 逗号连接
        let desc_one_line = description.replace(['\r', '\n'], " ");
        let kw_line = keywords.join(", ");
        let content = format!(
            "---\nname: {name}\ndescription: {desc_one_line}\nkeywords: {kw_line}\n---\n\n{body}\n"
        );

        // 写入前自检：形态问题作为 warnings 回传，不阻断
        let validation = crate::skills::validate::validate_skill_text(&name, &content);
        let warnings: Vec<String> = validation
            .issues
            .iter()
            .filter(|issue| issue.level == "warning")
            .map(|issue| issue.message.clone())
            .collect();

        let base_dir = if use_flat { dir.clone() } else { dir.join(&name) };
        let skill_file = if use_flat {
            flat_file.clone()
        } else {
            base_dir.join("SKILL.md")
        };
        let is_update = skill_file.exists();
        if let Err(e) = std::fs::create_dir_all(&base_dir) {
            return ToolResult::standard_error(&format!("创建技能目录失败: {e}"), None, None);
        }
        if let Err(e) = std::fs::write(&skill_file, content) {
            return ToolResult::standard_error(&format!("写入技能文件失败: {e}"), None, None);
        }

        // 资源 upsert：只写传入项，未提及的既有文件保留
        for (relative, text) in &resources {
            let target = base_dir.join(relative);
            if let Some(parent) = target.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return ToolResult::standard_error(&format!("创建资源目录失败: {e}"), None, None);
                }
            }
            if let Err(e) = std::fs::write(&target, text) {
                return ToolResult::standard_error(
                    &format!("写入资源「{relative}」失败: {e}"),
                    None,
                    None,
                );
            }
        }

        // 立即注册（不等 30s 热重载），写入后即可 use_skill 激活
        let mut skill = crate::skills::Skill::global(name.clone(), desc_one_line.clone(), body)
            .with_keywords(keywords.clone())
            .with_path(if use_flat {
                flat_file.display().to_string()
            } else {
                base_dir.display().to_string()
            });
        if !use_flat {
            skill = skill.with_resources(crate::skills::collect_skill_resources(&base_dir));
        }
        svc.replace_or_register(skill);

        tracing::info!(
            "[create_skill] 技能「{}」已{}（{}）",
            name,
            if is_update { "更新" } else { "创建" },
            skill_file.display()
        );

        let warnings_hint = if warnings.is_empty() {
            String::new()
        } else {
            format!("；自检提示：{}", warnings.join("；"))
        };
        ToolResult::standard_success(
            &format!(
                "技能「{name}」已{}，立即生效（含 {} 个检索关键词{}）。之后可随时调用 use_skill(\"{name}\") 按其指引行动，或用 search_skill 按关键词召回。",
                if is_update { "更新" } else { "创建" },
                keywords.len(),
                warnings_hint
            ),
            Some(json!({
                "name": name,
                "file": skill_file.display().to_string(),
                "layout": if use_flat { "flat" } else { "folder" },
                "created": !is_update,
                "keywords": keywords,
                "resources": resources.iter().map(|(path, _)| path.clone()).collect::<Vec<_>>(),
                "warnings": warnings,
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsWrite
    }

    fn is_destructive(&self) -> bool {
        // 同名覆盖会替换旧技能文件
        true
    }

    fn search_hint(&self) -> &str {
        "create skill 封装技能 沉淀 复用 自进化 保存能力 create_skill"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "一次性提醒或待办（改用 add_todo / schedule_reminder）",
            "写日记或笔记（改用 write_diary / create_notebook）",
            "记录关于用户或自己的记忆事实（改用 save_memory）",
        ]
    }
}

/// search_skill 工具 - 按自然语言召回当前角色可见的技能（名称/描述/关键词），
/// 返回候选技能的名称+描述（不含正文），由 LLM 选定后再用 use_skill 加载正文。
/// 与 tool_search 对延迟工具的两段式加载同构：先召回定位，再按需取全文。
pub struct SearchSkillTool;

impl SearchSkillTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SearchSkillTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SearchSkillTool {
    fn name(&self) -> &str {
        "search_skill"
    }

    fn description(&self) -> &str {
        "Recall reusable skills that match a natural-language need. \
         Use this when: (1) you are unsure which skill fits the current task; \
         (2) the '## 可用技能' list does not obviously contain a match; \
         (3) you want to discover skills by what they do rather than by exact name. \
         Returns each match's name + description (+ keywords), ranked by relevance — NOT the full body. \
         After picking one, call use_skill(name) to load its full instructions. \
         Do NOT use this to load a skill you already know the exact name of (call use_skill directly)."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "按自然语言召回与当前需求匹配的可复用技能。适用场景：(1) 不确定该用哪项技能；\
            (2)「## 可用技能」列表里没有明显匹配项；(3) 想按“能做什么”而非精确名称来找技能。\
            返回每个匹配项的名称+描述(+关键词)，按相关性排序——不含正文。\
            选定后再调用 use_skill(name) 加载完整指引。\
            已知技能精确名称时不要用本工具（直接 use_skill）。",
            "ja" => "自然言語で現在のニーズに合う再利用可能スキルを召回する。使用場面：(1) どのスキルを使うか不明；\
            (2)「## 可用技能」リストに明白な一致がない；(3) 正確な名前ではなく機能でスキルを探したい。\
            各一致の名称+説明(+キーワード)を関連度順に返す——本文は含まない。\
            選定後に use_skill(name) で完全な指示を読み込む。\
            正確なスキル名が既知の場合は本ツールを使わない（直接 use_skill）。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "有没有相关的技能\n找个能用在这个情况的技能\n搜一下技能",
            "en" => "is there a skill for this\nfind a relevant skill\nsearch skills",
            "ja" => "関連するスキルはある？\nこの状況に合うスキルを探して\nスキルを検索して",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural-language description of what you need; matched against skill name/description/keywords (auto-tokenized for Chinese and English)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of skills to return, default 5",
                    "minimum": 1
                }
            },
            "required": ["query"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "用自然语言描述你的需求；按技能名称/描述/关键词匹配（中英文自动分词）"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "返回的最大技能数，默认 5",
                        "minimum": 1
                    }
                },
                "required": ["query"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "ニーズを自然言語で記述；スキル名/説明/キーワードにマッチ（中英自動分かち書き）"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "返すスキルの最大数、デフォルト 5",
                        "minimum": 1
                    }
                },
                "required": ["query"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
        if query.trim().is_empty() {
            return ValidationResult::failure("query 不能为空", 2);
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim();
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .max(1) as usize;

        let Some(svc) = crate::cordis::global_ctx()
            .and_then(|ctx| ctx.get_service::<crate::skills::SkillService>())
        else {
            return ToolResult::standard_error("技能服务不可用", None, None);
        };

        let char_id = ctx.char_id.clone();
        let hits = svc.search_skills(&char_id, query, max_results);
        if hits.is_empty() {
            return ToolResult::standard_success(
                "没有匹配的技能。可换一组关键词重试，或用 create_skill 沉淀一项新技能。",
                Some(json!({ "matches": [], "query": query })),
            );
        }

        let matches: Vec<Value> = hits
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "description": s.description,
                    "keywords": s.keywords,
                })
            })
            .collect();
        let lines: Vec<String> = hits
            .iter()
            .map(|s| format!("- {}：{}", s.name, s.description))
            .collect();

        ToolResult::standard_success(
            &format!(
                "search_skill 召回 {} 项技能：\n{}\n选定后调用 use_skill(name) 加载其完整指引。",
                hits.len(),
                lines.join("\n")
            ),
            Some(json!({
                "matches": matches,
                "query": query,
                "max_results": max_results,
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    /// 权限风险等级：读取本地文件 / 持久化数据，无写入
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsRead
    }

    fn search_hint(&self) -> &str {
        "search skill 搜索技能 召回技能 找技能 匹配技能 discover skill"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "已知技能精确名称、只想加载正文（直接 use_skill）",
            "搜索工具而非技能（改用 tool_search）",
            "搜索记忆事实（改用 memory_search）",
        ]
    }
}

/// 解析已注册技能用于校验：内置技能与单文件技能返回正文，目录化技能返回其目录。
fn resolve_skill_for_validation(name: &str) -> Option<Result<std::path::PathBuf, String>> {
    let svc = crate::cordis::global_ctx()
        .and_then(|ctx| ctx.get_service::<crate::skills::SkillService>())?;
    let skill = svc.find(name)?;
    let Some(path) = skill.path.clone() else {
        return Some(Err(skill.body));
    };
    let candidate = std::path::PathBuf::from(&path);
    if candidate.is_dir() {
        Some(Ok(candidate))
    } else {
        Some(Err(skill.body))
    }
}

/// validate_skill 工具 - 交付前校验技能形态：front-matter、命名、占位符与目录卫生。
///
/// 传 `name` 校验已注册技能（含内置技能与插件技能）；传 `content` 校验草稿文本。
/// 两者都给出时以 `content` 为准，`name` 作兜底技能名。只读，无副作用。
pub struct ValidateSkillTool;

impl ValidateSkillTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ValidateSkillTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ValidateSkillTool {
    fn name(&self) -> &str {
        "validate_skill"
    }

    fn description(&self) -> &str {
        "Validate a skill's shape before shipping it: front-matter, naming, unfinished placeholders \
         and stray auxiliary docs. Pass `name` to check an existing skill, or `content` to check \
         draft text (with `name` as the fallback skill name). Read-only."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "交付前校验技能形态：front-matter 是否完整、命名是否合规、有没有未完成的占位符与多余附属文档。\
            传 `name` 校验已注册技能（含内置与插件技能）；传 `content` 校验草稿文本（`name` 作兜底技能名）。\
            只读，无副作用。",
            "ja" => "納品前にスキルの形を検証する：front-matter・命名・未完成プレースホルダ・余計な付属文書。\
            `name` で登録済みスキル、`content` で草稿テキスト（`name` をフォールバック名に）。読み取り専用。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "校验技能\n检查技能格式\n技能写对了吗",
            "en" => "validate this skill\ncheck the skill format",
            "ja" => "スキルを検証して\nスキルの形式を確認して",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Existing skill name to validate (builtin and plugin skills included)"
                },
                "content": {
                    "type": "string",
                    "description": "Draft SKILL.md text to validate; when both are given, content wins and name is the fallback skill name"
                }
            }
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "要校验的已注册技能名（含内置与插件技能）" },
                    "content": { "type": "string", "description": "要校验的 SKILL.md 草稿文本；同时给出时以 content 为准，name 作兜底技能名" }
                }
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "検証する登録済みスキル名（組み込み・プラグイン含む）" },
                    "content": { "type": "string", "description": "検証する SKILL.md 草稿。両方指定時は content 優先、name はフォールバック名" }
                }
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let has_name = input
            .get("name")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty());
        let has_content = input
            .get("content")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty());
        if !has_name && !has_content {
            return ValidationResult::failure("name 与 content 至少给一项", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _context: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _context: &ToolUseContext) -> ToolResult {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .filter(|c| !c.trim().is_empty());
        let draft = content.map(|c| c.to_string());

        let (validation, target) = if let Some(draft) = draft {
            let fallback = if name.is_empty() { "skill".to_string() } else { name.clone() };
            (
                crate::skills::validate::validate_skill_text(&fallback, &draft),
                format!("草稿文本（兜底名 {fallback}）"),
            )
        } else if !name.is_empty() {
            match resolve_skill_for_validation(&name) {
                Some(Ok(dir)) => (
                    crate::skills::validate::validate_skill_dir(&dir),
                    dir.display().to_string(),
                ),
                Some(Err(body)) => (
                    crate::skills::validate::validate_skill_text(&name, &body),
                    format!("技能「{name}」"),
                ),
                None => {
                    return ToolResult::standard_error(&format!("未找到技能「{name}」"), None, None)
                }
            }
        } else {
            return ToolResult::standard_error("name 与 content 至少给一项", None, None);
        };

        let ok = validation.ok;
        let lines: Vec<String> = validation
            .issues
            .iter()
            .map(|issue| format!("[{}] {}：{}", issue.level, issue.code, issue.message))
            .collect();
        let summary = if lines.is_empty() {
            format!("技能校验通过（{target}），未发现问题。")
        } else {
            format!(
                "技能校验{}（{target}）：\n{}",
                if ok { "通过（含提示）" } else { "未通过" },
                lines.join("\n")
            )
        };
        ToolResult::standard_success(
            &summary,
            Some(json!({ "ok": ok, "issues": validation.issues, "target": target })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsRead
    }

    fn search_hint(&self) -> &str {
        "validate skill 校验技能 检查技能格式 skill check lint 形态"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "创建或更新技能（改用 create_skill）",
            "加载技能正文（改用 use_skill）",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_inputs_enforce_path_whitelist_and_quota() {
        let value = json!([{ "path": "references/a.md", "content": "x" }]);
        let parsed = parse_resource_inputs(Some(&value)).unwrap();
        assert_eq!(parsed, vec![("references/a.md".to_string(), "x".to_string())]);

        for bad in ["../evil.md", "/abs.md", "assets/a.png", "references/a/b.md", "con.md"] {
            let value = json!([{ "path": bad, "content": "x" }]);
            assert!(parse_resource_inputs(Some(&value)).is_err(), "{bad}");
        }

        let too_many: Vec<Value> = (0..21)
            .map(|i| json!({ "path": format!("references/{i}.md"), "content": "x" }))
            .collect();
        assert!(parse_resource_inputs(Some(&json!(too_many))).is_err());

        assert!(parse_resource_inputs(None).unwrap().is_empty());
    }

    #[test]
    fn builtin_guard_blocks_guide_skills() {
        assert!(crate::skills::BUILTIN_SKILL_NAMES.contains(&"review-agent"));
        assert!(crate::skills::BUILTIN_SKILL_NAMES.contains(&"skill-creator"));
    }
}
