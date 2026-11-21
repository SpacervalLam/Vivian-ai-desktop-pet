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
//!
//! 发布纪律（`skills/publish.rs`）：形态校验的 error 级问题阻断写入；
//! 正文/描述/关键词有硬预算；同名覆盖受单步改动幅度约束，整版替换必须
//! 显式声明当前 base 指纹；覆盖前保存点前缀目录里的可回退版本快照。
//!
//! 效果闸门（`skills/evaluation.rs` + `skills/selection.rs`）：发布前用宿主维护的
//! 独立选择集做配对回放，回放的是运行时真实的 BM25 选择函数。只有 `delta >= delta_min`
//! 且保留集零退化、无误召回净增，才判 `Accepted` 并真正落盘。未通过 / 无法验证的
//! 候选只进 `.staging/`，永不生效；拒绝反馈脱敏，不回传题目原文。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::skills::{evaluation, publish, selection};

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

    /// 版本查询与回退。只操作宿主保存过的已发布版本，不生成新候选。
    ///
    /// 回退本身也是一次发布：先把现行版本存成快照，保证回退可逆；
    /// 只恢复快照里记录的正文与资源，不删除之后新增的其它文件。
    fn version_action(&self, action: &str, name: &str, args: &Value) -> ToolResult {
        if !crate::skills::is_valid_skill_name(name) {
            return ToolResult::standard_error("技能名不合法", None, None);
        }
        let dir = crate::skills::SkillService::default_dir();
        if action == "revisions" {
            let versions = match publish::list_snapshots(&dir, name) {
                Ok(v) => v,
                Err(e) => return ToolResult::standard_error(&e, None, None),
            };
            let list: Vec<Value> = versions
                .iter()
                .map(|v| {
                    json!({
                        "revision": v.revision,
                        "layout": v.layout,
                        "origin": v.origin,
                        "created_at_ms": v.created_at_ms,
                        "changed_from": v.changed_from,
                    })
                })
                .collect();
            return ToolResult::standard_success(
                &format!("技能「{name}」有 {} 个可回退版本", list.len()),
                Some(json!({ "name": name, "versions": list })),
            );
        }
        let rev = args
            .get("revision")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let restored = match publish::restore_snapshot(&dir, name, rev) {
            Ok(v) => v,
            Err(e) => return ToolResult::standard_error(&e, None, None),
        };
        let skill_dir = dir.join(name);
        // 目标路径、布局归一、跨布局冲突与「现行版本到底在哪」都交给纯函数判定，
        // 保证快照用的是磁盘上真实存在的那个文件。
        let target = match publish::restore_target(&dir, name, &restored.layout) {
            Ok(t) => t,
            Err(e) => return ToolResult::standard_error(&e, None, None),
        };
        let restored_layout = target.layout;
        let skill_file = target.file.clone();
        if let Some((path, layout)) = &target.current {
            if let Err(e) = publish::save_snapshot(
                &dir,
                name,
                path,
                layout,
                "rollback",
                Some(&restored.revision),
            ) {
                return ToolResult::standard_error(
                    &format!("回退前保存现行版本失败：{e}"),
                    None,
                    None,
                );
            }
        }
        if let Some(parent) = skill_file.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolResult::standard_error(&format!("创建技能目录失败: {e}"), None, None);
            }
        }
        if let Err(e) = crate::utils::fs::write_atomic(&skill_file, &restored.content) {
            return ToolResult::standard_error(&format!("写回版本正文失败: {e}"), None, None);
        }
        let mut restored_resources = 0usize;
        for (relative, text) in &restored.resources {
            if !crate::skills::is_safe_skill_rel_path(relative) {
                continue;
            }
            let target = skill_dir.join(relative);
            if let Some(parent) = target.parent() {
                if std::fs::create_dir_all(parent).is_err() {
                    continue;
                }
            }
            if crate::utils::fs::write_atomic(&target, text).is_ok() {
                restored_resources += 1;
            }
        }
        let Some(svc) = crate::cordis::global_ctx()
            .and_then(|ctx| ctx.get_service::<crate::skills::SkillService>())
        else {
            return ToolResult::standard_error("技能服务不可用", None, None);
        };
        let parsed = crate::skills::parse_skill_doc(&restored.content, name);
        let mut skill = crate::skills::Skill::global(
            parsed.name,
            parsed.description,
            parsed.body,
        )
        .with_keywords(parsed.keywords);
        skill = if restored_layout == "flat" {
            skill.with_path(skill_file.display().to_string())
        } else {
            skill.with_path(skill_dir.display().to_string())
                .with_resources(crate::skills::collect_skill_resources(&skill_dir))
        };
        svc.replace_or_register(skill);
        ToolResult::standard_success(
            &format!(
                "技能「{name}」已回退到版本 {}，并立即生效（恢复 {restored_resources} 个资源）。现行版本已另存快照，可再次回退。",
                restored.revision
            ),
            Some(json!({
                "name": name,
                "revision": restored.revision,
                "layout": restored_layout,
                "restored_resources": restored_resources,
                "file": skill_file.display().to_string(),
            })),
        )
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

