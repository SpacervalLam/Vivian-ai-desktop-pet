//! 链接分享工具 - 当AI搜索到有趣的内容时，可以以微信链接卡片形式分享给用户
//!
//! 调用场景：
//! - 搜索到用户可能感兴趣的文章/视频/网页
//! - 需要分享具体链接让用户自行查看
//! - 分享后跟进一句简短的补充发言

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::messages::{MessageMeta, MessageSource};
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

// ============================================================================
// ShareLinkTool
// ============================================================================

pub struct ShareLinkTool;

impl ShareLinkTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ShareLinkTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ShareLinkTool {
    fn name(&self) -> &str {
        "share_link"
    }

    fn description(&self) -> &str {
        "Share a web link with the user as a rich card (similar to WeChat shared link preview), followed by a brief follow-up comment. Use this after web search when you find something interesting, useful, or relevant that the user might want to click through to read/watch. Only share links that are genuinely worth the user's time - don't share every search result. The card shows title, description/snippet, and source domain."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "以链接卡片形式分享网页给用户（类似微信分享链接预览），并跟进一句简短评论。在网络搜索后，当你发现有趣、有用或相关的内容用户可能想点开看时使用。只分享真正值得用户花时间的链接——不要把每条搜索结果都分享出来。卡片会显示标题、摘要/描述和来源域名。",
            "ja" => "ウェブリンクをリッチカードとしてユーザーに共有し（WeChatの共有リンクプレビューに類似）、簡単なコメントを添える。ウェブ検索後、ユーザーがクリックして見たくなるような面白い、役立つ、または関連するコンテンツが見つかった場合に使用する。価値のあるリンクのみ共有し、すべての検索結果を共有しないこと。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The full URL to share (must start with http:// or https://)"
                },
                "title": {
                    "type": "string",
                    "description": "Title of the page/content (keep concise, ideally under 30 characters)"
                },
                "description": {
                    "type": "string",
                    "description": "Brief description or snippet of the content (1-2 sentences, under 80 characters)"
                },
                "source": {
                    "type": "string",
                    "description": "Source name or domain (e.g. 'Bilibili', '知乎', 'GitHub', 'BBC News')"
                },
                "follow_up": {
                    "type": "string",
                    "description": "A brief natural follow-up comment after sharing the link (e.g. '这个UP主讲得挺清楚的', '感觉这篇分析很到位'). Keep it casual and conversational, 1-2 short sentences."
                }
            },
            "required": ["url", "title", "follow_up"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "要分享的完整URL（必须以 http:// 或 https:// 开头）"
                    },
                    "title": {
                        "type": "string",
                        "description": "页面/内容标题（保持简洁，建议30字以内）"
                    },
                    "description": {
                        "type": "string",
                        "description": "内容的简短描述或摘要（1-2句话，80字以内）"
                    },
                    "source": {
                        "type": "string",
                        "description": "来源名称或域名（如'Bilibili'、'知乎'、'GitHub'、'BBC News'）"
                    },
                    "follow_up": {
                        "type": "string",
                        "description": "分享链接后的简短自然跟进评论（如'这个UP主讲得挺清楚的'、'感觉这篇分析很到位'）。语气随意自然，1-2句短句。"
                    }
                },
                "required": ["url", "title", "follow_up"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "共有する完全なURL（http:// または https:// で始まる必要がある）"
                    },
                    "title": {
                        "type": "string",
                        "description": "ページ/コンテンツのタイトル（簡潔に、30文字以内推奨）"
                    },
                    "description": {
                        "type": "string",
                        "description": "コンテンツの簡単な説明またはスニペット（1-2文、80文字以内）"
                    },
                    "source": {
                        "type": "string",
                        "description": "ソース名またはドメイン（例：'Bilibili'、'知乎'、'GitHub'、'BBC News'）"
                    },
                    "follow_up": {
                        "type": "string",
                        "description": "リンク共有後の簡単な自然なフォローアップコメント。カジュアルで自然な口調、1-2文の短い文。"
                    }
                },
                "required": ["url", "title", "follow_up"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let url = input.get("url").and_then(|v| v.as_str()).unwrap_or("").trim();
        let title = input.get("title").and_then(|v| v.as_str()).unwrap_or("").trim();
        let follow_up = input.get("follow_up").and_then(|v| v.as_str()).unwrap_or("").trim();

        if url.is_empty() {
            return ValidationResult::failure("url 不能为空", 2);
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return ValidationResult::failure("url 必须以 http:// 或 https:// 开头", 2);
        }
        if title.is_empty() {
            return ValidationResult::failure("title 不能为空", 2);
        }
        if follow_up.is_empty() {
            return ValidationResult::failure("follow_up 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let description = args.get("description").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).unwrap_or_default();
        let source = args.get("source").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).unwrap_or_default();
        let follow_up = args.get("follow_up").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();

        let app_handle = match APP_HANDLE.read().clone() {
            Some(h) => h,
            None => {
                return ToolResult::standard_error("AppHandle 未初始化", Some("app handle not set"), None);
            }
        };

        let char_id = if ctx.char_id.is_empty() { "vivian".to_string() } else { ctx.char_id.clone() };

        share_link_to_wechat(
            &app_handle,
            &char_id,
            &url,
            &title,
            &description,
            &source,
            &follow_up,
        )
        .await;

        ToolResult::standard_success(
            &format!("已分享链接：{}", title),
            Some(json!({
                "shared": true,
                "title": title,
                "url": url,
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Web
    }

    fn always_load(&self) -> bool {
        false
    }

    fn should_defer(&self) -> bool {
        // 网络检索专用工具，默认延迟加载；仅在 Task/Default 等任务场景自动注入完整 schema
        true
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "share link send url card preview webpage article video"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Sharing search results that are not directly relevant to the conversation",
            "Sharing too many links at once (share at most 1-2 per response)",
            "Using it instead of answering the user's question directly",
        ]
    }
}

// ============================================================================
// 共享：通过微信聊天面板发送链接卡片
// ============================================================================

/// 通过微信聊天面板发送链接卡片。
///
/// 复用入口：
/// - `share_link` 工具调用（LLM 主动调用工具）
/// - 知识采集（Busy 状态）发现值得分享的链接时立即发送
/// - inner_monologue 兴趣搜索发现有趣内容时立即发送
///
/// 发送内容：
/// 1. 写入对话历史（character.brain.dialogue）— 链接卡片 + 跟进评论
/// 2. emit `chat:link_card`：前端实时插入链接卡片
/// 3. 若 side_chat 窗口未打开，emit `wechat:message_banner` 横幅提示
/// 4. emit `chat:assistant_message`：跟进评论
///
/// 写入对话历史并 emit 事件。在记忆图谱中作为 wechat 节点（信封图标）出现：
/// - metadata 携带 kind=web_link + link_card 字段 + channel=wechat
/// - 前端 classifyMemory 不再将其识别为 reading 节点（仅内化知识文档走 reading）
/// - 走 isWechat && isDirectDialogue 分支，点击弹窗展示结构化链接卡片
pub async fn share_link_to_wechat(
    app_handle: &AppHandle,
    char_id: &str,
    url: &str,
    title: &str,
    description: &str,
    source: &str,
    follow_up: &str,
) {
    let now = chrono::Local::now().to_rfc3339();

    // 写入对话历史（通过 character 的 brain.dialogue.add_message_with_metadata）
    if let Some(state) = app_handle.try_state::<Arc<AppState>>() {
        let characters = state.characters.read();
        if let Some(instance) = characters.get(char_id) {
            // 写入链接卡片记录
            let card_content = format!("{}\n{}", title, url);
            let card_msg = DialogChatMessage {
                role: "assistant".to_string(),
                content: card_content,
                timestamp: Some(chrono::Local::now()),
                tool_calls: None,
                tool_call_id: None,
                reasoning: None,
                images: None,
                meta: Some(MessageMeta {
                    source: MessageSource::Assistant,
                    is_memory_disabled: false,
                    mirror_kind: None,
                    channel: Some("wechat".to_string()),
                    kind: None,
                }),
            };
            let _ = instance.brain.dialogue.add_message_with_metadata(card_msg, json!({
                "kind": "web_link",
                "link_card": {
                    "url": url,
                    "title": title,
                    "description": description,
                    "source": source,
                }
            }));

            // 写入跟进评论记录
            if !follow_up.is_empty() {
                let follow_msg = DialogChatMessage {
                    role: "assistant".to_string(),
                    content: follow_up.to_string(),
                    timestamp: Some(chrono::Local::now()),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning: None,
                    images: None,
                    meta: Some(MessageMeta {
                        source: MessageSource::Assistant,
                        is_memory_disabled: false,
                        mirror_kind: None,
                        channel: Some("wechat".to_string()),
                        kind: None,
                    }),
                };
                let _ = instance.brain.dialogue.add_message_with_metadata(follow_msg, json!({}));
            }
        }
    }

    // 发送链接卡片事件（前端实时插入卡片）
    let _ = app_handle.emit(
        "chat:link_card",
        json!({
            "url": url,
            "title": title,
            "description": description,
            "source": source,
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
        let preview = if !follow_up.is_empty() {
            format!("{}: {}", title, follow_up)
        } else {
            title.to_string()
        };
        let _ = app_handle.emit(
            "wechat:message_banner",
            json!({
                "character_id": char_id,
                "preview": preview,
                "kind": "link_card",
                "timestamp": now,
            }),
        );
    }

    // 发送跟进评论（SideChatPanel 和 ChatWindow 实时显示）
    if !follow_up.is_empty() {
        let _ = app_handle.emit(
            "chat:assistant_message",
            json!({
                "content": follow_up,
                "timestamp": now,
                "character_id": char_id,
                "channel": "wechat",
            }),
        );
    }
}
