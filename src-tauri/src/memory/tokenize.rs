//! 中文分词工具 - jieba-rs + 英文按非字母数字分割
//!
//! 用于 BM25 检索与哈希嵌入。cut_for_search 为搜索引擎优化分词，
//! 会对长词再细切，提高召回率。

use jieba_rs::Jieba;
use once_cell::sync::Lazy;

/// 全局 jieba 实例（首次使用时初始化）
static JIEBA: Lazy<Jieba> = Lazy::new(Jieba::new);

/// 进程级共享 Jieba 词典。
///
/// Jieba 初始化会展开一份较大的内置词典；各业务模块各建一个 `static Jieba`
/// 会按模块重复常驻同一份数据。所有分词/词性标注统一复用此实例。
pub fn jieba() -> &'static Jieba {
    &JIEBA
}

/// 判断字符是否为 CJK 字符
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F |   // CJK Symbols and Punctuation
        0x3040..=0x309F |   // Hiragana
        0x30A0..=0x30FF |   // Katakana
        0x3400..=0x4DBF |   // CJK Extension A
        0x4E00..=0x9FFF |   // CJK Unified Ideographs
        0xF900..=0xFAFF |   // CJK Compatibility Ideographs
        0xFF00..=0xFFEF     // Halfwidth and Fullwidth Forms
    )
}

/// 分词（jieba 中文分词 + 英文按非字母数字分割，统一转小写）
pub fn tokenize(text: &str) -> Vec<String> {
    let words = jieba().cut_for_search(text, true);
    let mut tokens: Vec<String> = Vec::new();
    for word in words {
        let word = word.trim();
        if word.is_empty() {
            continue;
        }
        if word.chars().any(|c| c.is_ascii_alphanumeric()) {
            for part in word.split(|c: char| !c.is_alphanumeric()) {
                if !part.is_empty() {
                    tokens.push(part.to_lowercase());
                }
            }
        } else if word.chars().any(is_cjk) {
            tokens.push(word.to_lowercase());
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_chinese() {
        let tokens = tokenize("我喜欢吃苹果");
        assert!(tokens.contains(&"喜欢".to_string()) || tokens.contains(&"我".to_string()));
    }

    #[test]
    fn test_tokenize_english() {
        let tokens = tokenize("Hello World");
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"world".to_string()));
    }

    #[test]
    fn test_tokenize_mixed() {
        let tokens = tokenize("使用 React 18 开发");
        assert!(tokens.iter().any(|t| t == "react"));
        assert!(tokens.iter().any(|t| t == "使用"));
    }

    #[test]
    fn test_tokenize_empty() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("   ").is_empty());
        assert!(tokenize("...").is_empty());
    }
}
