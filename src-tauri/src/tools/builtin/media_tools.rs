//! 媒体控制工具 - 播放/暂停、上一首、下一首、音量调节、静音
//!
//! 通过 Windows keybd_event 模拟媒体键，控制全局媒体播放与音量。
//! 通过统一的 action 参数分发到不同的媒体键。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};
use crate::utils::process::silent_command;

/// 通过 PowerShell 调用 keybd_event 发送一次按键（down + up）
fn send_vk(vk: u8) -> Result<(), String> {
    let script = format!(
        r#"Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public class K{{[DllImport("user32.dll")]public static extern void keybd_event(byte bVk,byte bScan,int dwFlags,int dwExtraInfo);}}';
[K]::keybd_event({vk},0,0,0);
[K]::keybd_event({vk},0,2,0);
"#
    );
    let output = silent_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| format!("启动 PowerShell 失败: {}", e))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// 在阻塞线程池中执行 send_vk，避免同步子进程阻塞异步运行时工作线程
async fn send_vk_async(vk: u8) -> Result<(), String> {
    tokio::task::spawn_blocking(move || send_vk(vk))
        .await
        .map_err(|e| format!("媒体键任务执行失败: {}", e))?
}

// VK 常量
const VK_MEDIA_PLAY_PAUSE: u8 = 0xB3;
const VK_MEDIA_NEXT_TRACK: u8 = 0xB0;
const VK_MEDIA_PREV_TRACK: u8 = 0xB1;
const VK_VOLUME_UP: u8 = 0xAF;
const VK_VOLUME_DOWN: u8 = 0xAE;
const VK_VOLUME_MUTE: u8 = 0xAD;

/// 将 action 字符串映射到 (VK 码, 标签)
fn map_action(action: &str) -> Option<(u8, &'static str)> {
    match action {
        "play_pause" => Some((VK_MEDIA_PLAY_PAUSE, "Play/Pause")),
        "next_track" => Some((VK_MEDIA_NEXT_TRACK, "Next Track")),
        "previous_track" => Some((VK_MEDIA_PREV_TRACK, "Previous Track")),
        "volume_up" => Some((VK_VOLUME_UP, "Volume Up")),
        "volume_down" => Some((VK_VOLUME_DOWN, "Volume Down")),
        "mute" => Some((VK_VOLUME_MUTE, "Mute")),
        _ => None,
    }
}

/// 媒体控制工具：通过 action 参数发送对应的媒体键
///
/// 整合原有的 6 个媒体键工具（play_pause / next_track / previous_track /
/// volume_up / volume_down / mute），减少 tool 数量与 token 开销。
pub struct MediaControlTool;

impl MediaControlTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MediaControlTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for MediaControlTool {
    fn name(&self) -> &str {
        "media_control"
    }

    fn description(&self) -> &str {
        "Send a media key to control global media playback or system volume.\
         The action parameter specifies which media key to send:\
         play_pause, next_track, previous_track, volume_up, volume_down, mute.\n\
         Typical scenario: call when the user says \"play/pause\", \"next track\", \"volume up\", \"mute\"."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "发送媒体键控制全局媒体播放或系统音量。\
         action 参数指定要发送的媒体键：\
         play_pause（播放/暂停）、next_track（下一首）、previous_track（上一首）、\
         volume_up（音量增大）、volume_down（音量减小）、mute（静音切换）。\n\
         典型场景：当用户说\"播放/暂停\"、\"下一首\"、\"音量增大\"、\"静音\"时调用。",
            "ja" => "メディアキーを送信してグローバルなメディア再生やシステム音量を制御する。\
         action パラメータで送信するメディアキーを指定する：\
         play_pause（再生/一時停止）、next_track（次のトラック）、previous_track（前のトラック）、\
         volume_up（音量を上げる）、volume_down（音量を下げる）、mute（ミュート切り替え）。\n\
         典型的なシナリオ：ユーザーが\"再生/一時停止\"\"次のトラック\"\"音量を上げて\"\"ミュート\"と言った時に呼び出す。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["play_pause", "next_track", "previous_track", "volume_up", "volume_down", "mute"],
                    "description": "Media key action to send."
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
                        "enum": ["play_pause", "next_track", "previous_track", "volume_up", "volume_down", "mute"],
                        "description": "要发送的媒体键动作。"
                    }
                },
                "required": ["action"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["play_pause", "next_track", "previous_track", "volume_up", "volume_down", "mute"],
                        "description": "送信するメディアキーアクション。"
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
            Some(a) if map_action(a).is_some() => ValidationResult::success(None),
            Some(a) => ValidationResult::failure(
                &format!(
                    "不支持的 action: {}（可选：play_pause / next_track / previous_track / volume_up / volume_down / mute）",
                    a
                ),
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
        let (vk, label) = match map_action(action) {
            Some(v) => v,
            None => {
                return ToolResult::standard_error(
                    &format!("不支持的 action: {}", action),
                    Some("InvalidAction"),
                    None,
                );
            }
        };

        match send_vk_async(vk).await {
            Ok(()) => ToolResult::standard_success(
                &format!("已发送 {}", label),
                Some(json!({ "key": label, "vk": vk, "action": action })),
            ),
            Err(e) => ToolResult::standard_error(
                "媒体键发送失败",
                Some(&e),
                Some(json!({ "key": label, "action": action })),
            ),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Media
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::InputControl
    }
}
