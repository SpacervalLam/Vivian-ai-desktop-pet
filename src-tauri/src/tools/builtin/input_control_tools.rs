//! 输入控制工具 - 鼠标点击、键盘快捷键、文本输入
//!
//! 使用持久原生执行器；兼容旧工具的屏幕物理坐标。
//!
//! 注意：这些工具具有破坏性（is_destructive = true），需要权限确认。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};


async fn native_action(args: Value, action: &str, ctx: &ToolUseContext) -> ToolResult {
    let mut args=args;
    args["action"]=json!(action);
    let mut action=match serde_json::from_value::<crate::desktop_native::Action>(args) {
        Ok(action)=>action, Err(error)=>return ToolResult::error(error.to_string()),
    };
    action.absolute_coordinates=true;
    if let Err(error)=action.validate() {return ToolResult::error(error);}
    match crate::desktop_native::request(format!("{}:{}",ctx.char_id,ctx.session_id),crate::desktop_native::Request::Perform(action),super::desktop_runtime::config()).await {
        Ok(reply)=>ToolResult::success(reply.data),Err(error)=>ToolResult::error(error),
    }
}

// ===== click_mouse =====

pub struct ClickMouseTool;

impl ClickMouseTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ClickMouseTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ClickMouseTool {
    fn name(&self) -> &str {
        "click_mouse"
    }

    fn description(&self) -> &str {
        "Move the mouse to (x, y) and click. button: left/right/double (default left)."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "将鼠标移动到 (x, y) 并点击。button：left/right/double（默认 left）。",
            "ja" => "マウスを (x, y) に移動してクリックする。button：left/right/double（デフォルト left）。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "点一下那里\n帮我点击\n鼠标点这个位置",
            "en" => "click there\nclick for me\nclick at this position",
            "ja" => "そこをクリックして\nクリックして\nこの位置を押して",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "x": {"type": "integer", "description": "Screen X coordinate"},
                "y": {"type": "integer", "description": "Screen Y coordinate"},
                "button": {"type": "string", "enum": ["left", "right", "double"], "default": "left"}
            },
            "required": ["x", "y"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "x": {"type": "integer", "description": "屏幕 X 坐标"},
                    "y": {"type": "integer", "description": "屏幕 Y 坐标"},
                    "button": {"type": "string", "enum": ["left", "right", "double"], "default": "left"}
                },
                "required": ["x", "y"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "x": {"type": "integer", "description": "画面の X 座標"},
                    "y": {"type": "integer", "description": "画面の Y 座標"},
                    "button": {"type": "string", "enum": ["left", "right", "double"], "default": "left"}
                },
                "required": ["x", "y"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        if input.get("x").and_then(|v| v.as_i64()).is_none() {
            return ValidationResult::failure("x 必须是整数", 2);
        }
        if input.get("y").and_then(|v| v.as_i64()).is_none() {
            return ValidationResult::failure("y 必须是整数", 2);
        }
        if let Some(btn) = input.get("button").and_then(|v| v.as_str()) {
            if !matches!(btn, "left" | "right" | "double") {
                return ValidationResult::failure(
                    "button 必须是 left/right/double 之一",
                    2,
                );
            }
        }
        if let Err(error) = super::desktop_runtime::validate_point(input["x"].as_i64().unwrap(), input["y"].as_i64().unwrap()) {
            return ValidationResult::failure(error, 2);
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        native_action(args,"click",_ctx).await
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_destructive(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::InputControl
    }

    // 长尾工具：延迟加载，需通过 tool_search 唤起
    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "click mouse"
    }
}

// ===== hotkey =====

pub struct HotkeyTool;

impl HotkeyTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HotkeyTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for HotkeyTool {
    fn name(&self) -> &str {
        "hotkey"
    }

    fn description(&self) -> &str {
        "Press a keyboard shortcut. keys: a string of key names joined by '+', supporting single keys (e.g. \"enter\") and combinations (e.g. \"ctrl+c\", \"shift+tab\")."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "按下键盘快捷键。keys：以 '+' 连接的键名字符串，支持单键（如 \"enter\"）和组合键（如 \"ctrl+c\"、\"shift+tab\"）。",
            "ja" => "キーボードショートカットを押す。keys：'+' で接続されたキー名の文字列、単一キー（例: \"enter\"）と組み合わせ（例: \"ctrl+c\"、\"shift+tab\"）をサポート。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "按个快捷键\n帮我按 Ctrl C\n发送组合键",
            "en" => "press a hotkey\nhit ctrl c\nsend a key combination",
            "ja" => "ショートカットを押して\nCtrl+Cを押して\nキーを組み合わせて",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "keys": {
                    "type": "string",
                    "description": "Key names joined by '+', e.g. \"ctrl+c\", \"enter\", \"shift+tab\""
                }
            },
            "required": ["keys"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "keys": {
                        "type": "string",
                        "description": "以 '+' 连接的键名，例如 \"ctrl+c\"、\"enter\"、\"shift+tab\""
                    }
                },
                "required": ["keys"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "keys": {
                        "type": "string",
                        "description": "'+' で接続されたキー名、例: \"ctrl+c\"、\"enter\"、\"shift+tab\""
                    }
                },
                "required": ["keys"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let keys = match input.get("keys").and_then(|v| v.as_str()) {
            Some(k) if !k.is_empty() => k.to_string(),
            _ => return ValidationResult::failure("keys 是必填项", 2),
        };
        if let Err(error)=crate::desktop_native::parse_keys(&keys) {
            return ValidationResult::failure(error,2);
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        native_action(args,"key",_ctx).await
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_destructive(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::InputControl
    }

    // 长尾工具：延迟加载，需通过 tool_search 唤起
    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "hotkey combination"
    }
}

// ===== type_text =====

pub struct TypeTextTool;

impl TypeTextTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TypeTextTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TypeTextTool {
    fn name(&self) -> &str {
        "type_text"
    }

    fn description(&self) -> &str {
        "Simulate typing a piece of text via the keyboard. text: the text to type (Unicode supported without changing the clipboard)."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "通过键盘模拟输入一段文本。text：要输入的文本（支持中文，不修改剪贴板）。",
            "ja" => "キーボードでテキストの入力をシミュレートする。text：入力するテキスト（Unicode 対応、クリップボードを変更しない）。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "帮我打点字\n输入这段话\n打字进去",
            "en" => "type this for me\nenter this text\ntype it in",
            "ja" => "これを入力して\nこの文章を打って\nタイピングして",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "Text to type"}
            },
            "required": ["text"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "要输入的文本"}
                },
                "required": ["text"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "入力するテキスト"}
                },
                "required": ["text"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        match input.get("text").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() && t.chars().count() <= 4000 => ValidationResult::success(Some(input.clone())),
            _ => ValidationResult::failure("text 是必填项", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        native_action(args,"type",_ctx).await
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_destructive(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::InputControl
    }

    // 长尾工具：延迟加载，需通过 tool_search 唤起
    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "type text"
    }
}
