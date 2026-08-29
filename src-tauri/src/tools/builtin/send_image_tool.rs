//! 图片发送工具 - 智能体把本地图片发送到聊天界面（微信面板 / 编程页）
//!
//! 调用场景：
//! - 用户想看某张本地图片（项目里的图、生成的图表、表情包文件）
//! - 智能体截屏后把截图发给用户（与 take_screenshot 配合）
//! - 编程智能体产出图片产物（渲染结果、构建产物截图）时展示给用户
//!
//! 双通道路由（按 ToolUseContext.session_id 是否命中编程会话）：
//! - 编程页：图片作为 assistant 消息追加进 coding 会话（随会话持久化），
//!   emit `coding:assistant_message`（携带 images）供编程页实时渲染
//! - 微信面板：图片写入对话历史（metadata 标记 kind=image + image_path），
//!   emit `chat:assistant_image` 实时插入图片气泡；chat 窗口不可见时弹横幅；
//!   caption 作为独立助手消息经 `chat:assistant_message` 跟进

use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::brain::coding_agent::CodingImage;
use crate::commands::coding_agent::CODING_AGENT;
use crate::state::AppState;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};
use crate::types::response::ChatMessage as DialogChatMessage;

/// 全局 AppHandle（由 lib.rs setup 注入）
static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用一次）
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// 发送图片到微信聊天面板。
///
/// 流程（镜像 `send_image_message` 的用户侧管线与 `share_link_to_wechat` 的播报管线）：
/// 1. 图片副本保存到 `<user_data_dir>/images/`（历史重载时按 image_path 懒加载）
/// 2. 写入对话历史：assistant 图片消息（metadata kind=image + image_path）
/// 3. emit `chat:assistant_image`：前端实时插入 AI 图片气泡
/// 4. 若 chat 窗口未可见，emit `wechat:message_banner` 横幅提示
/// 5. caption 非空时作为跟进助手消息写入历史并 emit `chat:assistant_message`
fn send_image_to_wechat(
    app_handle: &AppHandle,
    char_id: &str,
    mime: &str,
    b64: &str,
    rel_path: &str,
    caption: &str,
) {
    let now = chrono::Local::now().to_rfc3339();
    let data_url = format!("data:{};base64,{}", mime, b64);

    // 写入对话历史（assistant 图片消息，metadata 标记图片）
    if let Some(state) = app_handle.try_state::<Arc<AppState>>() {
        let characters = state.characters.read();
        if let Some(instance) = characters.get(char_id) {
            let mut img_msg = DialogChatMessage::assistant("📷 [图片]");
            img_msg.meta =
                Some(crate::messages::MessageMeta::assistant().with_channel("wechat"));
            let _ = instance.brain.dialogue.add_message_with_metadata(
                img_msg,
                json!({
                    "kind": "image",
                    "image_path": rel_path,
                    "channel": "wechat",
                    "role": "assistant",
                }),
            );

            // caption 作为跟进评论记录
            if !caption.is_empty() {
                let mut follow_msg = DialogChatMessage::assistant(caption);
                follow_msg.meta =
                    Some(crate::messages::MessageMeta::assistant().with_channel("wechat"));
                let _ = instance
                    .brain
                    .dialogue
                    .add_message_with_metadata(follow_msg, json!({}));
            }
        }
    }

    // 实时插入 AI 图片气泡
    let _ = app_handle.emit(
        "chat:assistant_image",
        json!({
            "data_url": data_url,
            "image_path": rel_path,
            "timestamp": now,
            "character_id": char_id,
            "channel": "wechat",
        }),
    );

    // 若 chat 窗口（微信主界面）未可见，emit 消息横幅提示用户
    let need_banner = match app_handle.get_webview_window("chat") {
        Some(win) => !win.is_visible().ok().unwrap_or(false),
        None => true,
    };
    if need_banner {
        let _ = app_handle.emit(
            "wechat:message_banner",
            json!({
                "character_id": char_id,
                "preview": if caption.is_empty() { "[图片]".to_string() } else { caption.to_string() },
                "kind": "image",
                "timestamp": now,
            }),
        );
    }

    // caption 实时显示（ChatWindow 已监听 chat:assistant_message）
    if !caption.is_empty() {
        let _ = app_handle.emit(
            "chat:assistant_message",
            json!({
                "content": caption,
                "timestamp": now,
                "character_id": char_id,
                "channel": "wechat",
            }),
        );
    }
}

/// 发送图片工具。
pub struct SendImageTool;

impl SendImageTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SendImageTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SendImageTool {
    fn name(&self) -> &str {
        "send_image"
    }

