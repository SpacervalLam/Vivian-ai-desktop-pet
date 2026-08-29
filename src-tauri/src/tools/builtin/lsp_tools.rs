//! LSP 查询工具 — 把语义操作（定义跳转/引用/实现/hover）暴露为 LLM 工具。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::lsp::{LspQuery, LspQueryKind, LspService};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

use std::sync::Arc;

fn service() -> Arc<LspService> {
    use once_cell::sync::Lazy;
    static SVC: Lazy<Arc<LspService>> = Lazy::new(|| LspService::new());
    Arc::clone(&SVC)
}

/// LSP 语义查询工具。
pub struct LspQueryTool;

impl LspQueryTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LspQueryTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for LspQueryTool {
    fn name(&self) -> &str {
        "lsp_query"
    }

    fn description(&self) -> &str {
        "Run a semantic code query via a language server: go_to_definition / find_references / go_to_implementation / hover, at a file position (0-based line/column). Requires a language server configured for the file's extension (lsp.json). Returns locations or hover text."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "经语言服务器执行语义代码查询：go_to_definition（跳转定义）/ find_references（查找引用）/ go_to_implementation（跳转实现）/ hover（悬浮信息），按文件位置（0 基行/列）。需要该扩展配置了语言服务器（lsp.json）。返回位置列表或悬浮文本。",
            "ja" => "言語サーバー経由でセマンティックなコードクエリを実行：go_to_definition（定義へ移動）/ find_references（参照検索）/ go_to_implementation（実装へ移動）/ hover（ホバー情報）、ファイル位置（0 基行/列）で指定。拡張子に応じた言語サーバー設定（lsp.json）が必要。位置リストまたはホバーテキストを返す。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kind": {"type": "string", "enum": ["go_to_definition", "find_references", "go_to_implementation", "hover"], "description": "Query kind"},
                "file": {"type": "string", "description": "Absolute file path"},
                "line": {"type": "integer", "description": "0-based line"},
                "column": {"type": "integer", "description": "0-based column"}
            },
            "required": ["kind", "file", "line", "column"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "kind": {"type": "string", "enum": ["go_to_definition", "find_references", "go_to_implementation", "hover"], "description": "查询类型"},
                    "file": {"type": "string", "description": "文件绝对路径"},
                    "line": {"type": "integer", "description": "0 基行号"},
                    "column": {"type": "integer", "description": "0 基列号"}
                },
                "required": ["kind", "file", "line", "column"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "kind": {"type": "string", "enum": ["go_to_definition", "find_references", "go_to_implementation", "hover"], "description": "クエリ種別"},
                    "file": {"type": "string", "description": "ファイルの絶対パス"},
                    "line": {"type": "integer", "description": "0 基行番号"},
                    "column": {"type": "integer", "description": "0 基列番号"}
                },
                "required": ["kind", "file", "line", "column"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let kind_ok = matches!(
            input.get("kind").and_then(|v| v.as_str()),
            Some("go_to_definition") | Some("find_references") | Some("go_to_implementation") | Some("hover")
        );
        if !kind_ok {
            return ValidationResult::failure("kind 必须是 go_to_definition / find_references / go_to_implementation / hover", 2);
        }
        match input.get("file").and_then(|v| v.as_str()) {
            Some(f) if !f.is_empty() => {}
            _ => return ValidationResult::failure("file 是必填项", 2),
        }
        if input.get("line").and_then(|v| v.as_u64()).is_none()
            || input.get("column").and_then(|v| v.as_u64()).is_none()
        {
            return ValidationResult::failure("line / column 是必填项（0 基）", 2);
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let kind = match args.get("kind").and_then(|v| v.as_str()) {
            Some("go_to_definition") => LspQueryKind::GoToDefinition,
            Some("find_references") => LspQueryKind::FindReferences,
            Some("go_to_implementation") => LspQueryKind::GoToImplementation,
            _ => LspQueryKind::Hover,
        };
        let query = LspQuery {
            kind,
            file: args.get("file").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            line: args.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            column: args.get("column").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        };
        match service().query(&query).await {
            Ok(result) => ToolResult::success(json!({ "result": result, "kind": query.kind.as_str() })),
            Err(e) => ToolResult::standard_error(&format!("LSP 查询失败：{e}"), Some("LspQueryFailed"), None),
        }
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

    fn always_load(&self) -> bool {
        false
    }

    fn search_hint(&self) -> &str {
        "lsp definition references implementation hover code"
    }
}
