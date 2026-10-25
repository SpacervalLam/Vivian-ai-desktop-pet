//! Hide parenthetical narration from spoken output, including across stream chunks.

/// Returns visible text and the unfinished parenthetical suffix, including its opening bracket.
/// At end of stream, discard the suffix; it is narration even if the model omitted a closing bracket.
pub fn filter_parentheses(text: &str, deferred: &str) -> (String, String) {
    let mut combined = String::with_capacity(deferred.len() + text.len());
    combined.push_str(deferred);
    combined.push_str(text);

    let mut result = String::with_capacity(combined.len());
    let mut depth = 0;
    let mut open_start = None;
    for (index, c) in combined.char_indices() {
        match c {
            '(' | '（' => {
                if depth == 0 {
                    open_start = Some(index);
                }
                depth += 1;
            }
            ')' | '）' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    open_start = None;
                }
            }
            _ if depth == 0 => result.push(c),
            _ => {}
        }
    }

    let remaining = open_start.map(|start| combined[start..].to_string()).unwrap_or_default();
    (result, remaining)
}

pub fn filter_parentheses_sync(text: &str) -> String {
    filter_parentheses(text, "").0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_the_actual_opening_bracket_in_deferred_text() {
        let (visible, deferred) = filter_parentheses("哇，原来是你呀！（猫耳抖了抖", "");
        assert_eq!(visible, "哇，原来是你呀！");
        assert_eq!(deferred, "（猫耳抖了抖");
        let (visible, deferred) = filter_parentheses("，好奇地凑近屏幕）那你录的时候可别漏出去哦~", &deferred);
        assert_eq!(visible, "那你录的时候可别漏出去哦~");
        assert!(deferred.is_empty());
    }

    #[test]
    fn defers_after_closed_narration_and_multibyte_speech() {
        let (visible, deferred) = filter_parentheses("（点头）好呀🙂(轻轻", "");
        assert_eq!(visible, "好呀🙂");
        assert_eq!(deferred, "(轻轻");
        assert_eq!(filter_parentheses("笑)继续聊。", &deferred), ("继续聊。".into(), String::new()));
    }

    #[test]
    fn streaming_matches_complete_output_at_every_pair_of_boundaries() {
        for (raw, expected) in [
            ("哇，原来是你呀！（猫耳抖了抖，好奇地凑近屏幕）那你录的时候可别把我们的小秘密漏出去哦~", "哇，原来是你呀！那你录的时候可别把我们的小秘密漏出去哦~"),
            ("（点头）好呀🙂(微笑)继续（抬手(轻轻挥动)）聊。", "好呀🙂继续聊。"),
            ("A（outer(inner）end)B", "AB"),
            ("嗯（猫耳抖了抖", "嗯"),
            ("（只有动作）", ""),
            ("普通回复🙂\n下一句。", "普通回复🙂\n下一句。"),
            ("孤立右括号）照旧", "孤立右括号）照旧"),
        ] {
            assert_eq!(filter_parentheses_sync(raw), expected);
            let boundaries: Vec<_> = raw.char_indices().map(|(i, _)| i).chain([raw.len()]).collect();
            for &first in &boundaries {
                for &second in boundaries.iter().filter(|&&i| i >= first) {
                    let mut output = String::new();
                    let mut deferred = String::new();
                    for chunk in [&raw[..first], &raw[first..second], &raw[second..]] {
                        let (visible, pending) = filter_parentheses(chunk, &deferred);
                        output.push_str(&visible);
                        deferred = pending;
                    }
                    assert_eq!(output, expected, "split at {first}, {second}: {raw}");
                    assert_eq!(filter_parentheses_sync(&deferred), "");
                }
            }
        }
    }
}
