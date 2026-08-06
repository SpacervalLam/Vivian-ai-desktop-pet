//! 命令处理器。
//!
//! - 解析 `/` 开头的硬指令（`/clear`、`/reset`、`/remember`、`/forget`、`/list_memories`）
//! - 调用对应处理器
//! - 返回标准化的命令响应（text / motion / expression）

use serde::{Deserialize, Serialize};

use crate::memory::MemoryManager;

/// 命令解析结果。
#[derive(Debug, Clone, Default)]
pub struct ParsedCommand {
    /// 命令名（不含 `/`）
    pub cmd: String,
    /// 命令参数
    pub args: String,
}

impl ParsedCommand {
    pub fn is_command(&self) -> bool {
        !self.cmd.is_empty()
    }
}

/// 命令响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResponse {
    pub text: String,
    #[serde(default = "default_motion")]
    pub motion: String,
    #[serde(default)]
    pub expression: String,
}

fn default_motion() -> String {
    "idle".to_string()
}

impl CommandResponse {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            motion: default_motion(),
            expression: String::new(),
        }
    }

    pub fn with_motion(mut self, motion: impl Into<String>) -> Self {
        self.motion = motion.into();
        self
    }

    pub fn with_expression(mut self, expr: impl Into<String>) -> Self {
        self.expression = expr.into();
        self
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            motion: default_motion(),
            expression: "angry".to_string(),
        }
    }
}

/// 命令解析器。
///
/// 解析 `/` 开头的硬指令。
pub struct CommandParser;

impl CommandParser {
    pub fn new() -> Self {
        Self
    }

    /// 解析用户输入是否为命令。
    ///
    /// 返回 `(cmd, args)`，非 `//` 开头返回空 ParsedCommand。
    pub fn parse(&self, text: &str) -> ParsedCommand {
        let trimmed = text.trim();
        if !trimmed.starts_with('/') {
            return ParsedCommand::default();
        }

        let mut parts = trimmed.splitn(2, ' ');
        let cmd_full = parts.next().unwrap_or("");
        let args = parts.next().unwrap_or("").to_string();

        // 去掉前导 `/`
        let cmd = cmd_full.trim_start_matches('/').to_string();

        ParsedCommand { cmd, args }
    }

    /// 格式化帮助文本。
    pub fn format_help(&self) -> String {
        "可用命令：\n\
         /remember <内容> - 强制记住指定内容\n\
         /forget <内容> - 忘记包含指定内容的记忆\n\
         /list_memories - 列出最近的长期记忆\n\
         /clear - 清空对话历史\n"
            .to_string()
    }
}

impl Default for CommandParser {
    fn default() -> Self {
        Self::new()
    }
}

/// 命令处理器。
///
/// 依赖 MemoryManager 处理记忆相关命令。
pub struct CommandHandler {
    pub parser: CommandParser,
    pub memory: std::sync::Arc<MemoryManager>,
}

impl CommandHandler {
    pub fn new(memory: std::sync::Arc<MemoryManager>) -> Self {
        Self {
            parser: CommandParser::new(),
            memory,
        }
    }

    /// 解析用户输入是否为命令。
    pub fn parse(&self, text: &str) -> ParsedCommand {
        self.parser.parse(text)
    }

    /// 处理用户命令。
    pub async fn handle_command(&self, cmd: &str, args: &str) -> CommandResponse {
        tracing::debug!(cmd = cmd, args = args, "[CommandHandler] 处理命令");

        match cmd {
            "remember" => self.handle_remember(args).await,
            "forget" => self.handle_forget(args).await,
            "list_memories" | "list" => self.handle_list_memories().await,
            "clear" => self.handle_clear().await,
            "reset" => self.handle_reset().await,
            "help" => CommandResponse::new(self.parser.format_help()),
            _ => CommandResponse::new(format!(
                "未知命令: {}\n{}",
                cmd,
                self.parser.format_help()
            )),
        }
    }

    /// 处理 /remember 命令。
    async fn handle_remember(&self, content: &str) -> CommandResponse {
        if content.trim().is_empty() {
            return CommandResponse::new("请告诉我要记住什么内容");
        }

        match self
            .memory
            .add_memory_with_metadata(
                content,
                crate::memory::MemoryType::LongTerm,
                1.0,
                vec!["explicitly_remembered".to_string()],
                serde_json::json!({
                    "channel": "direct",
                    "speaker": "user",
                    "listener": self.memory.char_id(),
                    "perspective": "speaker",
                    "knowledge_source": "direct",
                }),
            )
            .await
        {
            Ok(_) => CommandResponse::new(format!("已记住: {}", content))
                .with_motion("Scene1")
                .with_expression("shy"),
            Err(e) => {
                tracing::error!(error = %e, "[CommandHandler] 记住命令执行失败");
                CommandResponse::error("记住失败了...")
            }
        }
    }

