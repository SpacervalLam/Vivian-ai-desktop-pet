//! Bounded navigation derived from the current Markdown, without a stale sidecar index.
pub const NAVIGATION_MAX_CHARS: usize = 2_000;
const PIN_BUDGET: usize = 800;

#[derive(Debug)]
struct Section {
    title: String,
    line: usize,
    offset: usize,
    end_line: usize,
    body: Vec<String>,
}

fn clip(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn sections(raw: &str) -> Vec<Section> {
    let mut result: Vec<Section> = Vec::new();
    let mut offset = 0;
    let mut fence: Option<&str> = None;
    for (index, line) in raw.split_inclusive('\n').enumerate() {
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
        let heading = fence.is_none().then(|| text.strip_prefix("## ")).flatten();
        if let Some(title) = heading {
            result.push(Section {
                title: title.into(),
                line: index + 1,
                offset,
                end_line: index + 1,
                body: Vec::new(),
            });
        } else if !text.is_empty() && !text.starts_with(['#', '>']) && !text.starts_with("<!--") {
            if result.is_empty() {
                result.push(Section {
                    title: "未分组记忆".into(),
                    line: index + 1,
                    offset,
                    end_line: index + 1,
                    body: Vec::new(),
                });
            }
            result
                .last_mut()
                .unwrap()
                .body
                .push(line.trim_end().to_string());
        }
        if let Some(section) = result.last_mut() {
            section.end_line = index + 1;
        }
        offset += line.chars().count();
    }
    result
        .into_iter()
        .filter(|s| !s.body.is_empty() && s.title != "整理来源材料")
        .collect()
}

/// Character offsets are relative to the unmodified file, including CRLF and Unicode.
pub fn navigation(raw: &str) -> Option<String> {
    let sections = sections(raw);
    if sections.is_empty() {
        return None;
    }
    let mut out = format!(
        "共 {} 个主题、{} 行。以下是导航，不代表已经读取全部记忆。\n",
        sections.len(),
        raw.lines().count()
    );
    let mut pin_remaining = PIN_BUDGET;
    for section in &sections {
        if ["常驻约定", "关键约定", "权限边界"].contains(&section.title.trim()) {
            let body = section.body.join("\n");
            let heading = format!(
                "\n[{}，行 {}–{}]\n",
                section.title, section.line, section.end_line
            );
            let excerpt = clip(
                &body,
                pin_remaining.saturating_sub(heading.chars().count() + 1),
            );
            if !excerpt.is_empty() {
                let block = format!("{heading}{excerpt}\n");
                pin_remaining = pin_remaining.saturating_sub(block.chars().count());
                out.push_str(&block);
            }
            if excerpt.chars().count() < body.chars().count() {
                out.push_str("常驻约定未完整展示，涉及它时必须读取原文。\n");
                break;
            }
        }
    }
    out.push_str("\n主题导航（offset 为 read_file 的字符偏移）：\n");
    // Show early and recent sections so an append cannot hide the oldest topics.
    let count = sections.len().min(6);
    let first = count.div_ceil(2);
    let mut indices: Vec<_> = (0..first).collect();
    indices.extend(sections.len().saturating_sub(count - first)..sections.len());
    indices.sort_unstable();
    indices.dedup();
    let mut shown = 0;
    for index in indices {
        let section = &sections[index];
        let preview = section
            .body
            .iter()
            .find(|s| !s.trim().starts_with(['`', '~']))
            .map(|s| clip(s, 65))
            .unwrap_or_default();
        let row = format!(
            "- {}：行 {}–{}，offset={}；{}\n",
            clip(&section.title, 40),
            section.line,
            section.end_line,
            section.offset,
            preview
        );
        if out.chars().count() + row.chars().count() + 140 > NAVIGATION_MAX_CHARS {
            break;
        }
        out.push_str(&row);
        shown += 1;
    }
    if shown < sections.len() {
        out.push_str(
            "其余主题未展示；用 grep_search 搜索记忆目录，未命中时停止查询，不猜测内容。\n",
        );
    }
    Some(clip(&out, NAVIGATION_MAX_CHARS))
}

pub fn source_marker(id: &str) -> String {
    format!("<!-- memory-source: memory_sources/{id}.json -->")
}

/// Provenance is maintained by the host; it need not consume the memory model's
/// input budget or make every subsequent append trigger a full rewrite.
pub fn memory_for_model(raw: &str) -> String {
    raw.lines()
        .filter(|line| {
            let line = line.trim();
            !line.contains("<!-- memory-source:")
                && line != "## 整理来源材料"
                && line != "这些来源供核对整理过程，不表示每条结论都由每份材料证明。"
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Merge overlapping byte spans before replacement (e.g. api_key=sk-... matches
/// both generic and provider-specific detectors). Regex spans are UTF-8 boundaries.
pub fn redact_ranges(text: &str, mut ranges: Vec<(usize, usize)>) -> String {
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        if start >= end
            || end > text.len()
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
        {
            continue;
        }
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    let mut out = String::new();
    let mut offset = 0;
    for (start, end) in merged {
        out.push_str(&text[offset..start]);
        out.push_str("[REDACTED_SECRET]");
        offset = end;
    }
    out.push_str(&text[offset..]);
    out
}

fn is_source_marker(line: &str) -> bool {
    line.strip_prefix("<!-- memory-source: memory_sources/")
        .and_then(|s| s.strip_suffix(".json -->"))
        .is_some_and(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Model text cannot manufacture provenance. Rewrites retain the existing source
/// pool as references to consolidation materials, not per-fact proof.
pub fn attach_source(existing: Option<&str>, body: &str, marker: &str, rewrite: bool) -> String {
    let clean: Vec<_> = body
        .lines()
        .filter(|line| !line.contains("<!-- memory-source:"))
        .collect();
    let mut out = clean.join("\n");
    let mut sources = std::collections::BTreeSet::new();
    if rewrite {
        for line in existing
            .unwrap_or("")
            .lines()
            .map(str::trim)
            .filter(|s| is_source_marker(s))
        {
            sources.insert(line.to_string());
        }
        out.push_str(
            "\n\n## 整理来源材料\n这些来源供核对整理过程，不表示每条结论都由每份材料证明。\n",
        );
    }
    if is_source_marker(marker) {
        sources.insert(marker.to_string());
    }
    for source in sources {
        out.push('\n');
        out.push_str(&source);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_is_bounded_but_keeps_early_recent_and_pinned_topics() {
        let mut raw = "# 项目记忆\n## 常驻约定\n- 不自动发布\n".to_string();
        for i in 0..100 {
            raw.push_str(&format!("## 经验{i}\n- {}\n", "细节".repeat(100)));
        }
        let nav = navigation(&raw).unwrap();
        assert!(nav.chars().count() <= NAVIGATION_MAX_CHARS);
        assert!(nav.contains("不自动发布"));
        assert!(nav.contains("经验0"));
        assert!(nav.contains("经验99"));
        assert!(nav.contains("其余主题未展示"));
        assert!(!nav.contains(&"细节".repeat(100)));
    }

    #[test]
    fn offsets_match_the_actual_unicode_crlf_file_and_refresh_after_edit() {
        let raw = "\r\n# 项目记忆\r\n## 入口\r\n- 甲🙂乙\r\n## 测试\r\n- cargo test\r\n";
        let parsed = sections(raw);
        assert_eq!(parsed[1].line, 5);
        assert!(raw
            .chars()
            .skip(parsed[1].offset)
            .collect::<String>()
            .starts_with("## 测试"));
        let edited = raw.replace("测试", "新命令");
        assert!(navigation(&edited).unwrap().contains("新命令"));
        assert!(!navigation(&edited).unwrap().contains("测试"));
    }

    #[test]
    fn empty_headers_and_code_headings_are_not_topics() {
        assert!(navigation("# 项目记忆\n> 使用说明\n").is_none());
        assert_eq!(
            sections("## 示例\n```md\n## 代码内标题\n```\n- 注释\n").len(),
            1
        );
    }

    #[test]
    fn many_manual_pins_cannot_crowd_out_navigation() {
        let mut raw = "## 最早经验\n- first entry\n".to_string();
        for i in 0..100 {
            raw.push_str(&format!("## 常驻约定\n- 用户要求{i}\n"));
        }
        raw.push_str("## 最新经验\n- latest entry\n");
        let nav = navigation(&raw).unwrap();
        assert!(nav.chars().count() <= NAVIGATION_MAX_CHARS);
        assert!(nav.contains("常驻约定未完整展示"));
        assert!(nav.contains("最早经验") && nav.contains("最新经验"));
    }

    #[test]
    fn rewrite_keeps_host_sources_and_rejects_invented_model_sources() {
        let old = source_marker(&"a".repeat(64));
        let new = source_marker(&"b".repeat(64));
        let fake = source_marker(&"c".repeat(64));
        let result = attach_source(Some(&old), &format!("- 新命令\n{fake}"), &new, true);
        assert!(result.contains(&old) && result.contains(&new));
        assert!(!result.contains(&fake));
        assert!(result.contains("不表示每条结论"));
        assert!(!attach_source(None, &format!("- inline {fake}"), &new, false).contains(&fake));
        assert_eq!(memory_for_model(&result).trim(), "- 新命令");
    }

    #[test]
    fn overlapping_redactions_preserve_unicode_and_never_copy_secret_fragments() {
        let text = "甲🙂api_key=sk-secret-value乙";
        let start = text.find("api_key").unwrap();
        let secret = text.find("sk-").unwrap();
        let end = text.find('乙').unwrap();
        let clean = redact_ranges(text, vec![(secret, end), (start, end)]);
        assert_eq!(clean, "甲🙂[REDACTED_SECRET]乙");
    }
}
