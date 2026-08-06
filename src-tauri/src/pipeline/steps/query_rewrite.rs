//! 查询重写步骤 —— 检索前用 LLM 把口语化输入改写为更适合向量检索的查询。
//!
//! 借鉴 Spring AI 的 RewriteQueryTransformer。
//!
//! ## 触发条件
//! - 记忆数 ≥ `min_memories`（默认 10）：空库时无意义，跳过避免无谓 LLM 调用
//! - 输入长度 ≤ `max_input_len`（默认 50 字）：长输入已是充分表达，无需重写
//! - 命中缓存直接返回（LRU，避免重复调用）
//!
//! ## 失败降级
//! LLM 调用失败 / 超时 / 返回空：保留原输入，不阻塞主流程。
//!
//! ## 重要约束
//! 此步骤调用 LLM 处于读路径。为避免重蹈"读路径 9-60s 延迟"覆辙：
//! - 仅在记忆数 ≥ 阈值时触发
//! - 带 LRU 缓存
//! - 失败立即降级，不重试

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use lru::LruCache;
use parking_lot::Mutex;
use serde_json::Value;

use crate::error::VivianResult;
use crate::memory::MemoryManager;
use crate::pipeline::base::{Runnable, RunnableConfig};
use crate::pipeline::state::PipelineState;
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;

const REWRITE_TASK: &str = "memory";
const REWRITE_TIMEOUT: Duration = Duration::from_secs(8);

/// 查询重写步骤。
pub struct QueryRewriteStep {
    router: Arc<ModelRouter>,
    memory: Arc<MemoryManager>,
    cache: Mutex<LruCache<String, String>>,
    min_memories: usize,
    max_input_len: usize,
}

impl QueryRewriteStep {
    pub fn new(router: Arc<ModelRouter>, memory: Arc<MemoryManager>) -> Self {
        Self {
            router,
            memory,
            cache: Mutex::new(LruCache::new(std::num::NonZeroUsize::new(64).unwrap())),
            min_memories: 10,
            max_input_len: 50,
        }
    }

    pub fn with_min_memories(mut self, n: usize) -> Self {
        self.min_memories = n;
        self
    }

    pub fn with_max_input_len(mut self, n: usize) -> Self {
        self.max_input_len = n;
        self
    }

    /// 当前记忆条数（粗略估计，用于触发判断）。
    fn memory_count(&self) -> usize {
        self.memory.entry_count()
    }

    /// FLARE 式按需检索判断：根据用户输入特征判断是否需要检索记忆。
    ///
    /// 返回 `Some(reason)` 表示可跳过检索，`None` 表示需要检索。
    /// 启发式判断，不调用 LLM，避免增加读路径延迟。
    fn should_skip_retrieval(input: &str) -> Option<&'static str> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Some("empty_input");
        }

        let char_count = trimmed.chars().count();

        // 极短输入：匹配常见闲聊 / 问候 / 确认词，无需回忆过往
        if char_count <= 6 {
            const SKIP_PATTERNS: &[&str] = &[
                // 闲聊填充词
                "嗯", "嗯嗯", "哦", "啊", "哈", "嘿", "哈哈", "哈哈哈", "哈哈哈哈",
                "hi", "hey", "hello", "ok", "okay", "yep", "nope", "lol",
                // 问候语
                "你好", "您好", "早上好", "下午好", "晚上好", "晚安", "早安", "午安",
                "good morning", "good night", "good evening",
                // 确认 / 否认
                "好", "好的", "好吧", "行", "行吧", "可以", "对", "对对", "对对对",
                "是", "是的", "否", "不是", "不要", "不用", "不行", "没事", "没什么",
                "知道了", "明白了", "懂了", "收到", "fine", "算了", "拜拜", "再见", "bye",
            ];
            let lower = trimmed.to_lowercase();
            if SKIP_PATTERNS.iter().any(|p| lower == p.to_lowercase()) {
                return Some("smalltalk_or_greeting");
            }
        }

        // 纯标点 / 表情符号（无字母数字），无需检索
        if char_count <= 10 && !trimmed.chars().any(|c| c.is_alphanumeric()) {
            return Some("punctuation_or_emoji");
        }

        None
    }

    /// 调用 LLM 重写查询。失败返回 None。
    async fn rewrite(&self, query: &str) -> Option<String> {
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
        let prompt = match lang_norm {
            "en" => format!(
                "You are a query rewriter. Rewrite the following user input into a concise query better suited for vector retrieval:\n\
                 - Preserve core entities and intent; remove conversational filler\n\
                 - Output a single line only; no explanations, no quotation marks\n\
                 - If the input is already clear, return it as-is\n\
                 \n\
                 Input: {}",
                query
            ),
            "ja" => format!(
                "あなたはクエリリライターです。以下のユーザー入力をベクトル検索に適した簡潔なクエリに書き換えてください：\n\
                 - コアなエンティティと意図を保ち、口語的なフィラーを削除する\n\
                 - 出力は1行のみ、説明や引用符は不要\n\
                 - 入力がすでに明確な場合はそのまま返す\n\
                 \n\
                 入力：{}",
                query
            ),
            _ => format!(
                "你是查询重写器。把下面的用户输入改写为更适合向量检索的简洁查询：\n\
                 - 保留核心实体与意图，去除口语化填充词\n\
                 - 输出仅一行，不要解释、不要标点引号\n\
                 - 若输入已经清晰，原样返回\n\
                 \n\
                 输入：{}",
                query
            ),
        };
        let messages = vec![ChatMessage::user(prompt)];

        let fut = self.router.generate(LLMRequest::new(REWRITE_TASK, messages));
        match tokio::time::timeout(REWRITE_TIMEOUT, fut).await {
            Ok(Ok(text)) => {
                let cleaned = text
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .trim()
                    .to_string();
                if cleaned.is_empty() || cleaned == query {
                    None
                } else {
                    Some(cleaned)
                }
            }
            Ok(Err(e)) => {
                tracing::warn!("[QueryRewrite] LLM 调用失败，降级到原输入: {}", e);
                None
            }
            Err(_) => {
                tracing::warn!("[QueryRewrite] LLM 调用超时（{:?}），降级到原输入", REWRITE_TIMEOUT);
                None
            }
        }
    }
}

