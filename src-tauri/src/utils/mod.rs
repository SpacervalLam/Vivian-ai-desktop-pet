pub mod autostart;
pub mod cancel_token;
pub mod environment;
pub mod fs;
pub mod job_object;
pub mod path;
pub mod pid_file;
pub mod power_events;
pub mod playback_gate;
pub mod powershell;
pub mod proactive_leader;
pub mod process;
pub mod session_coordinator;
pub mod system_idle;
pub mod token_estimate;
pub mod watchdog;

pub use environment::{CurrentState, EnvironmentInfo, EnvironmentManager, UserActivity};
pub use path::get_user_data_dir;
pub use playback_gate::{PlaybackEvent, PlaybackGate};
pub use powershell::{run_ps, run_ps_async};
pub use proactive_leader::ProactiveLeaderCoordinator;
pub use process::{silent_command, silent_command_async};
pub use session_coordinator::{SessionCoordinator, TurnGuard, TurnKind};
pub use system_idle::get_system_idle_seconds;

/// FNV-1a 64-bit 哈希（字节切片版本，用于二进制数据如像素缓冲区）
pub fn fnv1a_64_bytes(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// FNV-1a 64-bit 哈希（用于缓存键生成等非加密场景）
pub fn fnv1a_64(s: &str) -> u64 {
    fnv1a_64_bytes(s.as_bytes())
}

/// 按 Unicode 字符截断字符串（避免在多字节字符中间截断）
pub fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 按 Unicode 字符截断并追加省略号
pub fn truncate_chars_with_ellipsis(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t = truncate_chars(s, n);
        t.push('…');
        t
    }
}

/// 由消息列表生成缓存 key（用 `|` 连接所有消息 content）
pub fn messages_cache_key(messages: &[crate::types::response::ChatMessage]) -> String {
    messages
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("|")
}

/// 移除文本中括号及其内容
///
/// 移除所有中文括号（()、（））及其内部的文本内容。
/// 对于未闭合的括号（流式场景），暂存到 deferred 中，等待下一个 chunk。
///
/// 返回值: (清理后的文本, 暂存的未闭合括号内容)
pub fn filter_parentheses(text: &str, deferred: &str) -> (String, String) {
    let mut combined = String::with_capacity(deferred.len() + text.len());
    combined.push_str(deferred);
    combined.push_str(text);

    let mut result = String::with_capacity(combined.len());
    let mut in_paren = false;
    let mut paren_count = 0;
    let mut i = 0;
    let chars: Vec<char> = combined.chars().collect();

    while i < chars.len() {
        let c = chars[i];
        if c == '(' || c == '（' {
            if !in_paren {
                in_paren = true;
                paren_count = 1;
            } else {
                paren_count += 1;
            }
            i += 1;
            continue;
        }

        if c == ')' || c == '）' {
            if in_paren {
                paren_count -= 1;
                if paren_count == 0 {
                    in_paren = false;
                }
            } else {
                result.push(c);
            }
            i += 1;
            continue;
        }

        if in_paren {
            i += 1;
            continue;
        }

        result.push(c);
        i += 1;
    }

    let remaining = if in_paren {
        let start_idx = result.len();
        let mut remaining_text = String::new();
        let mut j = start_idx;
        while j < chars.len() {
            remaining_text.push(chars[j]);
            j += 1;
        }
        remaining_text
    } else {
        String::new()
    };

    (result, remaining)
}

/// 非流式版本：一次性移除所有括号内容
pub fn filter_parentheses_sync(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_paren = false;
    let mut paren_count = 0;

    for c in text.chars() {
        if c == '(' || c == '（' {
            if !in_paren {
                in_paren = true;
                paren_count = 1;
            } else {
                paren_count += 1;
            }
            continue;
        }

        if c == ')' || c == '）' {
            if in_paren {
                paren_count -= 1;
                if paren_count == 0 {
                    in_paren = false;
                }
            } else {
                result.push(c);
            }
            continue;
        }

        if !in_paren {
            result.push(c);
        }
    }

    result
}

/// 清洗 Markdown / 富文本渲染语法，仅保留纯文本内容。
///
/// 处理：代码块、粗体、斜体、删除线、行内代码、图片、链接、
/// 标题前缀、引用前缀、无序/有序列表前缀、分隔线、HTML 标签。
/// 保留换行与自然空格（不压缩为单行），适合显示与持久化。
pub fn strip_markdown_syntax(text: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;

    static RE_CODE_BLOCK: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"```[\s\S]*?```").unwrap());
    static RE_BOLD: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\*\*(.+?)\*\*").unwrap());
    static RE_BOLD_UNDER: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"__(.+?)__").unwrap());
    static RE_ITALIC: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\*(\S(?:[^*\n]*?\S)?)\*").unwrap());
    static RE_STRIKE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"~~(.+?)~~").unwrap());
    static RE_INLINE_CODE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"`([^`]+)`").unwrap());
    static RE_IMAGE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"!\[([^\]]*)\]\([^)]*\)").unwrap());
    static RE_LINK: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap());
    static RE_HEADER: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?m)^#{1,6}\s+").unwrap());
    static RE_BLOCKQUOTE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?m)^>\s?").unwrap());
    static RE_UL_LIST: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?m)^[\s]*[-*+]\s+").unwrap());
    static RE_OL_LIST: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?m)^[\s]*\d+[.)]\s+").unwrap());
    static RE_HR: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?m)^[-*_]{3,}\s*$").unwrap());
    static RE_HTML_TAG: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"</?[a-zA-Z][^>]*>").unwrap());

    let s = text;
    let s = RE_CODE_BLOCK.replace_all(&s, "");
    let s = RE_BOLD.replace_all(&s, "$1");
    let s = RE_BOLD_UNDER.replace_all(&s, "$1");
    let s = RE_ITALIC.replace_all(&s, "$1");
    let s = RE_STRIKE.replace_all(&s, "$1");
    let s = RE_INLINE_CODE.replace_all(&s, "$1");
    let s = RE_IMAGE.replace_all(&s, "$1");
    let s = RE_LINK.replace_all(&s, "$1");
    let s = RE_HEADER.replace_all(&s, "");
    let s = RE_BLOCKQUOTE.replace_all(&s, "");
    let s = RE_UL_LIST.replace_all(&s, "");
    let s = RE_OL_LIST.replace_all(&s, "");
    let s = RE_HR.replace_all(&s, "");
    let s = RE_HTML_TAG.replace_all(&s, "");

    // 压缩连续空行为最多两个换行，去除行尾多余空格
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for line in s.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank += 1;
            if blank <= 1 {
                out.push('\n');
            }
        } else {
            blank = 0;
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out.trim().to_string()
}
