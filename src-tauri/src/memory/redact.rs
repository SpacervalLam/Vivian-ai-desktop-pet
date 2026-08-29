use std::collections::HashMap;
use std::hash::Hasher;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use regex::Regex;

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactStatus {
    Clean,
    Detected,
    Redacted,
}

impl Default for RedactStatus {
    fn default() -> Self {
        RedactStatus::Clean
    }
}

impl RedactStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RedactStatus::Clean => "clean",
            RedactStatus::Detected => "detected",
            RedactStatus::Redacted => "redacted",
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PiiSpan {
    pub pii_type: String,
    pub start: usize,
    pub end: usize,
    pub tracker_id: String,
    pub redacted: bool,
}

struct PiiPattern {
    pii_type: &'static str,
    regex: Regex,
}

static PATTERNS: Lazy<Vec<PiiPattern>> = Lazy::new(|| {
    vec![
        PiiPattern {
            pii_type: "api_key_openai",
            regex: Regex::new(r"sk-[a-zA-Z0-9]{20,}").unwrap(),
        },
        PiiPattern {
            pii_type: "api_key_generic",
            regex: Regex::new(r#"(?i)(?:api[_-]?key|secret|token|bearer)\s*[:=]\s*['"]?[a-zA-Z0-9_\-]{16,}['"]?"#).unwrap(),
        },
        PiiPattern {
            pii_type: "email",
            regex: Regex::new(r"[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}").unwrap(),
        },
        PiiPattern {
            pii_type: "phone_cn",
            regex: Regex::new(r"\b1[3-9]\d{9}\b").unwrap(),
        },
        PiiPattern {
            pii_type: "id_card_cn",
            regex: Regex::new(r"\b\d{17}[\dXx]\b").unwrap(),
        },
        PiiPattern {
            pii_type: "bank_card",
            regex: Regex::new(r"\b\d{16,19}\b").unwrap(),
        },
        PiiPattern {
            pii_type: "password",
            regex: Regex::new(r"(?i)(?:password|passwd|pwd)\s*[:=]\s*\S+").unwrap(),
        },
        PiiPattern {
            pii_type: "ipv4",
            regex: Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b").unwrap(),
        },
    ]
});

static TRACKER_STORE: Lazy<RwLock<HashMap<String, String>>> = Lazy::new(|| RwLock::new(HashMap::new()));

fn compute_tracker_id(pii_type: &str, matched_text: &str) -> String {
    use std::hash::Hash;
    let mut hasher = std::hash::DefaultHasher::new();
    pii_type.hash(&mut hasher);
    matched_text.hash(&mut hasher);
    format!("{}_{:016x}", pii_type, hasher.finish())
}

pub fn detect_pii(text: &str) -> Vec<PiiSpan> {
    let mut spans = Vec::new();
    for pattern in PATTERNS.iter() {
        for mat in pattern.regex.find_iter(text) {
            let matched = mat.as_str();
            let tracker_id = compute_tracker_id(pattern.pii_type, matched);
            {
                let mut store = TRACKER_STORE.write();
                store.entry(tracker_id.clone()).or_insert_with(|| matched.to_string());
            }
            spans.push(PiiSpan {
                pii_type: pattern.pii_type.to_string(),
                start: mat.start(),
                end: mat.end(),
                tracker_id,
                redacted: false,
            });
        }
    }
    spans.sort_by_key(|s| s.start);
    spans
}

pub fn redact_content(text: &str) -> (String, Vec<PiiSpan>, RedactStatus) {
    let spans = detect_pii(text);
    if spans.is_empty() {
        return (text.to_string(), spans, RedactStatus::Clean);
    }

    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    let mut redacted_spans = Vec::with_capacity(spans.len());

    for mut span in spans {
        result.push_str(&text[last_end..span.start]);
        let placeholder = format!("[{}]", span.pii_type.to_uppercase());
        result.push_str(&placeholder);
        span.redacted = true;
        redacted_spans.push(span.clone());
        last_end = span.end;
    }
    result.push_str(&text[last_end..]);

    (result, redacted_spans, RedactStatus::Redacted)
}

pub fn redact_for_log(text: &str, max_len: usize) -> String {
    let (redacted, _, status) = redact_content(text);
    if redacted.chars().count() <= max_len {
        redacted
    } else {
        let truncated: String = redacted.chars().take(max_len).collect();
        match status {
            RedactStatus::Clean => format!("{}…", truncated),
            RedactStatus::Redacted => format!("{}…(redacted)", truncated),
            _ => truncated,
        }
    }
}

pub fn has_pii(text: &str) -> bool {
    PATTERNS.iter().any(|p| p.regex.is_match(text))
}

pub fn tracker_lookup(tracker_id: &str) -> Option<String> {
    TRACKER_STORE.read().get(tracker_id).cloned()
}

pub fn redact_status_from_metadata(meta: &serde_json::Value) -> RedactStatus {
    meta.get("redact_status")
        .and_then(|v| v.as_str())
        .map(|s| match s {
            "detected" => RedactStatus::Detected,
            "redacted" => RedactStatus::Redacted,
            _ => RedactStatus::Clean,
        })
        .unwrap_or(RedactStatus::Clean)
}

/// 占位符残留正则：匹配 redact_content 产出的 `[大写类型]` 标记
static PLACEHOLDER_RESIDUE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[[A-Z][A-Z0-9_]*\]").unwrap());

/// 判断脱敏后的内容是否只剩占位符/空白（即原始内容为纯凭据，无语义价值）
///
/// 这类消息不值得入库：占位符不携带任何可检索的语义，只会污染条目库、
/// 向量索引与巩固流水线。调用方应据此跳过写入。
pub fn is_pure_placeholder_content(redacted_text: &str) -> bool {
    let residue = PLACEHOLDER_RESIDUE_RE.replace_all(redacted_text, "");
    residue.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_pure_placeholder_content() {
        // 纯凭据（脱敏后只剩占位符）
        assert!(is_pure_placeholder_content("[API_KEY_OPENAI]"));
        assert!(is_pure_placeholder_content("[PHONE_CN] [ID_CARD_CN]  "));
        // 混合内容（有语义残留）
        assert!(!is_pure_placeholder_content("我的 [PHONE_CN] 打不通"));
        assert!(!is_pure_placeholder_content("今天聊了很多"));
        // 中文方括号内容不受影响
        assert!(!is_pure_placeholder_content("[轻声笑了笑]"));
        assert!(is_pure_placeholder_content(""));
    }
}
