//! 回复格式验证 Runnable
//!
//! 在 ResponseParsing 之后、ExpressionMotion 之前执行，对 AI 回复做验证：
//! - 空文本检测：should_respond=true 但 text 为空时记录 warning
//! - 长度上限截断：超过 MAX_RESPONSE_CHARS 时在句边界截断
//! - 基础清理：去除首尾空白、折叠连续空行
//! - 轻量幻觉检测（可选）：注入 router 后，当记忆上下文非空且回复较长时，
//!   用小模型检查回复是否包含与记忆矛盾的陈述。仅记录 warning，不修改回复。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::VivianResult;
use crate::pipeline::base::{Runnable, RunnableConfig};
use crate::pipeline::state::PipelineState;
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;

/// 回复文本最大字符数（超过后在句边界截断）
const MAX_RESPONSE_CHARS: usize = 500;

/// 截断时保留的最小字符数（避免截断到只剩几个字）
const MIN_KEEP_CHARS: usize = 50;

/// 幻觉检测触发的最小回复长度（字符数）
const HALLUCINATION_CHECK_MIN_LEN: usize = 30;

/// 幻觉检测超时时间
const HALLUCINATION_CHECK_TIMEOUT: Duration = Duration::from_secs(8);

pub struct ValidationRunnable {
    /// 可选的 LLM 路由器：注入后启用轻量幻觉检测
    router: Option<Arc<ModelRouter>>,
}

impl ValidationRunnable {
    pub fn new() -> Self {
        Self { router: None }
    }

    /// 注入 LLM 路由器，启用轻量幻觉检测
    pub fn with_router(router: Arc<ModelRouter>) -> Self {
        Self { router: Some(router) }
    }

    /// 在句边界截断文本
    ///
    /// 从句末标点（。！？.!?）处断开，尽量不截断到半句话。
    /// 如果找不到合适的句边界，在 MIN_KEEP_CHARS 之后的空格/换行处截断。
    fn truncate_at_boundary(text: &str, max_chars: usize) -> String {
        let char_count = text.chars().count();
        if char_count <= max_chars {
            return text.to_string();
        }

        // 收集字符索引，用于按字符数而非字节数定位
        let chars: Vec<char> = text.chars().collect();

        // 从句末标点向前搜索
        let sentence_ends: &[char] = &['。', '！', '？', '.', '!', '?', '\n'];
        let mut best_cut = None;
        for i in (MIN_KEEP_CHARS..max_chars).rev() {
            if sentence_ends.contains(&chars[i]) {
                best_cut = Some(i + 1); // 保留句末标点
                break;
            }
        }

        // 找不到句边界时，在空格处截断
        if best_cut.is_none() {
            for i in (MIN_KEEP_CHARS..max_chars).rev() {
                if chars[i] == ' ' || chars[i] == '\n' {
                    best_cut = Some(i);
                    break;
                }
            }
        }

        match best_cut {
            Some(cut) => {
                let truncated: String = chars[..cut].iter().collect();
                format!("{}…", truncated.trim_end())
            }
            None => {
                // 极端情况：直接硬截断
                let truncated: String = chars[..max_chars].iter().collect();
                format!("{}…", truncated.trim_end())
            }
        }
    }

    /// 折叠连续空行为单个换行，去除首尾空白
    fn normalize_whitespace(text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        let mut prev_blank = false;
        for line in text.lines() {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                if !prev_blank && !result.is_empty() {
                    result.push('\n');
                }
                prev_blank = true;
            } else {
                if prev_blank && !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(trimmed);
                result.push('\n');
                prev_blank = false;
            }
        }
        result.trim().to_string()
    }

    /// 轻量幻觉检测：用小模型检查回复是否包含与记忆矛盾的陈述。
    ///
    /// 返回 `Ok(Some(issue))` 表示检测到潜在幻觉，`Ok(None)` 表示通过。
    /// 超时或 LLM 失败时返回 `Err`，调用方降级为跳过。
    async fn check_faithfulness(
        router: &ModelRouter,
        memory_text: &str,
        reply_text: &str,
        dialogue_history: &str,
    ) -> Result<Option<String>, String> {
        let lang_norm =
            crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
        let (system, user) = match lang_norm {
            "en" => (
                "You are a hallucination detector. Check if the AI reply contains claims that contradict or fabricate information not supported by the memories or recent conversation. Output 'OK' if no issue, or 'ISSUE: <brief description>' if a potential hallucination is found. Be conservative — only flag clear contradictions or fabrications. Characters mentioned in the conversation history are real, not fabricated.",
                format!(
                    "Recent conversation:\n{}\n\nMemories:\n{}\n\nAI reply:\n{}\n\nCheck for hallucinations:",
                    dialogue_history,
                    memory_text,
                    reply_text
                ),
            ),
            "ja" => (
                "あなたは幻覚検出器です。AIの返信に記憶と矛盾する、または記憶や最近の会話で裏付けられない虚構の情報が含まれているか確認してください。問題なければ 'OK'、問題があれば 'ISSUE: <簡潔な説明>' と出力してください。明らかな矛盾や虚構のみをフラグしてください。会話履歴に登場するキャラクターは実在します。",
                format!(
                    "最近の会話：\n{}\n\n記憶：\n{}\n\nAIの返信：\n{}\n\n幻覚チェック：",
                    dialogue_history,
                    memory_text,
                    reply_text
                ),
            ),
            _ => (
                "你是幻觉检测器。检查 AI 回复中是否包含与记忆或最近对话矛盾、或编造了记忆和对话中不存在的信息。如果没有问题输出 'OK'，如果发现潜在幻觉输出 'ISSUE: <简要描述>'。保守判断——只标记明确的矛盾或编造。会话历史中出现过的角色是真实存在的，不算编造。",
                format!(
                    "最近对话：\n{}\n\n记忆：\n{}\n\nAI 回复：\n{}\n\n幻觉检查：",
                    dialogue_history,
                    memory_text,
                    reply_text
                ),
            ),
        };
        let messages = vec![
            ChatMessage::system(system),
            ChatMessage::user(&user),
        ];
        let fut = router.generate(LLMRequest::new("memory", messages));
        match tokio::time::timeout(HALLUCINATION_CHECK_TIMEOUT, fut).await {
            Ok(Ok(resp)) => {
                let resp_lower = resp.trim().to_lowercase();
                if resp_lower.starts_with("ok") {
                    Ok(None)
                } else if resp_lower.starts_with("issue") {
                    Ok(Some(resp.trim().to_string()))
                } else {
                    // 无法解析，视为通过
                    Ok(None)
                }
            }
            Ok(Err(e)) => Err(format!("LLM 调用失败: {}", e)),
            Err(_) => Err("超时".to_string()),
        }
    }
}

