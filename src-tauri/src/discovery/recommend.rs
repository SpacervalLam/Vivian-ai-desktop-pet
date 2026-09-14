//! 推荐服务 — 从库存选内容并生成朋友式推荐文案
//!
//! 选择策略（纯规则，无 LLM）：
//! - 未推荐过 + 未收反馈
//! - 同 topic_group 最多 1 条（主题去重）
//! - score 降序，主题疲劳降权（最近 24h 内已推荐过同主题 → 大幅降权）
//!
//! 文案生成（LLM）：像朋友私聊一样解释为什么你会喜欢，
//! 必须引用至少一个具体内容细节，禁止算法套话。

use serde_json::Value;

use super::profile::InterestProfile;
use super::store::{ContentItem, ContentStore};

/// 选取推荐（不标记已推荐；由调用方在用户实际看到后调用 mark_recommended）
pub fn pick_recommendations(char_id: &str, limit: usize) -> Vec<ContentItem> {
    let store = ContentStore::load(char_id);
    let now = chrono::Utc::now().timestamp();

    // 候选：未推荐 + 未反馈
    let mut candidates: Vec<&ContentItem> = store
        .items
        .iter()
        .filter(|i| i.recommended_count == 0 && i.feedback.is_empty())
        .collect();

    // 主题疲劳：最近 24h 推荐过的主题降权
    let recent_topics: Vec<String> = store
        .items
        .iter()
        .filter(|i| i.last_recommended_at > 0 && now - i.last_recommended_at < 86400)
        .map(|i| i.topic_group.clone())
        .collect();

    candidates.sort_by(|a, b| {
        let sa = effective_score(a, &recent_topics);
        let sb = effective_score(b, &recent_topics);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });

    // 主题去重：同 topic_group 最多 1 条
    let mut picked: Vec<ContentItem> = Vec::new();
    let mut used_topics: Vec<String> = Vec::new();
    for item in candidates {
        if picked.len() >= limit {
            break;
        }
        if used_topics.contains(&item.topic_group) {
            continue;
        }
        used_topics.push(item.topic_group.clone());
        picked.push(item.clone());
    }
    picked
}

/// 有效分：基础分 - 主题疲劳惩罚
fn effective_score(item: &ContentItem, recent_topics: &[String]) -> f64 {
    let mut score = item.score;
    if recent_topics.iter().any(|t| *t == item.topic_group) {
        score -= 0.2;
    }
    score
}

/// LLM 生成朋友式推荐文案（批量，一次调用）
/// 返回与输入一一对应的 expression；LLM 失败或个别条目解析失败时回退到评估 reason。
pub async fn generate_expressions(
    profile: &InterestProfile,
    items: &[ContentItem],
) -> Vec<String> {
    if items.is_empty() {
        return Vec::new();
    }

    let fallback: Vec<String> = items
        .iter()
        .map(|i| {
            if i.reason.is_empty() {
                format!("「{}」，感觉你会感兴趣", i.title)
            } else {
                i.reason.clone()
            }
        })
        .collect();

    let system = "你要像一个真正懂这个人的朋友一样，为多条候选内容（来自 B 站/Bangumi/V2EX 等平台）\
各写一段推荐话。像朋友私聊，不是推荐引擎。\
输出严格 JSON 数组，长度与输入一致，顺序一一对应，每项原样带回 id。";

    let content_items: Vec<Value> = items
        .iter()
        .map(|i| {
            let desc: String = if i.description.chars().count() > 100 {
                i.description.chars().take(100).collect()
            } else {
                i.description.clone()
            };
            serde_json::json!({
                "id": i.bvid,
                "platform": i.platform,
                "title": i.title,
                "description": desc,
                "author": i.up_name,
                "duration_secs": i.duration_secs,
                "internal_reason": i.reason,
            })
        })
        .collect();

    let user = format!(
        "{}\n\n## 要求\n\
1. 每条 expression 50-150 字中文口语，像朋友随口安利\n\
2. 必须引用至少一个具体内容细节（标题关键词、作者特点、独特切入角度），不要说空话\n\
3. 避免：算法套话、信息密度、高质量、深度好文、值得一看、强烈推荐\n\
4. 每条开头措辞必须不同，禁止重复同一句式\n\
5. 避开画像中用户不喜欢的主题或话术模式\n\n\
严格输出 JSON：[{{\"id\":\"...\",\"expression\":\"...\"}}]\n\n\
## 候选内容\n{}",
        profile.to_prompt_context(),
        serde_json::to_string(&content_items).unwrap_or_default(),
    );

    let Some(content) = super::llm_complete(system, &user, Some(0.9)).await else {
        return fallback;
    };
    let Some(value) = super::parse_json_tolerant(&content) else {
        return fallback;
    };
    let arr = match value.as_array() {
        Some(a) => a.clone(),
        None => match value.get("expressions").and_then(|e| e.as_array()) {
            Some(a) => a.clone(),
            None => return fallback,
        },
    };

    items
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let matched = arr.iter().find(|e| {
                e.get("id")
                    .or_else(|| e.get("bvid"))
                    .and_then(|b| b.as_str())
                    .map(|b| b == item.bvid)
                    .unwrap_or(false)
            });
            let expr = matched
                .and_then(|m| m.get("expression"))
                .and_then(|e| e.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if expr.chars().count() >= 10 {
                expr
            } else {
                fallback[idx].clone()
            }
        })
        .collect()
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::bilibili::VideoInfo;

    fn make_item(bvid: &str, score: f64, topic: &str, recommended: u32) -> ContentItem {
        let mut item = ContentItem::from_video(
            &VideoInfo {
                bvid: bvid.to_string(),
                title: format!("标题{}", bvid),
                description: String::new(),
                up_name: "up".to_string(),
                cover_url: String::new(),
                duration_secs: 300,
                view_count: 1000,
                like_count: 100,
                pubdate: 0,
                source: "popular".to_string(),
            },
            score,
            "匹配画像".to_string(),
            topic.to_string(),
        );
        item.recommended_count = recommended;
        item
    }

    #[test]
    fn test_pick_topic_dedup() {
        let items = vec![
            make_item("BV1", 0.9, "科技", 0),
            make_item("BV2", 0.85, "科技", 0),
            make_item("BV3", 0.7, "生活", 0),
        ];
        let recent: Vec<String> = vec![];
        let mut sorted: Vec<&ContentItem> = items.iter().collect();
        sorted.sort_by(|a, b| {
            let sa = effective_score(a, &recent);
            let sb = effective_score(b, &recent);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut picked = Vec::new();
        let mut topics = Vec::new();
        for item in sorted {
            if topics.contains(&item.topic_group) {
                continue;
            }
            topics.push(item.topic_group.clone());
            picked.push(item.bvid.clone());
        }
        assert_eq!(picked, vec!["BV1".to_string(), "BV3".to_string()]);
    }

    #[test]
    fn test_fatigue_penalty() {
        let a = make_item("BV1", 0.8, "科技", 0);
        let b = make_item("BV2", 0.8, "生活", 0);
        let recent = vec!["科技".to_string()];
        assert!(effective_score(&a, &recent) < effective_score(&b, &recent));
    }
}