#[async_trait]
impl Runnable for QueryRewriteStep {
    async fn ainvoke(&self, input: Value, _config: Option<RunnableConfig>) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);

        if state.user_input.is_empty() || state.is_command {
            return Ok(state.to_json());
        }

        // FLARE 式按需检索：闲聊 / 问候 / 确认类输入无需回忆过往，
        // 标记跳过检索，下游 MemoryRetrievalStep 读取此标志直接返回。
        if let Some(reason) = Self::should_skip_retrieval(&state.user_input) {
            state.metadata["skip_memory_retrieval"] = Value::Bool(true);
            state.metadata["skip_retrieval_reason"] = Value::String(reason.to_string());
            return Ok(state.to_json());
        }

        // 记忆数过少时跳过：空库重写无意义
        if self.memory_count() < self.min_memories {
            return Ok(state.to_json());
        }

        let original = state.user_input.trim();
        // 长输入已是充分表达，跳过
        if original.chars().count() > self.max_input_len {
            state.metadata["query_rewrite_skipped"] = Value::String("input_too_long".into());
            return Ok(state.to_json());
        }

        // 缓存命中
        if let Some(cached) = self.cache.lock().get(original).cloned() {
            state.metadata["query_rewrite_cache_hit"] = Value::Bool(true);
            state.metadata["query_rewrite_original"] = Value::String(original.to_string());
            state.metadata["query_rewrite_result"] = Value::String(cached.clone());
            // 注意：不覆盖 user_input（保留原始表述给 LLM 生成使用），
            // 只写入 resolved_user_input 供检索使用
            if state.resolved_user_input.is_empty() {
                state.resolved_user_input = cached;
            }
            return Ok(state.to_json());
        }

        // 调用 LLM 重写
        match self.rewrite(original).await {
            Some(rewritten) => {
                self.cache.lock().put(original.to_string(), rewritten.clone());
                state.metadata["query_rewrite_cache_hit"] = Value::Bool(false);
                state.metadata["query_rewrite_original"] = Value::String(original.to_string());
                state.metadata["query_rewrite_result"] = Value::String(rewritten.clone());
                if state.resolved_user_input.is_empty() {
                    state.resolved_user_input = rewritten;
                }
            }
            None => {
                state.metadata["query_rewrite_skipped"] = Value::String("llm_failed".into());
            }
        }

        Ok(state.to_json())
    }
}

#[cfg(test)]
mod tests {
    // QueryRewriteStep 依赖 ModelRouter/MemoryManager，集成测试在 chat_chain 层验证。
    // 此处仅验证常量与基本逻辑。
    use super::*;

    #[test]
    fn rewrite_constants_are_reasonable() {
        assert_eq!(REWRITE_TASK, "memory");
        assert!(REWRITE_TIMEOUT >= Duration::from_secs(1));
    }
}
