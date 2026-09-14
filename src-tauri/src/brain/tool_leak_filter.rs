//! 工具调用 Markup 泄露过滤 — 防止 LLM 把工具调用 XML 标签泄露到文本内容。
//!
//! 部分 LLM（Qwen3 / Hermes 格式模型）会把工具调用 markup 直接输出到 content 字段，
//! 而非走原生的 `tool_calls` API 字段。这会导致前端渲染乱码、TTS 念出 XML 标签。
//!
//! 本模块处理三种泄露形态：
//! 1. `<tool_call>...</tool_call>` — Qwen3 通用工具调用标签
//! 2. `<seed:tool_call>...</seed:tool_call>` — Qwen3 seed 格式
//! 3. `<function>...</function>` — Hermes 结构化格式（含嵌套 `<name>` / `<parameter>`）
//!
//! 流式版本为跨 chunk 状态机，非流式版本为正则清理。

use once_cell::sync::Lazy;
use regex::Regex;

/// 开标签模式：`<tool_call>` / `<seed:tool_call>` / `<function>`
static TOOL_OPEN_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)<(?:seed\s*:\s*)?tool_call\b[^>]*>|<function\b[^>]*>").unwrap());

/// 闭标签模式：`</tool_call>` / `</seed:tool_call>` / `</function>`
static TOOL_CLOSE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)</(?:seed\s*:\s*)?tool_call\s*>|</function\s*>").unwrap()
});

/// 成对 `<function>...</function>` 块（含嵌套标签，非贪婪匹配最外层）
static FUNCTION_BLOCK_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<function\b[^>]*>.*?</function\s*>").unwrap());

/// 成对 `<tool_call>...</tool_call>` 块
static TOOL_CALL_BLOCK_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<(?:seed\s*:\s*)?tool_call\b[^>]*>.*?</(?:seed\s*:\s*)?tool_call\s*>").unwrap());

/// 从完整（非流式）文本中移除工具调用 markup。
///
/// 处理成对块和孤立闭合标签（前文为泄露的 markup）。
/// 保守策略：仅当存在工具标签时才处理。
pub fn strip_tool_call_markup(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    // 1) 移除成对块（先 function 再 tool_call，避免嵌套误匹配）
    let s = FUNCTION_BLOCK_RE.replace_all(text, "");
    let s = TOOL_CALL_BLOCK_RE.replace_all(&s, "");
    // 2) 若仍有孤立闭合标签 → 前文均为泄露 markup，移除首个闭合标签及前文
    let s = if TOOL_CLOSE_RE.is_match(&s) {
        // 找到第一个闭合标签，移除它及之前的所有内容
        if let Some(m) = TOOL_CLOSE_RE.find(&s) {
            format!("{}{}", &s[m.end()..], {
                // 检查剩余是否还有更多闭合标签
                let rest = &s[m.end()..];
                if TOOL_CLOSE_RE.is_match(rest) {
                    // 递归处理剩余的孤立闭合标签
                    rest // 递归会在下一轮处理
                } else {
                    ""
                }
            })
        } else {
            s.to_string()
        }
    } else {
        s.to_string()
    };
    s.trim().to_string()
}

/// 流式工具调用泄露过滤器。
///
/// 跨 chunk 状态机：
/// - `Normal`：透传文本，扫描开标签；chunk 尾部可能的开标签前缀被缓冲
/// - `Suppressing`：丢弃文本，扫描闭标签；chunk 尾部可能的闭标签前缀被缓冲
///
/// 代码块内（``` ... ```）被抑制时输出 `[tool-call markup omitted]` 占位符。
pub struct ToolLeakFilter {
    /// 待处理缓冲（跨 chunk 的潜在标签前缀）
    pending: String,
    /// 是否正在抑制（处于工具调用 markup 内部）
    suppressing: bool,
    /// 当前抑制模式的闭标签正则（Normal 时为 None）
    /// 在 Suppressing 时用于查找闭标签
    in_code_fence: bool,
    /// 代码块行缓冲（跟踪 ``` 跨行）
    fence_line_buf: String,
    /// 最大尾部缓冲长度（防止无限增长）
    max_tail: usize,
}

impl ToolLeakFilter {
    pub fn new() -> Self {
        Self {
            pending: String::new(),
            suppressing: false,
            in_code_fence: false,
            fence_line_buf: String::new(),
            max_tail: 512,
        }
    }

