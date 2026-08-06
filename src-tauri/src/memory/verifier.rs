//! 检索后验证器：用小模型判断检索结果能否回答用户问题。
//!
//! 在检索完成后，用小模型快速判断"这些记忆能否回答用户的问题"，
//! 过滤掉无法回答的噪声记忆，避免污染 prompt 上下文。
//!
//! 设计权衡：
//! - verifier 是可选的——LLM 不可用时降级为"全部保留"。
//! - verifier 只做二分类（能/不能），不做细粒度评分，保证延迟可控。
//! - verifier 使用 `memory` 任务类型调用小模型，避免占用主对话通道。

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::VivianResult;
use crate::types::response::ChatMessage;

use super::types::MemoryItem;

/// verifier LLM 客户端抽象。
#[async_trait]
pub trait VerifierLlmClient: Send + Sync {
    async fn verify(&self, prompt: &str) -> VivianResult<String>;
}

/// 为 `ModelRouter` 实现 verifier 客户端。
#[async_trait]
impl VerifierLlmClient for crate::providers::ModelRouter {
    async fn verify(&self, prompt: &str) -> VivianResult<String> {
        let messages = vec![ChatMessage::user(prompt.to_string())];
        self.generate(crate::providers::base::LLMRequest::new("memory", messages))
            .await
    }
}

/// 检索后验证结果。
#[derive(Debug, Clone)]
pub struct VerificationResult {
    /// 通过验证的记忆索引列表（指向输入 memories 的位置）。
    pub verified_indices: Vec<usize>,
    /// 是否跳过了 LLM 验证（LLM 不可用时降级）。
    pub skipped: bool,
}

/// 对检索结果进行后验证。
///
/// - `memories`：检索到的候选记忆
/// - `query`：用户原始问题
/// - `llm`：verifier LLM 客户端（None 时跳过验证，全部保留）
///
/// 返回通过验证的记忆列表。LLM 不可用时降级为"全部保留"。
pub async fn verify_retrieval(
    memories: &[MemoryItem],
    query: &str,
    llm: Option<&Arc<dyn VerifierLlmClient>>,
) -> VerificationResult {
    if memories.is_empty() {
        return VerificationResult {
            verified_indices: Vec::new(),
            skipped: true,
        };
    }

    // 记忆数 ≤ 2 时无需验证（开销不值得）
    if memories.len() <= 2 {
        return VerificationResult {
            verified_indices: (0..memories.len()).collect(),
            skipped: true,
        };
    }

    let Some(llm) = llm else {
        return VerificationResult {
            verified_indices: (0..memories.len()).collect(),
            skipped: true,
        };
    };

    let prompt = build_verify_prompt(memories, query);

    match llm.verify(&prompt).await {
        Ok(resp) => {
            let indices = parse_verify_response(&resp, memories.len());
            if indices.is_empty() {
                // LLM 返回无法解析，降级为全部保留
                tracing::warn!("[Verifier] LLM 响应无法解析，降级为全部保留");
                VerificationResult {
                    verified_indices: (0..memories.len()).collect(),
                    skipped: true,
                }
            } else {
                VerificationResult {
                    verified_indices: indices,
                    skipped: false,
                }
            }
        }
        Err(e) => {
            tracing::warn!("[Verifier] LLM 验证失败，降级为全部保留: {}", e);
            VerificationResult {
                verified_indices: (0..memories.len()).collect(),
                skipped: true,
            }
        }
    }
}