    /// 处理 /forget 命令。
    async fn handle_forget(&self, content: &str) -> CommandResponse {
        if content.trim().is_empty() {
            return CommandResponse::new("请告诉我要忘记什么内容");
        }

        // 简化：列出所有记忆，删除内容匹配的
        match self.memory.get_all_memories().await {
            Ok(memories) => {
                let mut removed = 0;
                for mem in memories {
                    if mem.content.contains(content) {
                        if let Err(e) = self.memory.delete_memory(&mem.id).await {
                            tracing::warn!(error = %e, "[CommandHandler] 删除记忆失败");
                        } else {
                            removed += 1;
                        }
                    }
                }
                if removed > 0 {
                    CommandResponse::new(format!("已忘记 {} 条包含 '{}' 的记忆", removed, content))
                        .with_expression("angry")
                } else {
                    CommandResponse::new(format!("没有找到包含 '{}' 的记忆", content))
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "[CommandHandler] 忘记命令执行失败");
                CommandResponse::error("忘记失败了...")
            }
        }
    }

    /// 处理 /list_memories 命令。
    async fn handle_list_memories(&self) -> CommandResponse {
        match self.memory.get_all_memories().await {
            Ok(memories) => {
                if memories.is_empty() {
                    return CommandResponse::new("没有找到记忆");
                }

                let mut memory_list = String::from("记忆列表：\n");
                let count = memories.len().min(5);
                for (i, memory) in memories.iter().take(count).enumerate() {
                    let preview: String = memory.content.chars().take(50).collect();
                    memory_list.push_str(&format!("{}. {}...\n", i + 1, preview));
                }

                if memories.len() > 5 {
                    memory_list.push_str(&format!("... 共 {} 条记忆", memories.len()));
                }

                CommandResponse::new(memory_list)
            }
            Err(e) => {
                tracing::error!(error = %e, "[CommandHandler] 列出记忆命令执行失败");
                CommandResponse::error("列出记忆失败了...")
            }
        }
    }

    /// 处理 /clear 命令。
    async fn handle_clear(&self) -> CommandResponse {
        match self.memory.clear_all_memories().await {
            Ok(_) => CommandResponse::new("已清空对话历史"),
            Err(e) => {
                tracing::error!(error = %e, "[CommandHandler] 清空命令执行失败");
                CommandResponse::error("清空失败了...")
            }
        }
    }

    /// 处理 /reset 命令（重置大脑状态）。
    async fn handle_reset(&self) -> CommandResponse {
        // 与 /clear 类似，但语义上"更彻底"
        match self.memory.clear_all_memories().await {
            Ok(_) => CommandResponse::new("已重置大脑状态").with_expression("normal"),
            Err(e) => {
                tracing::error!(error = %e, "[CommandHandler] 重置命令执行失败");
                CommandResponse::error("重置失败了...")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command() {
        let parser = CommandParser::new();

        let parsed = parser.parse("/remember buy milk");
        assert!(parsed.is_command());
        assert_eq!(parsed.cmd, "remember");
        assert_eq!(parsed.args, "buy milk");
    }

    #[test]
    fn test_parse_not_command() {
        let parser = CommandParser::new();
        let parsed = parser.parse("hello world");
        assert!(!parsed.is_command());
        assert_eq!(parsed.cmd, "");
    }

    #[test]
    fn test_parse_no_args() {
        let parser = CommandParser::new();
        let parsed = parser.parse("/clear");
        assert!(parsed.is_command());
        assert_eq!(parsed.cmd, "clear");
        assert_eq!(parsed.args, "");
    }

    #[test]
    fn test_command_response_builder() {
        let resp = CommandResponse::new("hello")
            .with_motion("idle")
            .with_expression("star_eyes");
        assert_eq!(resp.text, "hello");
        assert_eq!(resp.motion, "idle");
        assert_eq!(resp.expression, "star_eyes");
    }

    #[test]
    fn test_format_help() {
        let parser = CommandParser::new();
        let help = parser.format_help();
        assert!(help.contains("/remember"));
        assert!(help.contains("/clear"));
    }
}