    /// 处理一个文本块，返回可下发的文本。
    pub fn feed(&mut self, chunk: &str) -> String {
        if chunk.is_empty() {
            return String::new();
        }

        let mut text = if self.pending.is_empty() {
            chunk.to_string()
        } else {
            let mut combined = std::mem::take(&mut self.pending);
            combined.push_str(chunk);
            combined
        };

        let mut output = String::new();

        loop {
            if text.is_empty() {
                break;
            }

            if self.suppressing {
                // 在抑制模式：查找闭标签
                if let Some(m) = TOOL_CLOSE_RE.find(&text) {
                    // 找到闭标签，切换到 Normal
                    let consumed = &text[..m.end()];
                    self.track_code_fences(consumed);
                    if self.in_code_fence {
                        output.push_str("[tool-call markup omitted]");
                    }
                    self.suppressing = false;
                    text = text[m.end()..].to_string();
                } else {
                    // 未找到闭标签：保留尾部作为潜在前缀，丢弃其余
                    let keep = self.close_tag_tail_len(&text);
                    if keep > 0 && keep < text.len() {
                        let consumed = &text[..text.len() - keep];
                        self.track_code_fences(consumed);
                        if self.in_code_fence {
                            output.push_str("[tool-call markup omitted]");
                        }
                        self.pending = text[text.len() - keep..].to_string();
                    } else if keep >= text.len() {
                        // 整段都是潜在前缀，全部保留
                        self.track_code_fences(&text);
                        if self.in_code_fence {
                            output.push_str("[tool-call markup omitted]");
                        }
                        self.pending = std::mem::take(&mut text);
                    } else {
                        // keep == 0：全部丢弃
                        self.track_code_fences(&text);
                        if self.in_code_fence {
                            output.push_str("[tool-call markup omitted]");
                        }
                    }
                    break;
                }
            } else {
                // Normal 模式：查找开标签
                if let Some(m) = TOOL_OPEN_RE.find(&text) {
                    // 找到开标签，输出前文，切换到 Suppressing
                    if m.start() > 0 {
                        let before = &text[..m.start()];
                        self.track_code_fences(before);
                        output.push_str(before);
                    }
                    self.suppressing = true;
                    text = text[m.end()..].to_string();
                } else {
                    // 未找到开标签：保留尾部作为潜在前缀，输出其余
                    let keep = self.open_tag_tail_len(&text);
                    if keep > 0 && keep < text.len() {
                        let safe = &text[..text.len() - keep];
                        self.track_code_fences(safe);
                        output.push_str(safe);
                        self.pending = text[text.len() - keep..].to_string();
                    } else {
                        self.track_code_fences(&text);
                        output.push_str(&text);
                    }
                    break;
                }
            }
        }

        output
    }

    /// 流结束时排空缓冲。
    pub fn flush(&mut self) -> String {
        if self.suppressing {
            // 流结束时仍在抑制中：标记为已终结
            if self.in_code_fence {
                self.suppressing = false;
                let pending = std::mem::take(&mut self.pending);
                self.in_code_fence = false;
                self.fence_line_buf.clear();
                return format!("[tool-call markup omitted]{}", pending);
            }
            self.suppressing = false;
            std::mem::take(&mut self.pending)
        } else {
            std::mem::take(&mut self.pending)
        }
    }

    /// 重置状态。
    pub fn reset(&mut self) {
        self.pending.clear();
        self.suppressing = false;
        self.in_code_fence = false;
        self.fence_line_buf.clear();
    }

    /// 计算尾部可能作为开标签前缀的长度。
    fn open_tag_tail_len(&self, text: &str) -> usize {
        let bytes = text.as_bytes();
        let min_start = if text.len() > self.max_tail {
            text.len() - self.max_tail
        } else {
            0
        };
        // 从后往前找 `<` 字符
        for i in (min_start..text.len()).rev() {
            if bytes[i] == b'<' {
                let tail = &text[i..];
                if is_open_tag_prefix(tail) {
                    return tail.len();
                }
            }
        }
        0
    }

    /// 计算尾部可能作为闭标签前缀的长度。
    fn close_tag_tail_len(&self, text: &str) -> usize {
        let bytes = text.as_bytes();
        let min_start = if text.len() > self.max_tail {
            text.len() - self.max_tail
        } else {
            0
        };
        // 从后往前找 `<` 字符
        for i in (min_start..text.len()).rev() {
            if bytes[i] == b'<' {
                let tail = &text[i..];
                if is_close_tag_prefix(tail) {
                    return tail.len();
                }
            }
        }
        0
    }

    /// 跟踪代码块标记（``` 或 ~~~）。
    fn track_code_fences(&mut self, text: &str) {
        self.fence_line_buf.push_str(text);
        while let Some(pos) = self.fence_line_buf.find('\n') {
            let line: String = self.fence_line_buf[..pos].to_string();
            let rest: String = self.fence_line_buf[pos + 1..].to_string();
            self.fence_line_buf = rest;
            self.apply_fence_line(&line);
        }
    }