/// 构造验证 prompt。
///
/// 让小模型判断每条记忆是否与回答用户问题相关，返回相关记忆的编号列表。
fn build_verify_prompt(memories: &[MemoryItem], query: &str) -> String {
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
    let (q_label, intro, footer, type_labels) = match lang_norm {
        "en" => (
            "User question: ",
            "Below are retrieved candidate memories. Determine whether each memory is relevant to answering the question above.",
            "Output only the numbers of relevant memories, separated by commas (e.g., 1,3,5). If no relevant memories, output none.",
            ("long-term", "short-term", "conversation"),
        ),
        "ja" => (
            "ユーザー質問：",
            "以下は検索された候補記憶です。各記憶が上記の質問に答えるために関連するか判断してください。",
            "関連する記憶の番号のみを出力、カンマ区切り（例：1,3,5）。関連する記憶がない場合は none を出力。",
            ("長期", "短期", "会話"),
        ),
        _ => (
            "用户问题：",
            "以下是检索到的候选记忆，请判断每条记忆是否与回答上述问题相关。",
            "只输出相关记忆的编号，用逗号分隔（如：1,3,5）。如果没有相关记忆，输出 none。",
            ("长期", "短期", "对话"),
        ),
    };
    let mut lines = Vec::new();
    lines.push(format!("{}{}", q_label, query));
    lines.push("".to_string());
    lines.push(intro.to_string());
    lines.push("".to_string());

    for (i, m) in memories.iter().enumerate() {
        // 截断到 400 字符（基于 char count，避免 UTF-8 边界切片 panic）
        let content = if m.content.chars().count() > 400 {
            let truncated: String = m.content.chars().take(400).collect();
            format!("{}...", truncated)
        } else {
            m.content.clone()
        };
        // 从 tags 提取类型
        let type_label = if m.tags.iter().any(|t| t == "long_term") {
            type_labels.0
        } else if m.tags.iter().any(|t| t == "short_term") {
            type_labels.1
        } else {
            type_labels.2
        };
        // 时间格式化为 MM-DD
        let time_str = format_timestamp_mmdd(m.timestamp);
        // 描述（如果有且不为空）
        let desc_suffix = match m.description.as_ref() {
            Some(d) if !d.trim().is_empty() => format!("（描述：{}）", d),
            _ => String::new(),
        };
        lines.push(format!(
            "[{}] ({}, {}, imp={:.2}) {}{}",
            i + 1,
            time_str,
            type_label,
            m.importance,
            content,
            desc_suffix
        ));
    }

    lines.push("".to_string());
    lines.push(footer.to_string());

    lines.join("\n")
}

/// 将 Unix 时间戳（秒）格式化为 MM-DD 字符串
fn format_timestamp_mmdd(timestamp: f64) -> String {
    let secs = timestamp as i64;
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|dt| dt.format("%m-%d").to_string())
        .unwrap_or_else(|| "??-??".to_string())
}

/// 解析验证响应，返回相关记忆的索引列表（0-based）。
fn parse_verify_response(resp: &str, total: usize) -> Vec<usize> {
    let trimmed = resp.trim().to_lowercase();

    if trimmed == "none" || trimmed.is_empty() {
        return Vec::new();
    }

    // 尝试提取逗号分隔的数字
    let mut indices = Vec::new();
    for part in trimmed.split(|c: char| c == ',' || c == ' ' || c == '\n') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // 去除可能的前缀符号（如 [1] 中的括号）
        let num_str: String = part.chars().filter(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = num_str.parse::<usize>() {
            if n >= 1 && n <= total {
                indices.push(n - 1); // 转为 0-based
            }
        }
    }

    indices.sort();
    indices.dedup();
    indices
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::Granularity;

    fn make_item(content: &str) -> MemoryItem {
        MemoryItem::new(content.to_string(), Granularity::Turn, 0.5)
    }

    #[test]
    fn test_parse_response() {
        let indices = parse_verify_response("1,3,5", 5);
        assert_eq!(indices, vec![0, 2, 4]);
    }

    #[test]
    fn test_parse_response_none() {
        let indices = parse_verify_response("none", 5);
        assert!(indices.is_empty());
    }

    #[test]
    fn test_parse_response_with_brackets() {
        let indices = parse_verify_response("[1], [3]", 5);
        assert_eq!(indices, vec![0, 2]);
    }

    #[test]
    fn test_parse_response_out_of_range() {
        let indices = parse_verify_response("1,9", 3);
        assert_eq!(indices, vec![0]); // 9 被过滤
    }

    #[tokio::test]
    async fn test_verify_empty_memories() {
        let result = verify_retrieval(&[], "test", None).await;
        assert!(result.skipped);
        assert!(result.verified_indices.is_empty());
    }

    #[tokio::test]
    async fn test_verify_no_llm() {
        let memories = vec![
            make_item("记忆1"),
            make_item("记忆2"),
            make_item("记忆3"),
        ];
        let result = verify_retrieval(&memories, "test", None).await;
        assert!(result.skipped);
        assert_eq!(result.verified_indices.len(), 3);
    }

    #[tokio::test]
    async fn test_verify_skip_small_set() {
        let memories = vec![make_item("记忆1"), make_item("记忆2")];
        let result = verify_retrieval(&memories, "test", None).await;
        assert!(result.skipped); // ≤ 2 条跳过验证
        assert_eq!(result.verified_indices.len(), 2);
    }
}
