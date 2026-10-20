//! 运行时环境工具：工作目录切换、依赖路径探测、运行时能力自省。
//!
//! 这三个工具解决的是同一类问题：**模型对"我现在在哪、手上有什么"缺少可靠来源**。
//! 没有它们时，模型只能靠路径猜工作目录、靠试错判断某个解释器在不在、
//! 靠提示词里的静态描述猜自己有哪些工具——三件事都会导致反复失败。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::commands::coding_agent::CODING_AGENT;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};
use crate::tools::ToolSystem;

// ============================================================================
// working_directory
// ============================================================================

/// 读取 / 切换当前会话的工作目录。
pub struct WorkingDirectoryTool;

impl WorkingDirectoryTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WorkingDirectoryTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WorkingDirectoryTool {
    fn name(&self) -> &str {
        "working_directory"
    }

    fn description(&self) -> &str {
        "Read the session's current working directory, or switch it to an existing directory. \
         Relative paths passed to other tools are resolved against this. \
         Switching also moves the sandbox boundary, so only switch to a directory you have been given."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "读取本会话的当前工作目录，或切换到一个已存在的目录。其他工具的相对路径都基于它解析。\
                     切换会同时移动沙箱边界——只切到用户授权过的目录。",
            "ja" => "セッションの現在の作業ディレクトリを読む、または既存ディレクトリへ切り替える。\
                     相対パスはこれを基準に解決される。切り替えるとサンドボックスの境界も動くので、許可されたディレクトリだけにすること。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "现在在哪个目录\n切到那个文件夹\n工作目录是什么",
            "en" => "what directory am I in\nchange to that folder\ncurrent working directory",
            "ja" => "今どのディレクトリ\nそのフォルダに移動\n作業ディレクトリは",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "要切到的目录（绝对路径；不传则只读取当前目录）" }
            },
            "required": []
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let target = args.get("path").and_then(|v| v.as_str()).map(|s| s.trim()).filter(|s| !s.is_empty());
        match target {
            None => ToolResult::standard_success(
                &format!("当前工作目录：{}", ctx.working_directory),
                Some(json!({"working_directory": ctx.working_directory})),
            ),
            Some(path) => match CODING_AGENT.set_work_directory(&ctx.session_id, path) {
                Ok(abs) => ToolResult::standard_success(
                    &format!("已切换工作目录到：{abs}（相对路径现在以它为基准；沙箱边界同步移动）"),
                    Some(json!({"working_directory": abs})),
                ),
                Err(e) => ToolResult::standard_error(&e, Some("InvalidInput"), None),
            },
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::File
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsWrite
    }

    fn search_hint(&self) -> &str {
        "working directory cd pwd 工作目录 切换目录 当前路径"
    }
}

// ============================================================================
// load_workspace_dependencies
// ============================================================================

/// 探测本机可用的运行时与常见工具链绝对路径。
pub struct LoadWorkspaceDependenciesTool;

impl LoadWorkspaceDependenciesTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LoadWorkspaceDependenciesTool {
    fn default() -> Self {
        Self::new()
    }
}

