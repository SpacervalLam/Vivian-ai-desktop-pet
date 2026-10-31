//! 技能工具 - use_skill / create_skill
//!
//! 技能目录（<用户数据目录>/skills 的 *.md）默认只注入"名称+描述"到 prompt，
//! LLM 判断某项技能适用时调用 use_skill 获取技能正文，按其指引行动。
//! 这样技能正文不常驻上下文，按需激活，控制 token 开销。
//!
//! 读侧合并为 `use_skill` 一个入口，三种动作：
//! - `use`：按名称激活技能正文（默认；旧 use_skill 形状）
//! - `search`：按自然语言召回候选（旧 search_skill 形状：有 query）
//! - `validate`：交付前校验形态，只读（旧 validate_skill 形状：有 content）
//!
//! `create_skill` 是自进化闭环的写入侧：智能体把复用做法沉淀为技能文件，
//! 写入即注册（不等热重载），之后会话可通过 use_skill 激活。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};

/// 读侧技能动作解析（旧工具没有 action 参数）。
///
/// 旧参数形状：`content` → validate；`query` → search；其余 → use。
/// `name` 单给时默认 use——旧 `validate_skill(name)` 与 `use_skill(name)` 形状
/// 完全相同，只能选更常用的激活；按名校验需显式 `action:"validate"`。
pub(crate) fn effective_skill_action(args: &Value) -> String {
    if let Some(action) = args
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|action| !action.is_empty())
    {
        return action.to_ascii_lowercase();
    }
    if args.get("content").is_some() {
        "validate".to_string()
    } else if args.get("query").is_some() {
        "search".to_string()
    } else {
        "use".to_string()
    }
}

const SKILL_ACTIONS: &[&str] = &["use", "search", "validate"];

fn skill_schema(lang: &str) -> Value {
    match lang {
        "zh" => json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": SKILL_ACTIONS,
                    "description": "use：按名称激活技能正文（默认）；search：按自然语言召回候选（只回名称+描述+关键词）；validate：交付前校验技能形态（只读）"
                },
                "name": {
                    "type": "string",
                    "description": "要激活（use）或校验（validate）的技能名称（来自「可用技能」段落）"
                },
                "query": {
                    "type": "string",
                    "description": "search 用：用自然语言描述你的需求；按技能名称/描述/关键词匹配（中英文自动分词）"
                },
                "max_results": {
                    "type": "integer",
                    "description": "search 返回的最大技能数，默认 5",
                    "minimum": 1
                },
                "content": {
                    "type": "string",
                    "description": "validate 用：要校验的 SKILL.md 草稿文本；同时给出 name 时以 content 为准，name 作兜底技能名"
                }
            },
            "required": []
        }),
        "ja" => json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": SKILL_ACTIONS,
                    "description": "use：名前で本文を有効化（既定）；search：自然言語で候補を召回（名前+説明+キーワードのみ）；validate：納品前の形式検証（読み取り専用）"
                },
                "name": {
                    "type": "string",
                    "description": "有効化（use）または検証（validate）するスキル名（「可用技能」セクションから）"
                },
                "query": {
                    "type": "string",
                    "description": "search 用：ニーズを自然言語で記述；スキル名/説明/キーワードにマッチ"
                },
                "max_results": {
                    "type": "integer",
                    "description": "search が返すスキルの最大数、デフォルト 5",
                    "minimum": 1
                },
                "content": {
                    "type": "string",
                    "description": "validate 用：検証する SKILL.md 草稿。両方指定時は content 優先、name はフォールバック名"
                }
            },
            "required": []
        }),
        _ => json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": SKILL_ACTIONS,
                    "description": "use: activate a skill by name (default); search: recall candidates from a natural-language need (name+description+keywords only); validate: shape-check a skill (read-only)"
                },
                "name": {
                    "type": "string",
                    "description": "Skill name to activate (use) or validate (from the 可用技能 section)"
                },
                "query": {
                    "type": "string",
                    "description": "For search: natural-language description of what you need; matched against skill name/description/keywords"
                },
                "max_results": {
                    "type": "integer",
                    "description": "For search: maximum number of skills to return, default 5",
                    "minimum": 1
                },
                "content": {
                    "type": "string",
                    "description": "For validate: draft SKILL.md text to validate; when name is also given, content wins and name is the fallback skill name"
                }
            },
            "required": []
        }),
    }
}

