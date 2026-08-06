pub mod cancel_token;
pub mod environment;
pub mod job_object;
pub mod path;
pub mod pid_file;
pub mod playback_gate;
pub mod powershell;
pub mod proactive_leader;
pub mod process;
pub mod session_coordinator;
pub mod system_idle;
pub mod token_estimate;

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
