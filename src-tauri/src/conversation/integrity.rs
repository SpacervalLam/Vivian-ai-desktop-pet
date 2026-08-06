//! 对话完整性修复（Conversation Integrity Repair）
//!
//! 当对话被中断（Ctrl+C、崩溃、tokio abort）时，assistant 消息中的 `tool_calls`
//! 可能缺少对应的 `tool` role 结果消息。这会导致 OpenAI 兼容 API 返回 400 错误
//! （tool_call 和 tool_result 必须配对）。
//!
//! 本模块扫描消息列表，检测孤立的 tool_call，并为每个孤立的 tool_call_id
//! 插入一条合成的 tool_result 消息。
//!
//! 特性：
//! - **幂等**：对已完整的对话运行不产生任何修复
//! - **安全**：合成消息内容明确标注为中断，不会被误认为真实结果
//! - **位置正确**：合成结果插入在对应 assistant 消息之后

use std::collections::HashSet;

use crate::types::response::ChatMessage;

/// 对话完整性修复器
pub struct ConversationIntegrity;

/// 修复动作描述
#[derive(Debug, Clone)]
pub enum RepairAction {
    /// 为孤立的 tool_call_id 插入了合成结果
    InsertedSyntheticResult {
        tool_call_id: String,
        tool_name: String,
    },
}

impl ConversationIntegrity {
    /// 扫描消息列表，修复所有孤立的 tool_call（无对应 tool_result）
    ///
    /// 返回修复动作列表（用于日志记录）。
    ///
    /// # 算法
    /// 1. 前向扫描收集所有 assistant tool_call 的 (index, tool_call_id, tool_name)
    /// 2. 前向扫描收集所有 tool_result 消息的 tool_call_id
    /// 3. 差集 = 孤立的 tool_call
    /// 4. 按 index 倒序插入合成结果（避免插入影响后续索引）
    pub fn repair(messages: &mut Vec<ChatMessage>) -> Vec<RepairAction> {
        if messages.is_empty() {
            return Vec::new();
        }

        // 步骤 1：收集所有 tool_call（来自 assistant 消息）
        // (message_index, tool_call_id, tool_name)
        let mut pending_calls: Vec<(usize, String, String)> = Vec::new();

        for (idx, msg) in messages.iter().enumerate() {
            if let Some(tool_calls) = &msg.tool_calls {
                for tc in tool_calls {
                    pending_calls.push((idx, tc.id.clone(), tc.name.clone()));
                }
            }
        }

        if pending_calls.is_empty() {
            return Vec::new();
        }

        // 步骤 2：收集所有已有 tool_result 的 tool_call_id
        let resolved_ids: HashSet<String> = messages
            .iter()
            .filter_map(|m| m.tool_call_id.clone())
            .collect();

        // 步骤 3：找到孤立的 tool_call
        let orphaned: Vec<(usize, String, String)> = pending_calls
            .into_iter()
            .filter(|(_, id, _)| !resolved_ids.contains(id))
            .collect();

        if orphaned.is_empty() {
            return Vec::new();
        }

        // 步骤 4：按 index 倒序插入合成结果
        // 倒序确保插入不影响后续索引计算
        let mut repairs = Vec::new();
        let mut sorted_orphans = orphaned;
        sorted_orphans.sort_by(|a, b| b.0.cmp(&a.0)); // 降序

        for (assistant_idx, tool_call_id, tool_name) in sorted_orphans {
            let synthetic = ChatMessage::tool_result(
                format!(
                    "[工具执行被中断，未完成。工具: {}，调用ID: {}]",
                    tool_name, tool_call_id
                ),
                tool_call_id.clone(),
            );

            // 插入位置：assistant 消息之后
            // 如果有多个 tool_call 对应同一个 assistant 消息，
            // 它们会被按倒序插入，最终保持原始顺序
            let insert_pos = (assistant_idx + 1).min(messages.len());
            messages.insert(insert_pos, synthetic);

            repairs.push(RepairAction::InsertedSyntheticResult {
                tool_call_id,
                tool_name,
            });
        }

        repairs.reverse(); // 返回正序（按消息位置从前到后）
        repairs
    }