/// use_skill 工具 - 技能读侧统一入口：激活正文 / 自然语言召回 / 形态校验。
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
        "One entry for the read side of the skill library. Actions: \
         use (default; activate a skill by exact name and return its full instructions — the skill list is in the \
         '## 可用技能' prompt section), \
         search (recall candidates from a natural-language need; returns name + description + keywords only, ranked by relevance), \
         validate (shape-check before shipping: front-matter, naming, unfinished placeholders, stray auxiliary docs; read-only). \
         When you know the exact name, activate directly; search only when you are unsure which skill fits. \
         After a search, activate the chosen one by name. Skills grant no permissions and execute nothing: \
         follow their guidance only with tools that are registered and approved."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "技能库读侧的统一入口。action 取值：\
            use（默认，按精确名称激活技能并返回完整指引——可用技能列在 prompt 的「## 可用技能」段落）、\
            search（按自然语言召回候选，只返回名称+描述+关键词，按相关性排序）、\
            validate（交付前校验形态：front-matter、命名、未完成占位符与多余附属文档，只读）。\
            已知精确名称就直接激活；只有不确定该用哪项时才 search，选定后再按名称激活。\
            技能不授予权限、不执行操作：只按它的指引使用已注册且获准的工具。",
            "ja" => "スキルライブラリ読み取り側の統一入口。action：\
            use（既定。正確な名前で有効化し完全な指示を返す——「## 可用技能」セクションに一覧）、\
            search（自然言語で候補を召回。名前+説明+キーワードのみを関連度順に返す）、\
            validate（納品前の形式検証：front-matter・命名・未完成プレースホルダ・余計な付属文書。読み取り専用）。\
            正確な名前が分かっていれば直接有効化；どれを使うか不明な時だけ search し、選定後に名前で有効化。\
            スキルは権限を付与せず、何も実行しない：登録済みで許可されたツールだけを使う。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "用一下那个技能\n调用技能\n跑一下这个技能\n有没有相关的技能\n搜一下技能\n校验技能\n检查技能格式",
            "en" => "use that skill\nrun this skill\napply the skill\nis there a skill for this\nsearch skills\nvalidate this skill",
            "ja" => "そのスキルを使って\nスキルを実行して\n関連するスキルはある？\nスキルを検索して\nスキルを検証して",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        skill_schema("en")
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        skill_schema(lang)
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let action = effective_skill_action(input);
        if !SKILL_ACTIONS.contains(&action.as_str()) {
            return ValidationResult::failure(
                &format!(
                    "不支持的 action: {action}（可选：{}）",
                    SKILL_ACTIONS.join(" / ")
                ),
                2,
            );
        }
        match action.as_str() {
            "use" => match input.get("name").and_then(Value::as_str) {
                Some(name) if !name.trim().is_empty() => {
                    ValidationResult::success(Some(json!({ "name": name.trim() })))
                }
                _ => ValidationResult::failure("name 是必填项（要激活的技能名）", 2),
            },
            "search" => match input.get("query").and_then(Value::as_str) {
                Some(query) if !query.trim().is_empty() => {
                    ValidationResult::success(Some(json!({ "query": query.trim() })))
                }
                _ => ValidationResult::failure("query 不能为空（用自然语言描述需求）", 2),
            },
            _ => {
                let has_name = input
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty());
                let has_content = input
                    .get("content")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty());
                if !has_name && !has_content {
                    return ValidationResult::failure("validate 需要 name 或 content", 2);
                }
                ValidationResult::success(None)
            }
        }
    }

    async fn check_permissions(&self, _input: &Value, _context: &ToolUseContext) -> PermissionResult {
        // 三种动作都只读，无副作用，无需确认
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        match effective_skill_action(&args).as_str() {
            "use" => activate_skill(args, context).await,
            "search" => search_skill_candidates(args, context).await,
            "validate" => validate_skill_shape(args).await,
            other => ToolResult::standard_error(
                &format!("不支持的 action: {other}"),
                Some("InvalidAction"),
                None,
            ),
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

    fn search_hint(&self) -> &str {
        "skill 技能 激活 搜索技能 召回技能 校验技能 use discover validate"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "创建或更新技能（改用 create_skill）",
            "搜索工具而非技能（改用 tool_search）",
            "搜索记忆事实（改用 search_memory）",
        ]
    }
}