#[async_trait]
impl Runnable for ValidationRunnable {
    async fn ainvoke(
        &self,
        input: Value,
        _config: Option<RunnableConfig>,
    ) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);

        // 跳过不需要回复的场景
        if state.is_command || !state.should_respond || state.graceful_exit {
            return Ok(state.to_json());
        }

        // 1. 空文本检测
        if state.text.trim().is_empty() {
            tracing::warn!(
                "[Validation] AI 回复文本为空（should_respond=true, response_mode={}）",
                state.response_mode
            );
            // 不修改 state，让下游处理空文本
            return Ok(state.to_json());
        }

        // 2. 空白清理
        let cleaned = Self::normalize_whitespace(&state.text);
        if cleaned != state.text {
            tracing::debug!("[Validation] 回复空白清理：{} → {} 字符", state.text.chars().count(), cleaned.chars().count());
            state.text = cleaned;
        }

        // 3. 长度截断
        let char_count = state.text.chars().count();
        if char_count > MAX_RESPONSE_CHARS {
            let truncated = Self::truncate_at_boundary(&state.text, MAX_RESPONSE_CHARS);
            tracing::debug!(
                "[Validation] 回复超长截断：{} → {} 字符（上限 {}）",
                char_count,
                truncated.chars().count(),
                MAX_RESPONSE_CHARS
            );
            state.text = truncated;
        }

        // 4. 轻量幻觉检测（可选）
        // 仅当注入了 router、记忆上下文非空且回复足够长时触发。
        // 用小模型检查回复是否包含与记忆矛盾的陈述，仅记录 warning，不修改回复。
        // 跨角色对话跳过幻觉检测：闲聊场景风险低，且检测耗时（最高 8s）会挤占
        // talk_to_character 工具的超时预算，导致源角色误判目标角色"没回复"。
        if let Some(router) = &self.router {
            let is_cross_character = state.current_channel == "cross_character";
            let mem_text = state.memory_text.trim();
            let reply_text = state.text.trim();
            if !is_cross_character
                && !mem_text.is_empty()
                && reply_text.chars().count() >= HALLUCINATION_CHECK_MIN_LEN
            {
                // 从 state.messages 提取最近对话历史，让幻觉检测能感知上下文。
                // 避免把"会话中出现过的角色"误判为"编造的角色"。
                let dialogue_history: String = state
                    .messages
                    .iter()
                    .rev()
                    .take(8)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .map(|m| {
                        let role = match m.role.as_str() {
                            "user" => "User",
                            "assistant" => "AI",
                            other => other,
                        };
                        format!("{}: {}", role, m.content)
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                match Self::check_faithfulness(router, mem_text, reply_text, &dialogue_history).await {
                    Ok(Some(issue)) => {
                        tracing::warn!(
                            "[Validation] 幻觉检测发现问题: {}",
                            issue
                        );
                        state.metadata["hallucination_check"] = json!({
                            "status": "flagged",
                            "issue": issue,
                        });
                    }
                    Ok(None) => {
                        state.metadata["hallucination_check"] = json!({"status": "ok"});
                    }
                    Err(e) => {
                        tracing::debug!("[Validation] 幻觉检测跳过: {}", e);
                        state.metadata["hallucination_check"] = json!({"status": "skipped"});
                    }
                }
            }
        }

        Ok(state.to_json())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_text_unchanged() {
        let text = "你好呀～";
        assert_eq!(ValidationRunnable::truncate_at_boundary(text, 500), text);
    }

    #[test]
    fn truncate_long_text_at_sentence() {
        let text = "这是第一句话。这是第二句话。这是第三句话。这是第四句话。这是第五句话。";
        let result = ValidationRunnable::truncate_at_boundary(text, 20);
        assert!(result.chars().count() <= 22); // 20 + 句末标点 + …
        assert!(result.ends_with('…'));
    }

    #[test]
    fn normalize_whitespace_collapses_blanks() {
        let text = "你好\n\n\n\n世界\n\n\n你好";
        let result = ValidationRunnable::normalize_whitespace(text);
        assert_eq!(result, "你好\n\n世界\n\n你好");
    }

    #[test]
    fn normalize_whitespace_trims() {
        let text = "  \n  你好  \n  ";
        let result = ValidationRunnable::normalize_whitespace(text);
        assert_eq!(result, "你好");
    }
}