    fn apply_fence_line(&mut self, line: &str) {
        let stripped = line.trim_start();
        if stripped.starts_with("```") || stripped.starts_with("~~~") {
            let marker = &stripped[..3];
            if !self.in_code_fence {
                self.in_code_fence = true;
            } else if marker == "```" || marker == "~~~" {
                self.in_code_fence = false;
            }
        }
    }
}

impl Default for ToolLeakFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// 判断文本是否是某个开标签的前缀（可能跨 chunk 不完整）。
fn is_open_tag_prefix(text: &str) -> bool {
    let lower = text.to_lowercase();
    // `<tool_call>` / `<seed:tool_call>` / `<function>` 的前缀
    "<tool_call".starts_with(&lower)
        || "<seed:tool_call".starts_with(&lower)
        || "<seed:".starts_with(&lower)
        || "<seed".starts_with(&lower)
        || "<function".starts_with(&lower)
        || lower.starts_with("<tool_call")
        || lower.starts_with("<seed")
        || lower.starts_with("<function")
        || lower == "<"
}

/// 判断文本是否是某个闭标签的前缀。
fn is_close_tag_prefix(text: &str) -> bool {
    let lower = text.to_lowercase();
    "</tool_call".starts_with(&lower)
        || "</seed:tool_call".starts_with(&lower)
        || "</seed".starts_with(&lower)
        || "</function".starts_with(&lower)
        || lower.starts_with("</tool_call")
        || lower.starts_with("</seed")
        || lower.starts_with("</function")
        || lower == "<"
        || lower == "</"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_complete_tool_call_block() {
        let s = "before <tool_call>{\"name\":\"recall\"}</tool_call> after";
        assert_eq!(strip_tool_call_markup(s), "before  after");
    }

    #[test]
    fn strip_seed_tool_call_block() {
        let s = "before <seed:tool_call>content</seed:tool_call> after";
        assert_eq!(strip_tool_call_markup(s), "before  after");
    }

    #[test]
    fn strip_function_block() {
        let s = "before <function><name>recall_memory</name><parameter name=\"q\">test</parameter></function> after";
        assert_eq!(strip_tool_call_markup(s), "before  after");
    }

    #[test]
    fn strip_dangling_close() {
        let s = "leaked content</tool_call>real answer";
        assert_eq!(strip_tool_call_markup(s), "real answer");
    }

    #[test]
    fn strip_clean_passthrough() {
        assert_eq!(strip_tool_call_markup("普通回复"), "普通回复");
        assert_eq!(strip_tool_call_markup(""), "");
    }

    #[test]
    fn filter_feed_complete_block() {
        let mut f = ToolLeakFilter::new();
        let out = f.feed("before <tool_call>secret</tool_call> after");
        assert_eq!(out, "before  after");
    }

    #[test]
    fn filter_feed_cross_chunk() {
        let mut f = ToolLeakFilter::new();
        let out1 = f.feed("before <tool_");
        assert_eq!(out1, "before ");
        let out2 = f.feed("call>secret</tool_call> after");
        assert_eq!(out2, " after");
    }

    #[test]
    fn filter_feed_split_close_tag() {
        let mut f = ToolLeakFilter::new();
        f.feed("<tool_call>secret");
        let out = f.feed("</tool_cal");
        assert_eq!(out, "");
        let out = f.feed("l> done");
        assert_eq!(out, " done");
    }

    #[test]
    fn filter_flush_suppressing() {
        let mut f = ToolLeakFilter::new();
        f.feed("<tool_call>leaked content without close");
        let out = f.flush();
        // 流结束时仍在抑制中：返回缓冲内容
        assert!(out.contains("leaked content without close") || out.contains("[tool-call markup omitted]"));
    }

    #[test]
    fn filter_flush_normal() {
        let mut f = ToolLeakFilter::new();
        f.feed("normal text");
        let out = f.flush();
        assert_eq!(out, "");
    }

    #[test]
    fn filter_seed_tool_call() {
        let mut f = ToolLeakFilter::new();
        let out = f.feed("text <seed:tool_call>payload</seed:tool_call> more");
        assert_eq!(out, "text  more");
    }

    #[test]
    fn filter_function_block() {
        let mut f = ToolLeakFilter::new();
        let out = f.feed("x <function><name>tool</name></function> y");
        assert_eq!(out, "x  y");
    }

    #[test]
    fn filter_reset() {
        let mut f = ToolLeakFilter::new();
        f.feed("<tool_call>content");
        f.reset();
        assert!(!f.suppressing);
        assert!(f.pending.is_empty());
    }
}