/// action=use：按名称激活，返回技能正文与资源清单。
async fn activate_skill(args: Value, context: &ToolUseContext) -> ToolResult {
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

/// action=search：BM25 召回候选（不含正文），选定后再按名称激活。
async fn search_skill_candidates(args: Value, ctx: &ToolUseContext) -> ToolResult {
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
            "按需求召回 {} 项技能：\n{}\n选定后调用 use_skill(name) 加载其完整指引。",
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

/// action=validate：校验草稿文本或已注册技能，只读。
async fn validate_skill_shape(args: Value) -> ToolResult {
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
         (synonyms, trigger phrases, related terms) so use_skill can recall it later by description. \
         Same-name skills are overwritten."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "创建或更新一项可复用技能，并立即注册。新技能以目录形态写入 `<name>/SKILL.md`，\
            可选文本资源放在 `references/` 或 `scripts/` 下。当你总结出一套值得复用的做法时，\
            把它沉淀为命名技能，技能会出现在「## 可用技能」列表中，之后通过 use_skill 激活。\
            请提供 keywords（同义词、触发短语、相关词），以便之后 use_skill 能用自然语言召回它。\
            同名技能会被覆盖更新。",
            "ja" => "再利用可能なスキルを作成・更新する：Markdown 形式の指示ファイルとしてスキルディレクトリに保存し、即座に登録する。\
            繰り返し使える手順（例：複数ステップのタスクの処理方法）を名前付きスキルとしてまとめると、\
            「## 可用技能」リストに表示され、後から use_skill で有効化できる。\
            keywords（同義語・トリガー句・関連語）を指定すると、後で use_skill が自然言語で呼び出せる。\
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
                    "description": "Optional retrieval terms (synonyms, trigger phrases, related words) used by use_skill to recall this skill; not shown in the 可用技能 list"
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
                        "description": "可选检索词（同义词、触发短语、相关词），供 use_skill 召回此技能；不显示在「可用技能」列表中"
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
                        "description": "任意の検索語（同義語・トリガー句・関連語）。use_skill がこのスキルを召回するのに使用、「可用技能」リストには表示されない"
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
                "技能「{name}」已{}，立即生效（含 {} 个检索关键词{}）。之后可随时调用 use_skill(\"{name}\") 按其指引行动，或用 use_skill(query=\"…\") 按需求召回。",
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

    #[test]
    fn legacy_skill_arguments_infer_actions() {
        // 旧 use_skill：只有 name（旧 validate_skill(name) 形状相同，只能默认激活）
        assert_eq!(effective_skill_action(&json!({ "name": "demo" })), "use");
        // 旧 search_skill：query + max_results
        assert_eq!(
            effective_skill_action(&json!({ "query": "更新供应商预设", "max_results": 5 })),
            "search"
        );
        // 旧 validate_skill：content 优先（同时带 name 也走校验）
        assert_eq!(
            effective_skill_action(&json!({ "name": "demo", "content": "---\nname: demo\n---" })),
            "validate"
        );
        // 显式 action 永远优先
        assert_eq!(
            effective_skill_action(&json!({ "action": " VALIDATE ", "name": "demo" })),
            "validate"
        );
        // 空参数默认 use，交给 validate_input 报缺 name
        assert_eq!(effective_skill_action(&json!({})), "use");
    }

    #[test]
    fn unknown_skill_actions_are_rejected() {
        for action in ["", "activate", "create", "delete"] {
            let args = json!({ "action": action, "name": "demo" });
            let expected = if action.is_empty() { "use" } else { action };
            assert_eq!(effective_skill_action(&args), expected);
        }
        assert!(!SKILL_ACTIONS.contains(&"activate"));
    }
}
