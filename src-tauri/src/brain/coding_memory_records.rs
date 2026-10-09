//! Host-owned memory identity, citations and lossless bounded transcript batches.
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Candidate {
    pub text: String,
    #[serde(default = "default_topic")]
    pub topic: String,
    pub source_messages: Vec<usize>,
    #[serde(default)]
    pub supersedes: Vec<String>,
}

fn default_topic() -> String {
    "经验".into()
}

pub fn batches(records: &[(usize, String)], budget: usize) -> Vec<(String, Vec<usize>)> {
    let mut batches = Vec::new();
    let mut text = String::new();
    let mut indices = Vec::new();
    for (index, record) in records {
        let context = serde_json::from_str::<serde_json::Value>(record)
            .ok()
            .map(|value| {
                let role: String = value
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .chars()
                    .take(30)
                    .collect();
                format!(
                    ", role={role}, timestamp_ms={}",
                    value
                        .get("timestamp_ms")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0)
                )
            })
            .unwrap_or_default();
        let chars: Vec<_> = record.chars().collect();
        let size = budget.saturating_sub(150).max(1);
        let total = chars.len().max(1).div_ceil(size);
        for part in 0..total {
            let piece: String = chars.iter().skip(part * size).take(size).collect();
            let line = format!(
                "\n[message={index}, part={}/{total}{context}]\n{piece}\n",
                part + 1
            );
            if !text.is_empty() && text.chars().count() + line.chars().count() > budget {
                batches.push((std::mem::take(&mut text), std::mem::take(&mut indices)));
            }
            text.push_str(&line);
            if !indices.contains(index) {
                indices.push(*index);
            }
        }
    }
    if !text.is_empty() {
        batches.push((text, indices));
    }
    batches
}

pub fn parse_candidates(raw: &str, allowed: &[usize]) -> Result<Vec<Candidate>, String> {
    let raw = raw
        .trim()
        .strip_prefix("```json")
        .or_else(|| raw.trim().strip_prefix("```"))
        .unwrap_or(raw.trim())
        .trim()
        .trim_end_matches("```")
        .trim();
    let mut candidates: Vec<Candidate> =
        serde_json::from_str(raw).map_err(|e| format!("提炼输出不是有效条目 JSON: {e}"))?;
    for candidate in &mut candidates {
        candidate.text = candidate.text.trim().to_string();
        if candidate.text.is_empty()
            || candidate.text.contains(['\n', '\r'])
            || candidate.text.contains("<!--")
            || candidate.topic.is_empty()
            || candidate.topic.contains(['\n', '\r', '<', '>'])
            || candidate.topic.chars().count() > 40
            || ["常驻约定", "关键约定", "权限边界"].contains(&candidate.topic.trim())
            || candidate.source_messages.is_empty()
            || candidate
                .source_messages
                .iter()
                .any(|i| !allowed.contains(i))
            || candidate.supersedes.iter().any(|id| !valid_id(id))
        {
            return Err("记忆条目正文或来源索引无效，未提交".into());
        }
        candidate.source_messages.sort_unstable();
        candidate.source_messages.dedup();
    }
    Ok(candidates)
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn job_marker(id: &str) -> String {
    format!("<!-- memory-job: {id} -->")
}

pub fn committed(raw: &str, id: &str) -> bool {
    valid_id(id) && raw.lines().any(|line| line.trim() == job_marker(id))
}

/// Keep user-authored rules verbatim; model consolidation never owns these sections.
pub fn pinned_rules(raw: &str) -> String {
    let mut selected = false;
    let mut fence = None;
    let mut out = String::new();
    for line in raw.split_inclusive('\n') {
        let text = line.trim();
        let marker = if text.starts_with("```") {
            Some("```")
        } else if text.starts_with("~~~") {
            Some("~~~")
        } else {
            None
        };
        if let Some(marker) = marker {
            if fence == Some(marker) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(marker);
            }
        }
        if fence.is_none() {
            if let Some(title) = text.strip_prefix("## ") {
                selected = ["常驻约定", "关键约定", "权限边界"].contains(&title);
            }
        }
        if selected {
            out.push_str(line);
        }
    }
    out
}

