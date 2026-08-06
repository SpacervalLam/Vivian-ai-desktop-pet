//! 输入控制工具 - 鼠标点击、键盘快捷键、文本输入
//!
//! 通过 PowerShell P/Invoke 调用 user32.dll 模拟鼠标与键盘输入。
//!
//! 注意：这些工具具有破坏性（is_destructive = true），需要权限确认。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};
use crate::utils::process::silent_command;

fn run_ps(script: &str) -> Result<String, String> {
    let wrapped = format!(
        "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; $OutputEncoding = [System.Text.Encoding]::UTF8; {}",
        script
    );
    let output = silent_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &wrapped])
        .output()
        .map_err(|e| format!("启动 PowerShell 失败: {}", e))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// 在阻塞线程池中执行 run_ps，避免同步子进程阻塞异步运行时工作线程
async fn run_ps_async(script: &str) -> Result<String, String> {
    let script = script.to_string();
    tokio::task::spawn_blocking(move || run_ps(&script))
        .await
        .map_err(|e| format!("PowerShell 任务执行失败: {}", e))?
}

/// user32 P-Invoke 类型定义片段
const USER32_TYPES: &str = r#"
Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public class U{[DllImport("user32.dll")]public static extern bool SetCursorPos(int x,int y);[DllImport("user32.dll")]public static extern void mouse_event(int dwFlags,int dx,int dy,int cButtons,int dwExtraInfo);[DllImport("user32.dll")]public static extern void keybd_event(byte bVk,byte bScan,int dwFlags,int dwExtraInfo);}';
"#;

// mouse_event 标志
const MOUSEEVENTF_LEFTDOWN: i32 = 0x0002;
const MOUSEEVENTF_LEFTUP: i32 = 0x0004;
const MOUSEEVENTF_RIGHTDOWN: i32 = 0x0008;
const MOUSEEVENTF_RIGHTUP: i32 = 0x0010;

