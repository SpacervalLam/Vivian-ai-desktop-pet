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
            Some(s) => ToolResult::standard_success(
                &format!("技能「{}」已激活，请按以下指引行动：\n\n{}", s.name, s.body),
                Some(json!({ "name": s.name, "activated": true })),
            ),
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

/// 技能名合法性：字母 / 数字 / `_` / `-` / 中日韩表意文字，长度 ≤ 64。
/// 拒绝空白与路径分隔符，防止写出技能目录或路径穿越。
fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 64
        && name.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || c == '_'
                || c == '-'
                || ('\u{4e00}'..='\u{9fff}').contains(&c)
        })
}

#[async_trait]
impl Tool for CreateSkillTool {
    fn name(&self) -> &str {
        "create_skill"
    }

    fn description(&self) -> &str {
        "Create or update a reusable skill: a markdown instruction file saved to the skills \
         directory and registered immediately. Use this to distill a reusable procedure \
         (e.g. how you handled a multi-step task) into a named skill, so it appears in the \
         '## 可用技能' list and can be activated later via use_skill. Same-name skills are overwritten."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "创建或更新一项可复用技能：以 Markdown 指引文件保存到技能目录并立即注册。\
            当你总结出一套值得复用的做法（如某类多步骤任务的处理流程）时，把它沉淀为命名技能，\
            技能会出现在「## 可用技能」列表中，之后通过 use_skill 激活。同名技能会被覆盖更新。",
            "ja" => "再利用可能なスキルを作成・更新する：Markdown 形式の指示ファイルとしてスキルディレクトリに保存し、即座に登録する。\
            繰り返し使える手順（例：複数ステップのタスクの処理方法）を名前付きスキルとしてまとめると、\
            「## 可用技能」リストに表示され、後から use_skill で有効化できる。同名スキルは上書きされる。",
            _ => self.description(),
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
        if !is_valid_skill_name(name) {
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

        // 兜底校验（validate_input 已挡一道）
        if !is_valid_skill_name(&name) || description.is_empty() || body.is_empty() {
            return ToolResult::standard_error(
                "参数不合法：name/description/body 均必填；name 仅允许字母、数字、下划线、连字符或中文",
                None,
                None,
            );
        }

        // 内置风格预设不可覆盖
        if crate::skills::BUILTIN_SKILL_NAMES.contains(&name.as_str()) {
            return ToolResult::standard_error(
                &format!("「{name}」是内置风格预设技能，不可覆盖。请换一个技能名。"),
                None,
                None,
            );
        }

        let Some(svc) = crate::cordis::global_ctx()
            .and_then(|ctx| ctx.get_service::<crate::skills::SkillService>())
        else {
            return ToolResult::standard_error("技能服务不可用", None, None);
        };

        let dir = crate::skills::SkillService::default_dir();
        let file = dir.join(format!("{name}.md"));
        let is_update = file.exists();

        if let Err(e) = std::fs::create_dir_all(&dir) {
            return ToolResult::standard_error(&format!("创建技能目录失败: {e}"), None, None);
        }

        // front-matter 要求单行，description 内的换行替换为空格
        let desc_one_line = description.replace(['\r', '\n'], " ");
        let content = format!("---\nname: {name}\ndescription: {desc_one_line}\n---\n\n{body}\n");

        if let Err(e) = std::fs::write(&file, content) {
            return ToolResult::standard_error(&format!("写入技能文件失败: {e}"), None, None);
        }

        // 立即注册（不等 30s 热重载），写入后即可 use_skill 激活
        svc.replace_or_register(crate::skills::Skill::global(
            name.clone(),
            desc_one_line.clone(),
            body,
        ));

        tracing::info!(
            "[create_skill] 技能「{}」已{}（{}）",
            name,
            if is_update { "更新" } else { "创建" },
            file.display()
        );

        ToolResult::standard_success(
            &format!(
                "技能「{name}」已{}，立即生效。之后可随时调用 use_skill(\"{name}\") 按其指引行动。",
                if is_update { "更新" } else { "创建" }
            ),
            Some(json!({
                "name": name,
                "file": file.display().to_string(),
                "created": !is_update,
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
