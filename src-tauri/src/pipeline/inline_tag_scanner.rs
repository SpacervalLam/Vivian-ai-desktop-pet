//! 流式内联标签扫描器
//!
//! 检测 LLM 输出中的轻量内联标签，实时剥离并转换为前端事件：
//! - `<e name="happy" dur="3000"/>` — 表情 + 可选持续时间
//! - `<m name="wave"/>` — 动作
//! - `<s name="sticker_id"/>` — 贴纸
//!
//! 扫描器逐 chunk 接收文本，返回剥离标签后的干净文本，
//! 同时通过回调通知调用方检测到的标签（用于 emit Tauri 事件）。
//!
//! 跨 chunk 的部分标签通过 deferred buffer 处理：
//! 当 chunk 末尾出现未闭合的 `<` 时，将该段文本暂存，
//! 待下一个 chunk 到达后合并处理，避免误发不完整标签。

/// 检测到的内联标签
#[derive(Debug, Clone)]
pub enum InlineTag {
    Expression { name: String, duration_ms: Option<u64> },
    Motion { name: String },
    Sticker { name: String },
}

/// 标签检测回调（用于 emit 前端事件）
pub type TagCallback = Box<dyn Fn(InlineTag) + Send + Sync>;

/// 流式内联标签扫描器
///
/// 逐 chunk 喂入 LLM 文本增量，返回剥离标签后的干净文本。
/// 检测到的标签通过构造时注入的回调通知调用方。
pub struct InlineTagScanner {
    on_tag: TagCallback,
    /// 跨 chunk 暂存的部分标签文本
    deferred: String,
}

impl InlineTagScanner {
    pub fn new(on_tag: TagCallback) -> Self {
        Self {
            on_tag,
            deferred: String::new(),
        }
    }

    /// 喂入一个文本 chunk，返回剥离标签后的干净文本
    pub fn feed(&mut self, chunk: &str) -> String {
        // 合并上次暂存的部分标签文本
        let mut text = String::with_capacity(self.deferred.len() + chunk.len());
        text.push_str(&self.deferred);
        text.push_str(chunk);
        self.deferred.clear();

        let mut clean = String::with_capacity(text.len());
        let mut pos = 0;
        let bytes = text.as_bytes();

        while pos < bytes.len() {
            // 寻找下一个 `<`
            match text[pos..].find('<') {
                None => {
                    // 没有更多 `<`，剩余全部是干净文本
                    clean.push_str(&text[pos..]);
                    break;
                }
                Some(offset) => {
                    // 先把 `<` 之前的文本加入干净输出
                    clean.push_str(&text[pos..pos + offset]);
                    let tag_start = pos + offset;

                    if let Some(tag_close) = Self::find_tag_close(&text, tag_start) {
                        // 找到完整的自闭合标签 `/>`
                        let tag_str = &text[tag_start..=tag_close + 1]; // 包含 `/>`
                        if let Some(tag) = Self::parse_tag(tag_str) {
                            (self.on_tag)(tag);
                        } else {
                            // 解析失败，原样保留
                            clean.push_str(tag_str);
                        }
                        pos = tag_close + 2; // 跳过 `/>`
                    } else {
                        // 没找到 `/>`：可能是部分标签跨越 chunk 边界
                        // 检查 `<` 后是否紧跟有效标签字符（e/m/s）
                        let after_lt = tag_start + 1;
                        if after_lt < bytes.len()
                            && matches!(bytes[after_lt], b'e' | b'm' | b's')
                        {
                            // 看起来像标签开头，暂存到 deferred
                            self.deferred.push_str(&text[tag_start..]);
                            break;
                        } else {
                            // 不是有效标签开头（如 `<3`、`< `），当普通文本
                            clean.push('<');
                            pos = tag_start + 1;
                        }
                    }
                }
            }
        }

        clean
    }

    /// 处理结束后刷新 deferred 缓冲（关闭标签未到达的情况）
    ///
    /// 调用方应在流式结束后调用一次，将残留的部分标签文本作为普通文本输出。
    pub fn flush(&mut self) -> String {
        std::mem::take(&mut self.deferred)
    }

    /// 从 `start` 位置查找自闭合标签的关闭位置
    ///
    /// 返回 `/>` 中 `/` 的位置索引（相对于整个 text）。
    /// 为避免误匹配，限制搜索范围为 200 字符以内。
    fn find_tag_close(text: &str, start: usize) -> Option<usize> {
        let search_range = text[start..].char_indices().take(200);
        for (i, _) in search_range {
            let abs = start + i;
            if abs + 1 < text.len() && &text[abs..abs + 2] == "/>" {
                return Some(abs);
            }
        }
        None
    }