/// keywords 接受字符串数组，或逗号/空白分隔的字符串；归一为去空 token 列表。
fn parse_keywords(value: Option<&Value>) -> Vec<String> {
    match value {
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
    }
}

/// create_skill 的动作。写入是默认动作，其余分别做评估、候选管理与版本回退。
const CREATE_SKILL_ACTIONS: &[&str] = &[
    "write",
    "evaluate",
    "promote",
    "candidates",
    "revisions",
    "restore",
];

fn create_skill_action(args: &Value) -> String {
    args.get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|a| CREATE_SKILL_ACTIONS.contains(a))
        .unwrap_or("write")
        .to_string()
}

/// 发布前的效果闸门：加载宿主选择集，用运行时真实 BM25 做配对回放。
///
/// 评估集缺失、损坏或样本配比不足时返回 `Unverifiable`；调用方必须 fail-closed，
/// 不能把「没测」当成「测过了没事」。
fn evaluate_candidate(
    name: &str,
    content: &str,
    old_content: Option<&str>,
    skills_dir: &std::path::Path,
    all_skills: &[crate::skills::Skill],
) -> evaluation::Evaluation {
    // 必须走 locked 版本：点前缀目录只是目录隔离，通用文件工具仍可能改写
    // selection.json。locked 会复核进程启动时锁定摘要，会话内篡改一律 Unverifiable。
    let set = match evaluation::load_selection_set_locked(skills_dir, name) {
        Ok(set) => set,
        Err(error) => return evaluation::Evaluation::unverifiable(error),
    };
    let candidate = selection::SkillVariant::from_content(content, name);
    let baseline = old_content.map(|old| selection::SkillVariant::from_content(old, name));
    let distractors = selection::visible_distractors(all_skills, name);
    selection::replay(name, baseline.as_ref(), &candidate, &distractors, &set)
}

#[async_trait]
impl Tool for CreateSkillTool {
    fn name(&self) -> &str {
        "create_skill"
    }

