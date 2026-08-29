//! 工具创建工具 - create_tool
//!
//! 智能体能力自进化的执行侧：当现有工具无法覆盖某个可复用的执行能力时，
//! 智能体用本工具把「PowerShell 脚本 + 参数 schema」封装为一个新工具，
//! 立即注册进工具系统（下一轮请求即出现在工具列表，同一 agent 循环内
//! 也可直接调用）。
//!
//! 与能力沉淀体系其他两级的关系：
//! - `create_skill`：提示词级（怎么做的方法论）——无执行逻辑
//! - `run_workflow`：既有工具的编排（组合）——不产生新原语
//! - `create_tool`：可执行原语（新能力本身）——真正的能力进化

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::custom_tools;
use crate::tools::registry::ToolSystem;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};

/// create_tool 工具 - 注册一个 PowerShell 脚本支撑的新工具。
pub struct CreateToolTool {
    tool_system: Arc<ToolSystem>,
}

impl CreateToolTool {
    pub fn new(tool_system: Arc<ToolSystem>) -> Self {
        Self { tool_system }
    }
}

#[async_trait]
impl Tool for CreateToolTool {
    fn name(&self) -> &str {
        "create_tool"
    }

    fn description(&self) -> &str {
        "Create a new executable tool backed by a PowerShell script, registered immediately and \
         permanently available in future sessions. The script receives the tool-call arguments \
         as JSON on stdin (read with: $args = [Console]::In.ReadToEnd() | ConvertFrom-Json) and \
         writes its result to stdout. Use this when a reusable EXECUTABLE capability is missing \
         (e.g. calling a specific API and reformatting output). For prompt-level methodology use \
         create_skill instead; for composing existing tools use run_workflow."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "创建一个由 PowerShell 脚本支撑的新工具，立即注册且跨会话永久可用。\
            脚本通过 stdin 接收工具调用参数的 JSON（读取方式：$args = [Console]::In.ReadToEnd() | ConvertFrom-Json），\
            把结果写到 stdout。当缺少可复用的「可执行」能力时使用（如调用某个 API 并整理输出）。\
            沉淀方法论用 create_skill；编排既有工具用 run_workflow。",
            "ja" => "PowerShell スクリプトによる新しいツールを作成し、即座に登録して今後のセッションでも恒久的に利用可能にする。\
            スクリプトは stdin からツール引数の JSON を受け取り（読み取り方：$args = [Console]::In.ReadToEnd() | ConvertFrom-Json）、\
            結果を stdout に出力する。再利用可能な「実行可能」な能力が不足する場合（特定 API の呼び出しと出力整形など）に使う。\
            方法論の蓄積には create_skill、既存ツールの編成には run_workflow を使うこと。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Tool name: ASCII letters/digits/'_'/'-' only, 1-64 chars (e.g. 'fetch_github_issues')"
                },
                "description": {
                    "type": "string",
                    "description": "What the tool does and when to call it (shown in the tool list)"
                },
                "parameters": {
                    "type": "object",
                    "description": "JSON Schema (type: object) for the tool's input arguments; omit for no-arg tools"
                },
                "script": {
                    "type": "string",
                    "description": "PowerShell script: read JSON args from stdin ($args = [Console]::In.ReadToEnd() | ConvertFrom-Json), print result to stdout. Non-interactive, 120s timeout, output truncated at 8000 chars"
                },
                "deferred": {
                    "type": "boolean",
                    "description": "Injection tier: true = deferred (name-only listing, schema loaded via tool_search, saves tokens — recommended for niche tools); false/omit = always injected with full schema. Creation always requires user approval via preview card"
                }
            },
            "required": ["name", "description", "script"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "工具名：仅 ASCII 字母/数字/下划线/连字符，1-64 字符（如 'fetch_github_issues'）"
                    },
                    "description": {
                        "type": "string",
                        "description": "工具做什么、何时调用（显示在工具列表中）"
                    },
                    "parameters": {
                        "type": "object",
                        "description": "工具输入参数的 JSON Schema（type: object）；无参数工具可省略"
                    },
                    "script": {
                        "type": "string",
                        "description": "PowerShell 脚本：从 stdin 读取 JSON 参数（$args = [Console]::In.ReadToEnd() | ConvertFrom-Json），结果打印到 stdout。非交互、120 秒超时、输出超 8000 字符截断"
                    },
                    "deferred": {
                        "type": "boolean",
                        "description": "动态注入等级：true = 延迟加载（仅列名，经 tool_search 按需加载 schema，省 token，低频工具推荐）；false/省略 = 始终注入完整 schema。创建始终需要用户通过预览卡片授权"
                    }
                },
                "required": ["name", "description", "script"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "ツール名：ASCII 英数字・_・- のみ、1-64 文字（例：'fetch_github_issues'）"
                    },
                    "description": {
                        "type": "string",
                        "description": "ツールの機能と呼び出しタイミング（ツールリストに表示される）"
                    },
                    "parameters": {
                        "type": "object",
                        "description": "ツール入力引数の JSON Schema（type: object）。引数なしツールは省略可"
                    },
                    "script": {
                        "type": "string",
                        "description": "PowerShell スクリプト：stdin から JSON 引数を読み（$args = [Console]::In.ReadToEnd() | ConvertFrom-Json）、結果を stdout に出力。非対話・120 秒タイムアウト・出力は 8000 文字で切り詰め"
                    },
                    "deferred": {
                        "type": "boolean",
                        "description": "注入レベル：true = 遅延読み込み（名前のみ、tool_search で schema を読み込み、トークン節約、低頻度ツールに推奨）；false/省略 = 完全 schema を常時注入。作成には常にプレビューカードによるユーザー承認が必要"
                    }
                },
                "required": ["name", "description", "script"]
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
        let script = input
            .get("script")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        if name.is_empty() {
            return ValidationResult::failure("name 是必填项", 2);
        }
        if description.is_empty() {
            return ValidationResult::failure("description 是必填项", 2);
        }
        if script.is_empty() {
            return ValidationResult::failure("script 是必填项", 2);
        }
        // parameters 可选，但若提供必须是 object schema
        if let Some(params) = input.get("parameters") {
            if !params.is_null() {
                let type_ok = params
                    .get("type")
                    .and_then(Value::as_str)
                    .map(|t| t == "object")
                    .unwrap_or(true);
                if !params.is_object() || !type_ok {
                    return ValidationResult::failure(
                        "parameters 必须是 type 为 object 的 JSON Schema",
                        2,
                    );
                }
            }
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _context: &ToolUseContext) -> PermissionResult {
        // 强制走用户确认：新工具的创建必须经预览卡片授权（名称/描述/脚本/权限/注入等级）。
        // 权限矩阵在 FullControl 下会直接放行 Shell 级，因此这里显式返回 ask 保证必确认；
        // Bypass 模式在权限检查入口已被跳过（用户显式选择的全放行）。
        PermissionResult::ask("创建新工具需要用户授权：将弹出预览卡片展示工具详情")
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
        let parameters = args.get("parameters").cloned().unwrap_or(Value::Null);
        let script = args
            .get("script")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let deferred = args.get("deferred").and_then(|v| v.as_bool()).unwrap_or(false);

        // 用户已通过预览卡片授权（executor 权限门放行后才进入这里）
        match custom_tools::create_custom_tool(
            &self.tool_system,
            &name,
            &description,
            &parameters,
            &script,
            deferred,
        ) {
            Ok(registered) => ToolResult::standard_success(
                &format!(
                    "工具「{registered}」已创建并注册，现在就可以调用（传参方式见其 schema）。\
                     注入等级：{}。之后所有会话都能用它，每次调用都会请求用户确认。",
                    if deferred { "延迟加载（tool_search 按需加载）" } else { "始终注入完整 schema" }
                ),
                Some(json!({
                    "name": registered,
                    "registered": true,
                    "deferred": deferred,
                })),
            ),
            Err(e) => ToolResult::standard_error(&e, None, None),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        // 创建的是可执行能力（脚本文件 + 运行时注册），与 Shell 同级管控
        ToolRiskTier::Shell
    }

    fn is_destructive(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "create tool build custom capability script 自建工具 构建工具 能力进化 create_tool"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "沉淀做事方法论或流程指引（改用 create_skill，那是提示词级知识）",
            "组合既有工具完成多步任务（改用 run_workflow）",
            "一次性命令执行（改用 run_command）",
        ]
    }
}