    /// 快速检查：对话是否有孤立的 tool_call（不修改）
    pub fn has_orphaned_tool_calls(messages: &[ChatMessage]) -> bool {
        let mut call_ids: HashSet<String> = HashSet::new();
        let mut result_ids: HashSet<String> = HashSet::new();

        for msg in messages {
            if let Some(tool_calls) = &msg.tool_calls {
                for tc in tool_calls {
                    call_ids.insert(tc.id.clone());
                }
            }
            if let Some(id) = &msg.tool_call_id {
                result_ids.insert(id.clone());
            }
        }

        !call_ids.is_subset(&result_ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::response::MessageToolCall;

    fn system(content: &str) -> ChatMessage {
        ChatMessage::system(content)
    }

    fn user(content: &str) -> ChatMessage {
        ChatMessage::user(content)
    }

    fn assistant_with_tools(content: &str, calls: Vec<(&str, &str)>) -> ChatMessage {
        let tool_calls = calls
            .into_iter()
            .map(|(id, name)| MessageToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments: serde_json::json!({}),
            })
            .collect();
        ChatMessage::assistant_with_tool_calls(content, tool_calls)
    }

    fn tool_result(id: &str, _name: &str, content: &str) -> ChatMessage {
        ChatMessage::tool_result(content, id)
    }

    #[test]
    fn no_repair_needed_complete_pairs() {
        let mut msgs = vec![
            system("sys"),
            user("hi"),
            assistant_with_tools("calling", vec![("c1", "read_file")]),
            tool_result("c1", "read_file", "result"),
        ];
        let repairs = ConversationIntegrity::repair(&mut msgs);
        assert!(repairs.is_empty());
        assert_eq!(msgs.len(), 4);
    }

    #[test]
    fn repair_single_orphaned_call() {
        let mut msgs = vec![
            system("sys"),
            user("hi"),
            assistant_with_tools("calling", vec![("c1", "read_file")]),
            // 缺少 tool_result for c1
        ];
        let repairs = ConversationIntegrity::repair(&mut msgs);
        assert_eq!(repairs.len(), 1);
        assert_eq!(msgs.len(), 4); // 插入了合成结果
        assert_eq!(msgs[3].role, "tool");
        assert_eq!(msgs[3].tool_call_id.as_deref(), Some("c1"));
        assert!(msgs[3].content.contains("中断"));
    }

    #[test]
    fn repair_multiple_orphaned_calls() {
        let mut msgs = vec![
            system("sys"),
            assistant_with_tools("calling two", vec![("c1", "read"), ("c2", "write")]),
            // 两个都缺少结果
        ];
        let repairs = ConversationIntegrity::repair(&mut msgs);
        assert_eq!(repairs.len(), 2);
        assert_eq!(msgs.len(), 4);
        // 两个合成结果都在 assistant 之后
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[2].role, "tool");
        assert_eq!(msgs[3].role, "tool");
    }

    #[test]
    fn repair_partial_orphan() {
        let mut msgs = vec![
            system("sys"),
            assistant_with_tools("calling two", vec![("c1", "read"), ("c2", "write")]),
            tool_result("c1", "read", "ok"),
            // c2 缺少结果
        ];
        let repairs = ConversationIntegrity::repair(&mut msgs);
        assert_eq!(repairs.len(), 1);
        assert_eq!(msgs.len(), 4);
        // 合成结果在已有 tool_result 之后
        assert_eq!(msgs[3].tool_call_id.as_deref(), Some("c2"));
    }

    #[test]
    fn idempotent_repair() {
        let mut msgs = vec![
            system("sys"),
            assistant_with_tools("calling", vec![("c1", "read")]),
        ];
        // 第一次修复
        ConversationIntegrity::repair(&mut msgs);
        let len_after_first = msgs.len();
        // 第二次修复不应产生额外消息
        let repairs = ConversationIntegrity::repair(&mut msgs);
        assert!(repairs.is_empty());
        assert_eq!(msgs.len(), len_after_first);
    }

    #[test]
    fn empty_messages() {
        let mut msgs: Vec<ChatMessage> = vec![];
        let repairs = ConversationIntegrity::repair(&mut msgs);
        assert!(repairs.is_empty());
    }

    #[test]
    fn no_tool_calls_no_repair() {
        let mut msgs = vec![system("sys"), user("hi"), user("bye")];
        let repairs = ConversationIntegrity::repair(&mut msgs);
        assert!(repairs.is_empty());
    }

    #[test]
    fn has_orphaned_tool_calls_check() {
        let msgs = vec![
            assistant_with_tools("calling", vec![("c1", "read")]),
            // 没有 tool_result
        ];
        assert!(ConversationIntegrity::has_orphaned_tool_calls(&msgs));

        let msgs_complete = vec![
            assistant_with_tools("calling", vec![("c1", "read")]),
            tool_result("c1", "read", "ok"),
        ];
        assert!(!ConversationIntegrity::has_orphaned_tool_calls(&msgs_complete));
    }
}
