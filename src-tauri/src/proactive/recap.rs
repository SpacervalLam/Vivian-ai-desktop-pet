//! 用户回归摘要（welcome-back recap）
//!
//! 用户离开超过阈值后回归时，从统一事件账本提取离开窗口内该角色可见的
//! 事件，用轻量模型生成 1-3 句「刚才发生了什么」，写入 ObservationNote
//! 记忆并通知前端——角色在之后的对话中能自然提起离开期间做的事，
//! 而不是表现得像用户从未离开过。
//!
//! 挂点：proactive tick 的用户在场状态桥接（Away → Present 转换）。
//! `mark_user_present` 只在转换时返回 ReturnEvent，天然幂等不重入。

use std::sync::Arc;

use tauri::Emitter;

use crate::memory::types::MemoryType;
use crate::memory::MemoryManager;
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;

/// 触发阈值：离开不足此秒数不生成（短暂离开没有可总结的内容）
const MIN_AWAY_SECS: f64 = 600.0;
/// 参与摘要的事件上限（按重要性+时间倒序取）
const MAX_EVENTS: usize = 40;

/// 生成用户回归摘要
///
/// 返回 Some(recap) 表示已生成并写入记忆；离开时长不足或窗口内无事件时返回 None。
pub async fn generate_return_recap(
    char_id: &str,
    router: &Arc<ModelRouter>,
    memory: &Arc<MemoryManager>,
    away_secs: f64,
    app: &tauri::AppHandle,
) -> Option<String> {
    if away_secs < MIN_AWAY_SECS {
        return None;
    }

    // 离开窗口内该角色可见的事件（recent_events_visible_to 返回时间升序）
    let ledger = crate::memory::unified_event_ledger();
    let now = chrono::Local::now().timestamp() as f64;
    let since = now - away_secs;
    let events: Vec<_> = ledger
        .recent_events_visible_to(char_id, MAX_EVENTS * 2)
        .into_iter()
        .filter(|e| e.timestamp >= since)
        .take(MAX_EVENTS)
        .collect();
    if events.is_empty() {
        return None;
    }

    let event_lines: Vec<String> = events
        .iter()
        .map(|e| {
            let time_str = chrono::DateTime::from_timestamp(e.timestamp as i64, 0)
                .map(|dt| {
                    dt.with_timezone(&chrono::Local)
                        .format("%H:%M")
                        .to_string()
                })
                .unwrap_or_default();
            format!(
                "[{time_str}] {}: {} ({})",
                e.sender, e.content_preview, e.event_type
            )
        })
        .collect();

    let away_desc = if away_secs >= 3600.0 {
        format!("{:.1} 小时", away_secs / 3600.0)
    } else {
        format!("{:.0} 分钟", away_secs / 60.0)
    };

    let prompt = format!(
        "用户离开了约 {} 刚刚回来。以下是这段时间内的事件记录：\n{}\n\n\
         请以角色身份用 1-3 句话总结「你不在的这段时间里我做了什么 / 发生了什么」。\
         要求：先说最重要的一件事，再说接下来的打算（如有）；跳过流水账和状态报告；\
         只输出总结文本本身。",
        away_desc,
        event_lines.join("\n")
    );

    let req = LLMRequest::new("memory", vec![ChatMessage::user(prompt)]);
    let recap = match router.generate(req).await {
        Ok(text) => text.trim().to_string(),
        Err(e) => {
            tracing::debug!("[ReturnRecap:{}] 生成失败（静默跳过）: {e}", char_id);
            return None;
        }
    };
    if recap.is_empty() {
        return None;
    }

    // 写入旁观记忆：不参与对话摘要压缩，但可被向量检索命中，
    // 角色下次对话时能"想起"离开期间发生的事
    let metadata = serde_json::json!({
        "perspective": "observer",
        "memory_type": "observation_note",
        "kind": "return_recap",
        "away_secs": away_secs,
    });
    if let Err(e) = memory
        .add_memory_with_metadata(
            &recap,
            MemoryType::ObservationNote,
            0.4,
            vec![
                "observation_note".to_string(),
                "return_recap".to_string(),
            ],
            metadata,
        )
        .await
    {
        tracing::warn!("[ReturnRecap:{}] 记忆写入失败: {e}", char_id);
    }

    tracing::info!(
        "[ReturnRecap:{}] 用户离开 {} 回归，已生成摘要（{} 字）",
        char_id,
        away_desc,
        recap.chars().count()
    );
    let _ = app.emit(
        "user:return_recap",
        serde_json::json!({
            "character_id": char_id,
            "away_secs": away_secs,
            "recap": recap,
        }),
    );
    Some(recap)
}