    /// 解析自闭合标签字符串为 `InlineTag`
    ///
    /// 支持格式：
    /// - `<e name="happy" dur="3000"/>`
    /// - `<m name="wave"/>`
    /// - `<s name="heart"/>`
    fn parse_tag(tag_str: &str) -> Option<InlineTag> {
        let s = tag_str.trim();
        // 基本格式校验：<X ... />
        if !s.starts_with('<') || !s.ends_with("/>") || s.len() < 5 {
            return None;
        }

        // 提取标签名（`<` 后第一个字符）
        let tag_type = s.as_bytes().get(1)?;

        // 提取属性区（标签名之后、`/>` 之前）
        let attrs = &s[2..s.len() - 2].trim();

        let name = Self::extract_attr(attrs, "name")?;
        if name.is_empty() {
            return None;
        }

        match tag_type {
            b'e' => {
                let duration = Self::extract_attr(attrs, "dur")
                    .and_then(|d| d.parse::<u64>().ok());
                Some(InlineTag::Expression {
                    name,
                    duration_ms: duration,
                })
            }
            b'm' => Some(InlineTag::Motion { name }),
            b's' => Some(InlineTag::Sticker { name }),
            _ => None,
        }
    }

    /// 从属性字符串中提取指定属性的值
    ///
    /// 支持格式：`name="value"` 或 `name='value'`
    fn extract_attr(attrs: &str, key: &str) -> Option<String> {
        let pattern = format!("{}=", key);
        let idx = attrs.find(&pattern)?;
        let rest = &attrs[idx + pattern.len()..];
        let rest = rest.trim_start();

        let quote = rest.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let rest = &rest[1..]; // 跳过开引号
        let end = rest.find(quote)?;
        Some(rest[..end].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn collect_tags() -> (Arc<Mutex<Vec<InlineTag>>>, TagCallback) {
        let tags = Arc::new(Mutex::new(Vec::new()));
        let tags_clone = tags.clone();
        let cb: TagCallback = Box::new(move |tag| {
            tags_clone.lock().unwrap().push(tag);
        });
        (tags, cb)
    }

    #[test]
    fn test_expression_tag() {
        let (tags, cb) = collect_tags();
        let mut scanner = InlineTagScanner::new(cb);
        let clean = scanner.feed(r#"你好 <e name="happy" dur="3000"/> 世界"#);
        assert_eq!(clean, "你好  世界");
        let tags = tags.lock().unwrap();
        assert_eq!(tags.len(), 1);
        match &tags[0] {
            InlineTag::Expression { name, duration_ms } => {
                assert_eq!(name, "happy");
                assert_eq!(*duration_ms, Some(3000));
            }
            _ => panic!("expected Expression"),
        }
    }

    #[test]
    fn test_motion_tag() {
        let (tags, cb) = collect_tags();
        let mut scanner = InlineTagScanner::new(cb);
        let clean = scanner.feed(r#"<m name="wave"/>嗨"#);
        assert_eq!(clean, "嗨");
        assert_eq!(tags.lock().unwrap().len(), 1);
    }

    #[test]
    fn test_sticker_tag() {
        let (tags, cb) = collect_tags();
        let mut scanner = InlineTagScanner::new(cb);
        let clean = scanner.feed(r#"送你 <s name="flowers"/>"#);
        assert_eq!(clean, "送你 ");
        assert_eq!(tags.lock().unwrap().len(), 1);
    }

    #[test]
    fn test_multiple_tags_in_chunk() {
        let (tags, cb) = collect_tags();
        let mut scanner = InlineTagScanner::new(cb);
        let clean = scanner.feed(r#"<e name="shy"/>嗯 <m name="wave"/>拜拜"#);
        assert_eq!(clean, "嗯 拜拜");
        assert_eq!(tags.lock().unwrap().len(), 2);
    }

    #[test]
    fn test_no_tags() {
        let (tags, cb) = collect_tags();
        let mut scanner = InlineTagScanner::new(cb);
        let clean = scanner.feed("普通文本没有标签");
        assert_eq!(clean, "普通文本没有标签");
        assert_eq!(tags.lock().unwrap().len(), 0);
    }

    #[test]
    fn test_less_than_not_tag() {
        let (tags, cb) = collect_tags();
        let mut scanner = InlineTagScanner::new(cb);
        let clean = scanner.feed("3 < 5 对吧");
        assert_eq!(clean, "3 < 5 对吧");
        assert_eq!(tags.lock().unwrap().len(), 0);
    }

    #[test]
    fn test_tag_split_across_chunks() {
        let (tags, cb) = collect_tags();
        let mut scanner = InlineTagScanner::new(cb);

        // chunk 1: 标签未完成
        let clean1 = scanner.feed(r#"你好 <e name="happy""#);
        assert_eq!(clean1, "你好 ");
        assert_eq!(tags.lock().unwrap().len(), 0); // 还没检测到完整标签

        // chunk 2: 标签完成
        let clean2 = scanner.feed(r#" dur="3000"/> 世界"#);
        assert_eq!(clean2, " 世界");
        assert_eq!(tags.lock().unwrap().len(), 1);
    }

    #[test]
    fn test_flush_deferred() {
        let (tags, cb) = collect_tags();
        let mut scanner = InlineTagScanner::new(cb);
        let _ = scanner.feed(r#"text <e name="#);
        let remaining = scanner.flush();
        assert_eq!(remaining, r#"<e name="#);
        assert_eq!(tags.lock().unwrap().len(), 0);
    }

    #[test]
    fn test_invalid_tag_preserved() {
        let (tags, cb) = collect_tags();
        let mut scanner = InlineTagScanner::new(cb);
        let clean = scanner.feed(r#"<x name="unknown"/>"#);
        assert_eq!(clean, r#"<x name="unknown"/>"#);
        assert_eq!(tags.lock().unwrap().len(), 0);
    }
}