    fn description(&self) -> &str {
        "Send a local image file to the user in the chat interface, rendered as an image bubble. Use it when the user wants to see a picture: a screenshot you just took, a generated image/chart, or an image file in the project. The path must point to an existing image file (png/jpg/jpeg/gif/webp/bmp). Optionally include a short caption to say something about the image."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "把本地图片文件发送到聊天界面，渲染为图片气泡。当用户想看图片时使用：你刚截的屏、生成的图片/图表、项目里的图片文件等。路径必须指向真实存在的图片文件（png/jpg/jpeg/gif/webp/bmp）。可选附带一句简短说明。",
            "ja" => "ローカル画像ファイルをチャット画面に送信し、画像バブルとして表示する。ユーザーが画像を見たがる場面——撮った直後のスクリーンショット、生成した画像・チャート、プロジェクト内の画像ファイル——で使用する。パスは実在する画像ファイル（png/jpg/jpeg/gif/webp/bmp）を指す必要がある。任意で短い説明を添えられる。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the local image file (png/jpg/jpeg/gif/webp/bmp). Relative paths are resolved against the working directory."
                },
                "caption": {
                    "type": "string",
                    "description": "Optional short caption (one sentence) shown to the user alongside the image"
                }
            },
            "required": ["path"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "本地图片文件的绝对路径（png/jpg/jpeg/gif/webp/bmp），相对路径按工作目录解析"
                    },
                    "caption": {
                        "type": "string",
                        "description": "可选简短说明（一句话），随图片一起展示给用户"
                    }
                },
                "required": ["path"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "ローカル画像ファイルの絶対パス（png/jpg/jpeg/gif/webp/bmp）。相対パスは作業ディレクトリ基準で解決される"
                    },
                    "caption": {
                        "type": "string",
                        "description": "任意の短い説明（1文）、画像と共にユーザーに表示される"
                    }
                },
                "required": ["path"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("").trim();
        if path.is_empty() {
            return ValidationResult::failure("path 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let caption = args
            .get("caption")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        let app_handle = match APP_HANDLE.read().clone() {
            Some(h) => h,
            None => {
                return ToolResult::standard_error("AppHandle 未初始化", Some("app handle not set"), None);
            }
        };

        // 路径解析：相对路径按工作目录（编程会话即工作区）解析
        let raw = std::path::PathBuf::from(&path);
        let src = if raw.is_absolute() {
            raw
        } else if !ctx.working_directory.is_empty() {
            std::path::Path::new(&ctx.working_directory).join(raw)
        } else {
            raw
        };
        let file_name = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "image".to_string());

        // 读取 + 编码 + 保存副本（阻塞操作移入 spawn_blocking，大图 base64 编码 CPU 密集）
        let src_for_task = src.clone();
        let load = tokio::task::spawn_blocking(move || -> Result<(String, String, String), String> {
            let bytes = match std::fs::read(&src_for_task) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(format!("图片文件不存在: {}", src_for_task.display()));
                }
                Err(e) => return Err(format!("读取图片失败: {}", e)),
            };
            let mime = crate::commands::config::detect_image_mime(&bytes).to_string();
            let b64 = STANDARD.encode(&bytes);
            // 副本保存到用户数据目录 images/（历史重载时按相对路径懒加载）
            let data_dir = crate::utils::path::get_user_data_dir();
            let images_dir = data_dir.join("images");
            crate::utils::path::ensure_dir(&images_dir)
                .map_err(|e| format!("创建图片目录失败: {}", e))?;
            let ext = src_for_task
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("png")
                .to_lowercase();
            let saved_name = format!("{}.{}", uuid::Uuid::new_v4(), ext);
            let saved_path = images_dir.join(&saved_name);
            std::fs::copy(&src_for_task, &saved_path).map_err(|e| format!("保存图片失败: {}", e))?;
            let rel_path = format!("images/{}", saved_name);
            Ok((mime, b64, rel_path))
        })
        .await
        .map_err(|e| format!("图片处理任务失败: {}", e));
        let (mime, b64, rel_path) = match load {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return ToolResult::standard_error("发送图片失败", Some(&e), None),
            Err(e) => return ToolResult::standard_error("图片处理任务失败", Some(&e), None),
        };

        // 路由：session_id 命中编程会话 → 编程页；否则 → 微信面板
        let is_coding = !ctx.session_id.is_empty() && CODING_AGENT.has_session(&ctx.session_id);
        if is_coding {
            let images = vec![CodingImage {
                media_type: mime,
                data: b64,
                name: Some(file_name.clone()),
            }];
            match CODING_AGENT.push_agent_image(&app_handle, &ctx.session_id, images, &caption) {
                Ok(()) => ToolResult::standard_success(
                    &format!("已向用户发送图片：{}", file_name),
                    Some(json!({ "sent": true, "image_path": rel_path })),
                ),
                Err(e) => ToolResult::standard_error("发送图片失败", Some(&e), None),
            }
        } else {
            // 角色路由：ctx.char_id > 活跃角色 > vivian
            let char_id = if ctx.char_id.is_empty() {
                app_handle
                    .try_state::<Arc<AppState>>()
                    .map(|s| s.active_character_id.read().clone())
                    .unwrap_or_else(|| "vivian".to_string())
            } else {
                ctx.char_id.clone()
            };
            send_image_to_wechat(&app_handle, &char_id, &mime, &b64, &rel_path, &caption);
            ToolResult::standard_success(
                &format!("已向用户发送图片：{}", file_name),
                Some(json!({ "sent": true, "image_path": rel_path })),
            )
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Media
    }

    fn always_load(&self) -> bool {
        false
    }

    fn should_defer(&self) -> bool {
        // 聊天侧按需经 tool_search 加载；编程侧由 CODING_TOOLS 白名单直接注入完整 schema
        true
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "send image picture photo screenshot show 发图片 看图 截图"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Sending a path that does not exist or is not an image file",
            "Sending the same image repeatedly in one conversation",
            "Using it to share a web link (use share_link instead)",
        ]
    }
}