    fn description(&self) -> &str {
        "Create or update a reusable skill. A write is PUBLISHED only after it passes a paired \
         replay on a host-maintained independent selection set: the new version must improve \
         recall over the current version by at least delta_min, with zero regressions on protected \
         cases and no new false positives. If it fails, or no selection set exists, nothing is \
         published: the candidate is staged under .staging/ and a desensitized evaluation plus a \
         candidate_id is returned. New skills are written as `<name>/SKILL.md`; resources go under \
         `references/` or `scripts/`. Provide keywords (synonyms, trigger phrases) so use_skill can \
         recall the skill. Same-name overwrites are bounded: a full rewrite must pass \
         expected_base_hash (from a previous write or action=revisions); every overwrite is \
         snapshotted. Other actions: action=evaluate reports the replay result without publishing; \
         action=promote with candidate_id re-evaluates a staged candidate and publishes it; \
         action=candidates lists staged candidates; action=revisions lists saved versions; \
         action=restore with name+revision rolls back. The selection set is maintained by the host \
         and its query text is never returned to the model."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "创建或更新可复用技能。write 只有在通过**独立选择集上的配对回放**后才会真正发布：\
            新版本相对现行版本须有不低于 delta_min 的召回净收益，保留集样本零退化、无误召回净增。\
            未通过（或尚无选择集）时不写入技能目录，候选暂存到 .staging/，返回脱敏评估结论与 candidate_id。\
            新技能写入 `<name>/SKILL.md`，资源放在 `references/` 或 `scripts/` 下；\
            请提供 keywords（同义词、触发短语），以便之后 use_skill 用自然语言召回。\
            同名覆盖有单步改动上限；整版替换须传 expected_base_hash（上一次写入的 base_revision \
            或 action=revisions 查询结果）。覆盖前自动保存可回退版本。\
            其它动作：action=evaluate 只回报评估、不发布；action=promote 携 candidate_id 重评暂存候选并发布；\
            action=candidates 列出暂存候选；action=revisions 列出历史版本；action=restore 按 revision 回退。\
            选择集由宿主维护，题目原文不会回传给模型。",
            "ja" => "再利用可能なスキルを作成・更新する。write はホスト管理の独立選択集合での\
            ペアリプレイに合格した場合のみ公開される：現行版に対する召回改善が delta_min 以上、\
            保留集合の劣化ゼロ、誤召回の純増ゼロが必要。不合格または選択集合が無い場合は\
            スキルディレクトリには書き込まず、候補を .staging/ に保存し、題目を含まない評価結果と\
            candidate_id を返す。keywords（同義語・トリガー句）を指定すると use_skill が召回できる。\
            同名の上書きは 1 回の変更量に上限があり、全文差し替えには expected_base_hash が必要。\
            上書き前に版を保存する。action=evaluate は評価のみ、action=promote は candidate_id で\
            候補を再評価して公開、action=candidates は候補一覧、action=revisions / restore は版の確認・巻き戻し。",
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
                "action": {
                    "type": "string",
                    "enum": CREATE_SKILL_ACTIONS,
                    "description": "write (default) publishes ONLY if the paired replay passes; evaluate reports the replay without publishing; promote re-evaluates a staged candidate (needs candidate_id); candidates lists staged candidates; revisions lists saved versions; restore rolls back (needs revision)"
                },
                "name": {
                    "type": "string",
                    "description": "Skill name: letters, digits, '_', '-' or CJK only, no spaces/slashes (used as the .md filename)"
                },
                "description": {
                    "type": "string",
                    "description": "One-line description shown in the 可用技能 list. Required for action=write"
                },
                "body": {
                    "type": "string",
                    "description": "Full skill instructions in markdown. Required for action=write; bounded to 12000 chars"
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
                },
                "expected_base_hash": {
                    "type": "string",
                    "description": "For a same-name full rewrite: the base_revision of the CURRENT skill (from a previous write or action=revisions). Small edits do not need it; a mismatched value is rejected"
                },
                "revision": {
                    "type": "string",
                    "description": "For action=restore: the saved version fingerprint to roll back to"
                },
                "candidate_id": {
                    "type": "string",
                    "description": "For action=promote: the id returned when a write was staged instead of published"
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
                    "action": {
                        "type": "string",
                        "enum": CREATE_SKILL_ACTIONS,
                        "description": "write（默认）仅在配对回放通过后发布；evaluate 只回报评估、不发布；promote 携 candidate_id 重评暂存候选并发布；candidates 列出暂存候选；revisions 列出历史版本；restore 按 revision 回退"
                    },
                    "name": {
                        "type": "string",
                        "description": "技能名：仅字母/数字/下划线/连字符/中文，不含空格与斜杠（将作为 .md 文件名）"
                    },
                    "description": {
                        "type": "string",
                        "description": "一句话描述，显示在「可用技能」列表中；action=write 时必填"
                    },
                    "body": {
                        "type": "string",
                        "description": "技能完整指引（Markdown）：做什么、怎么做；action=write 时必填，上限 12000 字"
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
                    },
                    "expected_base_hash": {
                        "type": "string",
                        "description": "同名整版替换时必填：当前技能的 base_revision（上一次写入返回值或 action=revisions 查询结果）；小步改动不需要，指纹不匹配会拒绝"
                    },
                    "revision": {
                        "type": "string",
                        "description": "action=restore 时要回退到的历史版本指纹"
                    },
                    "candidate_id": {
                        "type": "string",
                        "description": "action=promote 时使用：write 被暂存而非发布时返回的候选 id"
                    }
                },
                "required": ["name"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": CREATE_SKILL_ACTIONS,
                        "description": "write（既定）はペアリプレイ合格時のみ公開、evaluate は評価のみ、promote は candidate_id で候補を再評価して公開、candidates は候補一覧、revisions は履歴一覧、restore は revision で巻き戻し"
                    },
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
                    },
                    "expected_base_hash": {
                        "type": "string",
                        "description": "同名の全文差し替えに必要な現行版の base_revision。小さな編集には不要、不一致は拒否される"
                    },
                        "revision": {
                            "type": "string",
                            "description": "action=restore で戻す先の版フィンガープリント"
                        },
                        "candidate_id": {
                            "type": "string",
                            "description": "action=promote で使う、公開されずにステージングされた候補の id"
                        }
                    },
                    "required": ["name"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        let name = input.get("name").and_then(|v| v.as_str()).unwrap_or_default().trim();
        if name.is_empty() {
            return ValidationResult::failure("name 是必填项", 2);
        }
        if !crate::skills::is_valid_skill_name(name) {
            return ValidationResult::failure(
                "技能名仅允许字母、数字、下划线、连字符或中文（≤64 字符），禁止空白与路径分隔符",
                2,
            );
        }
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .unwrap_or("write");
        if !CREATE_SKILL_ACTIONS.contains(&action) {
            return ValidationResult::failure(
                "action 只能是 write / evaluate / promote / candidates / revisions / restore",
                2,
            );
        }
        if action != "write" && action != "evaluate" {
            if action == "restore"
                && input
                    .get("revision")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
            {
                return ValidationResult::failure("restore 需要 revision", 2);
            }
            if action == "promote"
                && input
                    .get("candidate_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
            {
                return ValidationResult::failure("promote 需要 candidate_id", 2);
            }
            return ValidationResult::success(Some(json!({ "name": name, "action": action })));
        }
        let description = input
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        let body = input.get("body").and_then(|v| v.as_str()).unwrap_or_default();
        if description.is_empty() {
            return ValidationResult::failure("description 是必填项（一句话说明技能用途）", 2);
        }
        if body.trim().is_empty() {
            return ValidationResult::failure("body 是必填项（技能完整指引）", 2);
        }
        let keywords = parse_keywords(input.get("keywords"));
        if let Err(error) = publish::validate_budget(description, body, &keywords) {
            return ValidationResult::failure(&format!("技能发布预算不通过：{error}"), 2);
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

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
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
        let keywords = parse_keywords(args.get("keywords"));
        let action = create_skill_action(&args);
        if action == "revisions" || action == "restore" {
            return self.version_action(&action, &name, &args);
        }

        let dir = crate::skills::SkillService::default_dir();

        // 列出未生效的暂存候选。
        if action == "candidates" {
            let list = match evaluation::list_candidates(&dir, &name) {
                Ok(list) => list,
                Err(error) => return ToolResult::standard_error(&error, None, None),
            };
            let items: Vec<Value> = list
                .iter()
                .map(|c| {
                    json!({
                        "candidate_id": c.id,
                        "name": c.name,
                        "created_at_ms": c.created_at_ms,
                        "verdict": c.verdict,
                    })
                })
                .collect();
            return ToolResult::standard_success(
                &format!("技能「{name}」有 {} 个暂存候选（均未生效）", items.len()),
                Some(json!({ "name": name, "candidates": items })),
            );
        }

        // 评估集补齐后重评一个暂存候选。这里不直接写盘，而是转成等价的 write
        // 重走完整闸门（形态/预算/改动幅度/配对回放/快照/原子写），避免第二条发布路径。
        if action == "promote" {
            let id = args
                .get("candidate_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            let loaded = match evaluation::load_candidate(&dir, id) {
                Ok(value) => value,
                Err(error) => {
                    return ToolResult::standard_error(
                        &format!("读取暂存候选失败：{error}"),
                        None,
                        None,
                    )
                }
            };
            if loaded.name != name {
                return ToolResult::standard_error(
                    "candidate_id 属于另一个技能，name 必须一致",
                    None,
                    None,
                );
            }
            let parsed = crate::skills::parse_skill_doc(&loaded.content, &name);
            // 候选的原始布局与资源必须原样带回，否则重评通过后会发布一个残缺版本。
            let staged_resources: Vec<Value> = loaded
                .resources
                .iter()
                .map(|(path, text)| json!({ "path": path, "content": text }))
                .collect();
            let mut next = json!({
                "name": name,
                "description": parsed.description,
                "body": parsed.body,
                "keywords": parsed.keywords,
                "action": "write",
                "layout": loaded.layout,
                "resources": staged_resources,
            });
            // 整版替换需要 base 指纹；按当前磁盘状态自动补齐，避免被幅度闸门二次挡回。
            let flat_file = dir.join(format!("{name}.md"));
            let folder_file = dir.join(&name).join("SKILL.md");
            let current_file = if flat_file.is_file() {
                Some(flat_file)
            } else if folder_file.is_file() {
                Some(folder_file)
            } else {
                None
            };
            if let Some(path) = current_file {
                if let Ok(current) = std::fs::read_to_string(path) {
                    next["expected_base_hash"] = json!(publish::revision(&current));
                }
            }
            return self.call(next, context).await;
        }

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
        // 暂存候选与最终发布都要用这个已解析的布局，不能让 promote 退回 auto。
        let layout_name = if use_flat { "flat" } else { "folder" };
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

        // 发布预算由宿主复核，不能只依赖模型遵守 schema 的提示。
        if let Err(error) = publish::validate_budget(&desc_one_line, &body, &keywords) {
            return ToolResult::standard_error(&format!("技能发布预算不通过：{error}"), None, None);
        }

        // 形态校验的 error 级问题必须阻断写入，与设置页 save_skill 的门槛一致。
        // 旧实现只收集 warning、从不读 validation.ok，自动写入路径比人工路径还松。
        let validation = crate::skills::validate::validate_skill_text(&name, &content);
        if !validation.ok {
            let errors: Vec<String> = validation
                .issues
                .iter()
                .filter(|issue| issue.level == "error")
                .map(|issue| issue.message.clone())
                .collect();
            return ToolResult::standard_error(
                &format!("技能未通过校验，未写入：{}", errors.join("；")),
                None,
                None,
            );
        }
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
        let old_content = std::fs::read_to_string(&skill_file).ok();

        // —— 效果闸门：独立选择集 + 配对回放 ——
        // 回放的是运行时真实的 BM25 选择函数，不是代理指标。评估集由宿主维护、
        // 题目原文不回传模型；没有可用评估集时 fail-closed（暂存，不发布）。
        let assessment = evaluate_candidate(
            &name,
            &content,
            old_content.as_deref(),
            &dir,
            &svc.list_all(),
        );
        if action == "evaluate" {
            return ToolResult::standard_success(
                &format!(
                    "配对回放完成（{}，未发布）：{}",
                    assessment.verdict.as_str(),
                    assessment.reason_text()
                ),
                Some(json!({
                    "name": name,
                    "published": false,
                    "evaluation": assessment.to_value(),
                    "note": "选择集题目由宿主维护，不回传模型；evaluate 不写入任何技能文件",
                })),
            );
        }
        if assessment.verdict != evaluation::Verdict::Accepted {
            let candidate_id = match evaluation::save_candidate_with(
                &dir,
                &name,
                &content,
                &assessment,
                layout_name,
                &resources,
            ) {
                Ok(id) => id,
                Err(error) => {
                    return ToolResult::standard_error(
                        &format!("暂存未通过评估的候选失败：{error}"),
                        None,
                        None,
                    )
                }
            };
            let hint = if assessment.verdict == evaluation::Verdict::Unverifiable {
                "当前没有可用的独立选择集，自动发布处于暂停状态；请由宿主补齐 selection.json 后用 action=promote 重评此候选。"
            } else {
                "候选未达严格改善门槛；可调整 description / keywords 后重新提交，或用 action=revisions 查看现行版本。"
            };
            return ToolResult::standard_error(
                &format!(
                    "发布被配对回放拦截（{}）：{}。候选已暂存为 {candidate_id}，未生效。{hint}",
                    assessment.verdict.as_str(),
                    assessment.reason_text(),
                ),
                None,
                Some(json!({
                    "published": false,
                    "name": name,
                    "candidate_id": candidate_id,
                    "evaluation": assessment.to_value(),
                    "hint": hint,
                })),
            );
        }

        // 评估通过后才创建技能目录：被拒的候选不能在可加载树里留下空目录。
        if let Err(e) = std::fs::create_dir_all(&base_dir) {
            return ToolResult::standard_error(&format!("创建技能目录失败: {e}"), None, None);
        }

        // 版本检查：小步改动可直接覆盖；整版替换必须声明当前 base 指纹。
        // 这一步只约束「改动幅度」，有效性由上面的配对回放负责。
        let expected_base = args.get("expected_base_hash").and_then(Value::as_str);
        let plan = match publish::check_update(old_content.as_deref(), &content, expected_base) {
            Ok(plan) => plan,
            Err(error) => {
                return ToolResult::standard_error(&format!("发布被拒绝：{error}"), None, None)
            }
        };
        // 覆盖前先留可回退版本；快照在点前缀目录，不会触发热重载。
        let snapshot = match publish::save_snapshot(
            &dir,
            &name,
            &skill_file,
            layout_name,
            "auto",
            plan.base_revision.as_deref(),
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return ToolResult::standard_error(&format!("保存现行版本失败：{error}"), None, None)
            }
        };
        // 正文单文件原子落盘，避免热重载读到半截版本。
        if let Err(e) = crate::utils::fs::write_atomic(&skill_file, &content) {
            return ToolResult::standard_error(&format!("写入技能文件失败: {e}"), None, None);
        }
        let revision = publish::revision(&content);

        // 资源 upsert：只写传入项，未提及的既有文件保留
        for (relative, text) in &resources {
            let target = base_dir.join(relative);
            if let Some(parent) = target.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return ToolResult::standard_error(&format!("创建资源目录失败: {e}"), None, None);
                }
            }
            if let Err(e) = crate::utils::fs::write_atomic(&target, text) {
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
                "技能「{name}」已{}，立即生效（版本 {revision}，{} 个检索关键词{}）。\
                 已通过独立选择集上的配对回放：{} 条样本、净收益 {:.3} ≥ 门槛 {:.3}。{}之后可用 use_skill(\"{name}\") 激活。\
                 该回放只验证「技能召回选择」是否改善，不验证技能执行结果与最终答复质量。",
                if is_update { "更新" } else { "创建" },
                keywords.len(),
                warnings_hint,
                assessment.case_count,
                assessment.delta,
                assessment.delta_min,
                if is_update { "覆盖前的版本已存快照，可用 action=revisions / restore 回退。" } else { "" },
            ),
            Some(json!({
                "name": name,
                "file": skill_file.display().to_string(),
                "layout": layout_name,
                "created": !is_update,
                "revision": revision,
                "base_revision": plan.base_revision,
                "changed_chars": plan.changed_chars,
                "explicit_base": plan.explicit_base,
                "snapshot": snapshot,
                "keywords": keywords,
                "resources": resources.iter().map(|(path, _)| path.clone()).collect::<Vec<_>>(),
                "warnings": warnings,
                "validation": "paired_replay_accepted",
                "evaluation": assessment.to_value(),
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
    fn create_skill_actions_normalize_and_default_to_write() {
        assert_eq!(create_skill_action(&json!({})), "write");
        assert_eq!(create_skill_action(&json!({ "action": " restore " })), "write");
        assert_eq!(create_skill_action(&json!({ "action": "restore" })), "restore");
        assert_eq!(create_skill_action(&json!({ "action": "revisions" })), "revisions");
        assert_eq!(create_skill_action(&json!({ "action": "evaluate" })), "evaluate");
        assert_eq!(create_skill_action(&json!({ "action": "promote" })), "promote");
        assert_eq!(create_skill_action(&json!({ "action": "candidates" })), "candidates");
        // 未知值由 validate_input 拒绝；解析层只是归一到默认 write。
        assert_eq!(create_skill_action(&json!({ "action": "delete" })), "write");
        assert_eq!(CREATE_SKILL_ACTIONS.len(), 6);
    }

    #[test]
    fn all_language_schemas_share_the_action_whitelist_and_candidate_id() {
        let tool = CreateSkillTool;
        for (lang, schema) in [
            ("en", tool.parameters_schema()),
            ("zh", tool.parameters_schema_in("zh")),
            ("ja", tool.parameters_schema_in("ja")),
        ] {
            let actions = schema["properties"]["action"]["enum"]
                .as_array()
                .unwrap_or_else(|| panic!("{lang} schema 缺少 action enum"));
            assert_eq!(actions.len(), CREATE_SKILL_ACTIONS.len(), "{lang}");
            assert!(
                schema["properties"]["candidate_id"].is_object(),
                "{lang} schema 缺少 candidate_id"
            );
        }
    }

    #[test]
    fn create_skill_keywords_are_normalized_for_budget_checks() {
        assert_eq!(
            parse_keywords(Some(&json!(["召回, 触发", "同义"]))),
            vec!["召回".to_string(), "触发".to_string(), "同义".to_string()]
        );
        assert_eq!(
            parse_keywords(Some(&json!("a b,c"))),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert!(parse_keywords(None).is_empty());
        let long = vec!["k".repeat(publish::MAX_KEYWORD_CHARS + 1)];
        assert!(publish::validate_budget("d", "正文", &long).is_err());
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