/// Compaction only removes identical plain bullet lines. Annotated entries,
/// citations, job receipts and rules are kept verbatim, so provenance survives.
pub fn consolidate(raw: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut out = String::new();
    let mut protected = false;
    let mut annotated = false;
    let mut fence = None;
    let lines: Vec<_> = raw.split_inclusive('\n').collect();
    for (index, line) in lines.iter().enumerate() {
        let text = line.trim();
        let marker = if text.starts_with("```") {
            Some("```")
        } else if text.starts_with("~~~") {
            Some("~~~")
        } else {
            None
        };
        if let Some(marker) = marker {
            if fence == Some(marker) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(marker);
            }
            out.push_str(line);
            continue;
        }
        if fence.is_some() {
            out.push_str(line);
            continue;
        }
        if let Some(title) = text.strip_prefix("## ") {
            protected = ["常驻约定", "关键约定", "权限边界"].contains(&title);
            seen.clear();
        }
        if text.starts_with("<!-- memory-entry:") {
            annotated = true;
        }
        let follows_source = lines
            .get(index + 1)
            .is_some_and(|l| l.contains("memory-source:"));
        if protected
            || annotated
            || follows_source
            || !text.starts_with("- ")
            || seen.insert(text.to_string())
        {
            out.push_str(line);
        }
        if text.starts_with("- ") {
            annotated = false;
        }
    }
    out
}

pub fn retry_at(now: i64, failures: u32) -> i64 {
    now.saturating_add(
        2_000_i64
            .saturating_mul(1_i64 << failures.min(10))
            .min(3_600_000),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_fragment_keeps_the_speaker_and_observation_time() {
        let record =
            serde_json::json!({"role":"user", "timestamp_ms":123, "content":"长文".repeat(10000)})
                .to_string();
        let parts = batches(&[(7, record)], 6000);
        assert!(parts.len() > 1);
        assert!(parts
            .iter()
            .all(|(text, _)| text.contains("role=user, timestamp_ms=123")
                && text.chars().count() <= 6000));
    }
    #[test]
    fn lossless_batches_keep_long_unicode_messages_and_late_corrections() {
        let original = format!("{}最终纠正", "长内容\n".repeat(4000));
        let chunks = batches(
            &[(0, original.clone()), (1, "cargo test: failed".into())],
            6000,
        );
        assert!(chunks.iter().all(|(text, _)| text.chars().count() <= 6000));
        let recovered = chunks
            .iter()
            .flat_map(|(text, _)| {
                text.split('\n')
                    .skip(2)
                    .filter(|s| !s.starts_with("[message="))
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(recovered.contains("最终纠正") && recovered.contains("cargo test: failed"));
        assert_eq!(
            chunks
                .iter()
                .map(|(text, _)| text.matches('长').count())
                .sum::<usize>(),
            4000
        );
    }
    #[test]
    fn citations_are_bounded_to_actual_batch_and_cannot_invent_receipts() {
        assert!(parse_candidates(r#"[{"text":"rule","source_messages":[9]}]"#, &[0]).is_err());
        assert!(parse_candidates(
            r#"[{"text":"<!-- injected -->","source_messages":[0]}]"#,
            &[0]
        )
        .is_err());
        assert_eq!(
            parse_candidates(r#"[{"text":"rule","source_messages":[0,0]}]"#, &[0]).unwrap()[0]
                .source_messages,
            vec![0]
        );
    }
    #[test]
    fn consolidation_preserves_rules_annotations_and_replay_receipts() {
        let id = "a".repeat(64);
        let raw = format!("## 常驻约定\n- 禁止发布\n- 禁止发布\n## 经验\n- old\n- old\n<!-- memory-entry: memory_entries/{id}.json -->\n- old\n{}\n", job_marker(&id));
        let result = consolidate(&raw);
        assert_eq!(pinned_rules(&result), pinned_rules(&raw));
        assert!(committed(&result, &id));
        assert_eq!(result.matches("- old").count(), 2);
    }
    #[test]
    fn retries_back_off_and_saturate_safely() {
        assert!(retry_at(0, 5) > retry_at(0, 1));
        assert!(retry_at(0, u32::MAX) <= 3_600_000);
    }
}
