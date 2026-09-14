//! 思考链泄露过滤 — 防止 CoT 被 TTS 当作台词念出。
//!
//! 部分 Qwen3.5/3.6/3.7 混合推理模型不把推理写入独立的 `reasoning_content` 字段，
//! 而是把整个 CoT 倾倒进 `content` 字段，仅以一个孤立的 `</think>` 收尾。流式路径
//! 下若不过滤，CoT 会被逐 token 喂进 TTS 与 UI，把内心独白念出来。

use once_cell::sync::Lazy;
use regex::Regex;

/// 匹配任意 think 闭合标签：`</think>` / `</thinking>`（允许尾随空白，大小写不敏感）
static THINK_ANY_CLOSE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)</think(?:ing)?\s*>").unwrap());

/// 匹配成对 think 标签块：`<think>...</think>` / `<thinking>...</thinking>`
static THINK_PAIRED_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<think(?:ing)?\s*>.*?</think(?:ing)?\s*>").unwrap());

/// 匹配文本开头到第一个闭合标签（处理孤立闭合标签：前文均为泄露的 CoT）
static THINK_DANGLING_CLOSE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)^.*?</think(?:ing)?\s*>").unwrap());

/// 判断模型是否把 CoT 泄露到 content 字段。
///
/// 仅 Qwen3.5/3.6/3.7 混合模型有此行为；`qwen3-vl-*` 视觉模型把推理路由到
/// 独立的 `reasoning_content` 字段，保持干净，故排除。
pub fn leaks_thinking_in_content(model: &str) -> bool {
    let m = model.to_lowercase();
    if m.contains("vl") {
        return false;
    }
    m.contains("qwen3.5") || m.contains("qwen3.6") || m.contains("qwen3.7")
}

/// 从完整（非流式）回复中移除泄露的 CoT。
///
/// 处理两种形态：
/// 1. 成对的 `<think>...</think>` 块（任意数量）
/// 2. Qwen3.5/3.6 泄露：content 中只有孤立的 `</think>`（无开标签），前文为 CoT
///
/// 保守策略：仅当存在 think 标签时才处理，干净回复原样返回。
pub fn strip_thinking_segments(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    // 1) 先移除成对块
    let s = THINK_PAIRED_RE.replace_all(text, "");
    // 2) 若仍有闭合标签 → 前文均为 CoT，移除首个孤立闭合标签及前文
    let s = if THINK_ANY_CLOSE_RE.is_match(&s) {
        THINK_DANGLING_CLOSE_RE.replace(&s, "").to_string()
    } else {
        s.to_string()
    };
    s.trim().to_string()
}

/// 流式思考链剥离器。
///
/// 在看到第一个 `</think>` 之前 hold 所有 content（丢弃 CoT），之后切换到透传模式，
/// 逐块转发真实回答。若 leak-prone 模型本轮未思考（无闭合标签），`flush` 返回 hold
/// 的全部缓冲，保证内容不丢失。跨 chunk 的拆分标签安全（缓冲累积直到匹配）。
///
/// **仅对 leak-prone 模型启用**：干净模型的闭合标签永不到达，hold 直到 flush 会
/// 破坏流式体验。
pub struct ThinkingStreamStripper {
    buf: String,
    passthrough: bool,
}

impl ThinkingStreamStripper {
    pub fn new() -> Self {
        Self {
            buf: String::new(),
            passthrough: false,
        }
    }

    /// 返回本次可下发的文本切片（缓冲中返回空串）。
    pub fn feed(&mut self, text: &str) -> String {
        if self.passthrough {
            return text.to_string();
        }
        if text.is_empty() {
            return String::new();
        }
        self.buf.push_str(text);
        if let Some(m) = THINK_ANY_CLOSE_RE.find(&self.buf) {
            let tail = self.buf[m.end()..].to_string();
            self.buf.clear();
            self.passthrough = true;
            tail
        } else {
            String::new()
        }
    }

    /// 流结束时排空 hold 的缓冲（无闭合标签到达时，内容原样返回）。
    pub fn flush(&mut self) -> String {
        if self.passthrough {
            return String::new();
        }
        let residual = std::mem::take(&mut self.buf);
        self.passthrough = true;
        residual
    }

    /// 重置状态（工具调用轮次边界使用：下一段是全新语义单元）。
    pub fn reset(&mut self) {
        self.buf.clear();
        self.passthrough = false;
    }
}

impl Default for ThinkingStreamStripper {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaks_detection() {
        assert!(leaks_thinking_in_content("qwen3.5-72b"));
        assert!(leaks_thinking_in_content("Qwen3.6-Instruct"));
        assert!(leaks_thinking_in_content("qwen3.7-hybrid"));
        assert!(!leaks_thinking_in_content("qwen3-vl-72b"));
        assert!(!leaks_thinking_in_content("qwen2.5-72b"));
        assert!(!leaks_thinking_in_content("deepseek-r1"));
        assert!(!leaks_thinking_in_content("gpt-4o"));
        assert!(!leaks_thinking_in_content(""));
    }

    #[test]
    fn strip_paired_block() {
        let s = "<think>let me think</think>hello world";
        assert_eq!(strip_thinking_segments(s), "hello world");
    }

    #[test]
    fn strip_dangling_close() {
        let s = "这是CoT内容应该被丢弃</think>真实回答";
        assert_eq!(strip_thinking_segments(s), "真实回答");
    }

    #[test]
    fn strip_clean_passthrough() {
        assert_eq!(strip_thinking_segments("普通回复"), "普通回复");
        assert_eq!(strip_thinking_segments(""), "");
    }

    #[test]
    fn strip_multiple_paired() {
        let s = "<think>a</think>x<think>b</think>y";
        assert_eq!(strip_thinking_segments(s), "xy");
    }

    #[test]
    fn stripper_feed_then_passthrough() {
        let mut s = ThinkingStreamStripper::new();
        // CoT 分块到达
        assert_eq!(s.feed("let me "), "");
        assert_eq!(s.feed("think"), "");
        assert_eq!(s.feed("</think>"), "");
        // 切换到透传
        assert_eq!(s.feed("hello "), "hello ");
        assert_eq!(s.feed("world"), "world");
    }

    #[test]
    fn stripper_split_close_tag() {
        let mut s = ThinkingStreamStripper::new();
        assert_eq!(s.feed("cot</thi"), "");
        assert_eq!(s.feed("nk>answer"), "answer");
        assert_eq!(s.feed(" more"), " more");
    }

    #[test]
    fn stripper_flush_no_close() {
        let mut s = ThinkingStreamStripper::new();
        assert_eq!(s.feed("no cot here"), "");
        assert_eq!(s.flush(), "no cot here");
    }

    #[test]
    fn stripper_flush_after_passthrough() {
        let mut s = ThinkingStreamStripper::new();
        s.feed("</think>real");
        assert_eq!(s.flush(), "");
    }

    #[test]
    fn stripper_reset() {
        let mut s = ThinkingStreamStripper::new();
        s.feed("cot</think>real");
        s.reset();
        assert_eq!(s.passthrough, false);
        assert!(s.buf.is_empty());
    }
}
