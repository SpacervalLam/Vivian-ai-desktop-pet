//! 媒体控制工具：播放/暂停、上一首、下一首、音量调节、静音。
//!
//! 播放类动作（play_pause / next / previous）优先走 SMTC（`world::MusicSource::control`），
//! 可定向到具体播放器（`target_app`）并有成功回执；失败时降级为媒体键。
//! 指定 `target_app` 时 SMTC 失败不降级（媒体键无法定向）。
//! 音量与静音无 SMTC 对应 API，始终用媒体键。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};
use crate::utils::process::silent_command;
use crate::world::{MusicSource, PlaybackAction};

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

/// 媒体控制工具：通过 `action` 参数发送对应的媒体键
///
/// 整合 play_pause / next_track / previous_track / volume_up / volume_down / mute 六个动作。
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
        "Control global media playback or system volume.\
         The action parameter specifies what to do:\
         play_pause, next_track, previous_track, volume_up, volume_down, mute.\n\
         Playback actions go through the system media session (so they can target a specific \
         player and report what ended up playing), falling back to media keys if that fails. \
         Volume and mute always use media keys.\n\
         Typical scenario: call when the user says \"play/pause\", \"next track\", \"volume up\", \"mute\"."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "控制全局媒体播放或系统音量。\
         action 参数指定动作：\
         play_pause（播放/暂停）、next_track（下一首）、previous_track（上一首）、\
         volume_up（音量增大）、volume_down（音量减小）、mute（静音切换）。\n\
         播放类动作优先走系统媒体会话（可定向到具体播放器、并能回报最终在放什么），\
         失败时降级为模拟媒体键；音量与静音始终用媒体键。\n\
         典型场景：当用户说\"播放/暂停\"、\"下一首\"、\"音量增大\"、\"静音\"时调用。\n\
         注意：想按歌名找歌并播放请用 music_play，不要用本工具。",
            "ja" => "グローバルなメディア再生やシステム音量を制御する。\
         action パラメータで動作を指定する：\
         play_pause（再生/一時停止）、next_track（次のトラック）、previous_track（前のトラック）、\
         volume_up（音量を上げる）、volume_down（音量を下げる）、mute（ミュート切り替え）。\n\
         再生系の操作はシステムメディアセッションを優先し（特定のプレイヤーを指定でき、\
         最終的に何が再生されているかを返せる）、失敗時はメディアキーにフォールバックする。\n\
         典型的なシナリオ：ユーザーが\"再生/一時停止\"\"次のトラック\"\"音量を上げて\"\"ミュート\"と言った時に呼び出す。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "放首歌\n暂停一下\n下一首\n音量小一点\n别放了\n静音",
            "en" => "play some music\npause it\nnext track\nturn it down\nstop playing",
            "ja" => "音楽をかけて\n一時停止して\n次の曲\n音量を下げて\n再生を止めて",
            _ => "",
        }
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Find a specific song by name and play it (use music_play instead)",
            "Just checking what is currently playing (use music_now_playing instead)",
        ]
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["play_pause", "next_track", "previous_track", "volume_up", "volume_down", "mute"],
                    "description": "Media key action to send."
                },
                "target_app": {
                    "type": "string",
                    "description": "Optional. Restrict the action to a specific player, matched against its app id (e.g. \"cloudmusic\", \"spotify\", \"qqmusic\"). Omit to use the system's current session."
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
                    },
                    "target_app": {
                        "type": "string",
                        "description": "可选。把动作限定到某个播放器（按应用标识匹配，如 \"cloudmusic\"、\"spotify\"、\"qqmusic\"）。省略则作用于系统当前会话。"
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
                    },
                    "target_app": {
                        "type": "string",
                        "description": "任意。動作を特定のプレイヤーに限定する（アプリ ID で照合、例 \"cloudmusic\"、\"spotify\"、\"qqmusic\"）。省略時はシステムの現在のセッション。"
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
        PermissionResult::ask("控制媒体（播放/暂停/音量/静音）需要用户确认")
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let target_app = args
            .get("target_app")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

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

        // 播放类动作优先走 SMTC：可定向到具体播放器、有成功回执、能读回曲名做校验。
        // 音量/静音没有 SMTC 对应 API，直接发媒体键。
        let is_playback = matches!(action, "play_pause" | "next_track" | "previous_track");
        if is_playback {
            if let Some(pa) = PlaybackAction::parse(action) {
                match MusicSource::new().control(pa, target_app.clone()).await {
                    Ok(snapshot) => {
                        let now = snapshot
                            .map(|s| format!("{} — {}", s.artist, s.title))
                            .unwrap_or_default();
                        let message = if now.is_empty() {
                            format!("已通过系统媒体会话执行「{}」", label)
                        } else {
                            format!("已执行「{}」，当前播放：{}", label, now)
                        };
                        return ToolResult::standard_success(
                            &message,
                            Some(json!({
                                "action": action,
                                "via": "smtc",
                                "now_playing": now,
                                "target_app": target_app,
                            })),
                        );
                    }
                    Err(e) => {
                        tracing::debug!("[MediaControl] SMTC 控制失败，准备降级媒体键: {}", e);
                        // 指定 target_app 时不降级：媒体键是全局的、无法定向，会误控其他播放器
                        if target_app.is_some() {
                            return ToolResult::standard_error(
                                &format!(
                                    "定向控制「{}」失败：{}。媒体键无法定向，故不做降级（避免误控其他播放器）。",
                                    target_app.as_deref().unwrap_or(""), e
                                ),
                                Some("MediaTargetedControlFailed"),
                                Some(json!({ "action": action, "target_app": target_app })),
                            );
                        }
                    }
                }
            }
        }

        match send_vk_async(vk).await {
            Ok(()) => ToolResult::standard_success(
                &format!("已发送 {}", label),
                Some(json!({
                    "key": label,
                    "vk": vk,
                    "action": action,
                    "via": "media_key",
                })),
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
        ToolRiskTier::FsWrite
    }
}