/// 在 PATH 里找可执行文件（Windows 按 PATHEXT 补后缀）。
fn which(bin: &str) -> Option<PathBuf> {
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".to_string())
            .split(';')
            .map(|s| s.to_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = if ext.is_empty() {
                dir.join(bin)
            } else if bin.to_lowercase().ends_with(ext.as_str()) {
                dir.join(bin)
            } else {
                dir.join(format!("{bin}{ext}"))
            };
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[async_trait]
impl Tool for LoadWorkspaceDependenciesTool {
    fn name(&self) -> &str {
        "load_workspace_dependencies"
    }

    fn description(&self) -> &str {
        "Report absolute paths of the runtimes and toolchains available on this machine (python, node, npm, pnpm, \
         git, cargo, rg, powershell). Call it before assuming an interpreter exists — guessing turns a one-second \
         check into three failed commands."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "报告本机可用运行时与工具链的绝对路径（python / node / npm / pnpm / git / cargo / rg / powershell）。\
                     在假设某个解释器存在之前先调它——猜错会把一秒的检查变成三条失败命令。",
            "ja" => "このマシンで使えるランタイムとツールチェーンの絶対パスを報告する（python / node / npm / pnpm / git / cargo / rg / powershell）。\
                     存在を仮定する前に呼ぶこと。推測は 1 秒の確認を 3 回の失敗コマンドに変える。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "required": []})
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, _args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let probes = [
            "python", "python3", "node", "npm", "pnpm", "yarn", "git", "cargo", "rg", "powershell",
            "pwsh",
        ];
        let mut found = serde_json::Map::new();
        let mut text = String::from("本机可用的运行时/工具链：\n");
        for bin in probes {
            match which(bin) {
                Some(p) => {
                    text.push_str(&format!("- {bin}: {}\n", p.display()));
                    found.insert(bin.to_string(), json!(p.display().to_string()));
                }
                None => {
                    text.push_str(&format!("- {bin}: 未找到\n"));
                }
            }
        }
        text.push_str(
            "\n提示：用绝对路径调用最稳（尤其 Python，不同 venv 的包完全不同）。\
             项目若有锁文件（package-lock.json / pnpm-lock.yaml / Cargo.lock），优先用对应包管理器。",
        );
        ToolResult::standard_success(&text, Some(Value::Object(found)))
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "dependencies runtime python node path 环境 依赖 解释器 找不全"
    }
}

// ============================================================================
// runtime_inspect_list / runtime_inspect_query
// ============================================================================

/// 运行时自省：列出已注册的工具（含未暴露给本轮的）。
pub struct RuntimeInspectListTool {
    system: Weak<ToolSystem>,
}

impl RuntimeInspectListTool {
    pub fn new(system: Weak<ToolSystem>) -> Self {
        Self { system }
    }

    fn live(&self) -> Vec<Arc<dyn Tool>> {
        self.system.upgrade().map(|ts| ts.list_tools()).unwrap_or_default()
    }
}

#[async_trait]
impl Tool for RuntimeInspectListTool {
    fn name(&self) -> &str {
        "runtime_inspect_list"
    }

    fn description(&self) -> &str {
        "List every tool registered in the runtime, including ones not exposed to you this turn, with category, \
         risk tier and whether it is read-only. Use it to answer \"does this capability exist at all?\" — \
         a tool missing from your schema may still be registered but gated by mode or config."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "列出运行时注册的**全部**工具（含本轮没暴露给你的），带类别、风险等级、是否只读。\
                     用来回答「这个能力到底存不存在」——schema 里没有，可能只是被模式或配置挡了，注册表里仍然在。",
            "ja" => "ランタイムに登録されている**全**ツールを一覧表示する（今回公開されていないものも含む）。カテゴリ・リスク・読み取り専用かどうか付き。\
                     「この能力は存在するのか」を確かめるのに使う。schema に無くてもモードや設定で隠れているだけの場合がある。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "category": { "type": "string", "description": "可选：按类别过滤（file / system / memory / mcp 等）" }
            },
            "required": []
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let category = args.get("category").and_then(|v| v.as_str()).map(|s| s.to_lowercase());
        let exposed: std::collections::HashSet<String> =
            ctx.allowed_tools.clone().unwrap_or_default().into_iter().collect();
        let mut rows: Vec<Value> = Vec::new();
        let mut text = String::new();
        for t in self.live() {
            let cat = format!("{:?}", t.category()).to_lowercase();
            if let Some(c) = &category {
                if &cat != c {
                    continue;
                }
            }
            let name = t.name().to_string();
            let is_exposed = exposed.contains(&name);
            text.push_str(&format!(
                "- {name} [{cat}] risk={:?} read_only={} {}\n",
                t.risk(),
                t.is_read_only(),
                if is_exposed { "（本轮可用）" } else { "（本轮未暴露）" }
            ));
            rows.push(json!({
                "name": name, "category": cat,
                "risk": format!("{:?}", t.risk()),
                "read_only": t.is_read_only(),
                "exposed": is_exposed,
            }));
        }
        if rows.is_empty() {
            return ToolResult::standard_success("没有匹配的工具。", Some(json!({"tools": []})));
        }
        let header = format!("运行时注册工具：{} 个\n", rows.len());
        ToolResult::standard_success(&format!("{header}{text}"), Some(json!({"tools": rows})))
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "runtime inspect tools registry 注册表 自省 有哪些能力"
    }
}

/// 运行时自省：按名字查一个工具的真实定义。
pub struct RuntimeInspectQueryTool {
    system: Weak<ToolSystem>,
}

impl RuntimeInspectQueryTool {
    pub fn new(system: Weak<ToolSystem>) -> Self {
        Self { system }
    }
}

#[async_trait]
impl Tool for RuntimeInspectQueryTool {
    fn name(&self) -> &str {
        "runtime_inspect_query"
    }

    fn description(&self) -> &str {
        "Inspect one registered tool by name: description, parameter schema, category, risk tier, read-only flag. \
         Works even for tools not exposed this turn, so it answers \"why can't I call this\"."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "按名字查一个已注册工具的真实定义：描述、参数 schema、类别、风险等级、是否只读。\
                     本轮没暴露的工具也能查——用来回答「这个我为什么调不了」。",
            "ja" => "名前で登録済みツール 1 件を調べる：説明・パラメータ schema・カテゴリ・リスク・読み取り専用か。\
                     今回公開されていないツールも調べられる（「なぜ呼べないのか」に答える）。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "name": { "type": "string", "description": "工具名（精确匹配）" } },
            "required": ["name"]
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().is_empty() {
            return ValidationResult::failure("name 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
        let Some(system) = self.system.upgrade() else {
            return ToolResult::standard_error("工具系统未就绪。", None, None);
        };
        let Some(tool) = system.find_tool(name) else {
            return ToolResult::standard_error(
                &format!("运行时没有注册名为 {name} 的工具。用 runtime_inspect_list 看全部名称。"),
                Some("ToolNotFound"),
                None,
            );
        };
        let exposed = ctx
            .allowed_tools
            .as_ref()
            .map(|list| list.iter().any(|n| n == name))
            .unwrap_or(true);
        let schema = tool.parameters_schema();
        let text = format!(
            "{name}\n类别：{:?}\n风险：{:?}\n只读：{}\n本轮可用：{}\n\n说明：{}\n\n参数：{}",
            tool.category(),
            tool.risk(),
            tool.is_read_only(),
            if exposed { "是" } else { "否（被模式或配置挡住）" },
            tool.description(),
            serde_json::to_string_pretty(&schema).unwrap_or_default()
        );
        ToolResult::standard_success(
            &text,
            Some(json!({
                "name": name,
                "category": format!("{:?}", tool.category()),
                "risk": format!("{:?}", tool.risk()),
                "read_only": tool.is_read_only(),
                "exposed": exposed,
                "description": tool.description(),
                "parameters": schema,
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "runtime inspect query tool schema 查工具 为什么调不了"
    }
}

/// 供测试：确认路径探测不会 panic。
pub fn probe_all() -> Vec<(String, Option<PathBuf>)> {
    ["python", "node", "git"]
        .iter()
        .map(|b| (b.to_string(), which(b)))
        .collect()
}

/// 供外部引用（避免 Path 导入被误判为未使用）。
pub fn is_directory(p: &Path) -> bool {
    p.is_dir()
}