/// 把键名解析为 VK 码
fn key_to_vk(key: &str) -> Option<u8> {
    let lower = key.to_lowercase();
    let vk = match lower.as_str() {
        // 字母
        "a" => 0x41, "b" => 0x42, "c" => 0x43, "d" => 0x44, "e" => 0x45,
        "f" => 0x46, "g" => 0x47, "h" => 0x48, "i" => 0x49, "j" => 0x4A,
        "k" => 0x4B, "l" => 0x4C, "m" => 0x4D, "n" => 0x4E, "o" => 0x4F,
        "p" => 0x50, "q" => 0x51, "r" => 0x52, "s" => 0x53, "t" => 0x54,
        "u" => 0x55, "v" => 0x56, "w" => 0x57, "x" => 0x58, "y" => 0x59, "z" => 0x5A,
        // 数字
        "0" => 0x30, "1" => 0x31, "2" => 0x32, "3" => 0x33, "4" => 0x34,
        "5" => 0x35, "6" => 0x36, "7" => 0x37, "8" => 0x38, "9" => 0x39,
        // 控制键
        "enter" | "return" => 0x0D,
        "escape" | "esc" => 0x1B,
        "backspace" => 0x08,
        "tab" => 0x09,
        "space" => 0x20,
        "ctrl" | "control" => 0x11,
        "alt" => 0x12,
        "shift" => 0x10,
        "win" | "meta" => 0x5B,
        "up" => 0x26,
        "down" => 0x28,
        "left" => 0x25,
        "right" => 0x27,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "delete" | "del" => 0x2E,
        "insert" => 0x2D,
        "f1" => 0x70, "f2" => 0x71, "f3" => 0x72, "f4" => 0x73,
        "f5" => 0x74, "f6" => 0x75, "f7" => 0x76, "f8" => 0x77,
        "f9" => 0x78, "f10" => 0x79, "f11" => 0x7A, "f12" => 0x7B,
        _ => return None,
    };
    Some(vk)
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
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let x = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
        let y = args.get("y").and_then(|v| v.as_i64()).unwrap_or(0);
        let button = args.get("button").and_then(|v| v.as_str()).unwrap_or("left");

        let (down, up) = match button {
            "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            _ => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        };

        let single = format!(
            "[U]::mouse_event({},0,0,0,0); [U]::mouse_event({},0,0,0,0);",
            down, up
        );
        // double 执行两次左键点击序列
        let body = if button == "double" {
            format!("{} {}", single, single)
        } else {
            single
        };
        let script = format!(
            "{}; [U]::SetCursorPos({},{}) | Out-Null; {}",
            USER32_TYPES.trim(),
            x,
            y,
            body
        );
        match run_ps_async(&script).await {
            Ok(_) => ToolResult::standard_success(
                "已点击鼠标",
                Some(json!({ "x": x, "y": y, "button": button })),
            ),
            Err(e) => ToolResult::standard_error("点击鼠标失败", Some(&e), None),
        }
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
        for part in keys.split('+') {
            let part = part.trim();
            if part.is_empty() {
                return ValidationResult::failure("keys 格式错误：'+' 两侧不能为空", 2);
            }
            if key_to_vk(part).is_none() {
                return ValidationResult::failure(
                    format!("不支持的键名: {}（支持字母/数字/控制键如 enter/escape/up/f1）", part),
                    500,
                );
            }
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let keys_str = args.get("keys").and_then(|v| v.as_str()).unwrap_or("");
        let keys: Vec<String> = keys_str
            .split('+')
            .map(|s| s.trim().to_string())
            .collect();

        let vks: Vec<u8> = keys.iter().filter_map(|k| key_to_vk(k)).collect();

        let mut down_script = String::new();
        let mut up_script = String::new();

        for vk in &vks {
            down_script.push_str(&format!("[U]::keybd_event({},0,0,0); ", vk));
        }
        for vk in vks.iter().rev() {
            up_script.push_str(&format!("[U]::keybd_event({},0,2,0); ", vk));
        }

        let script = format!("{}; {} {}", USER32_TYPES.trim(), down_script, up_script);

        match run_ps_async(&script).await {
            Ok(_) => ToolResult::standard_success(
                &format!("已按下快捷键 {}", keys.join(" + ")),
                Some(json!({ "keys": keys })),
            ),
            Err(e) => ToolResult::standard_error("快捷键失败", Some(&e), None),
        }
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
        "Simulate typing a piece of text via the keyboard. text: the text to type (Chinese supported, pasted via clipboard)."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "通过键盘模拟输入一段文本。text：要输入的文本（支持中文，通过剪贴板粘贴）。",
            "ja" => "キーボードでテキストの入力をシミュレートする。text：入力するテキスト（中国語対応、クリップボード経由で貼り付け）。",
            _ => self.description(),
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
            Some(t) if !t.is_empty() => ValidationResult::success(Some(input.clone())),
            _ => ValidationResult::failure("text 是必填项", 2),
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
        // 用剪贴板粘贴的方式输入（支持中文）
        let escaped = text.replace('\'', "''");
        let script = format!(
            r#"Set-Clipboard -Value '{}';
Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public class U{{[DllImport("user32.dll")]public static extern void keybd_event(byte bVk,byte bScan,int dwFlags,int dwExtraInfo);}}';
[U]::keybd_event(0x11,0,0,0);
[U]::keybd_event(0x56,0,0,0);
[U]::keybd_event(0x56,0,2,0);
[U]::keybd_event(0x11,0,2,0);
"#,
            escaped
        );
        match run_ps_async(&script).await {
            Ok(_) => ToolResult::standard_success(
                "已输入文本",
                Some(json!({ "text": text, "length": text.chars().count() })),
            ),
            Err(e) => ToolResult::standard_error("输入文本失败", Some(&e), None),
        }
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
