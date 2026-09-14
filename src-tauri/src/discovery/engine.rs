//! 发现引擎 — 从用户画像出发跨平台主动搜寻内容
//!
//! 多源架构（sources/）：
//! - bilibili（匿名 WBI 搜索 + 热门）、bangumi（v0 搜索 + 榜单）、v2ex（官方 API）
//! - 每轮按画像搜索词驱动各源定向发现 + 热门兜底，跨源统一去重
//!
//! 流程（每轮）：
//! 1. LLM 从兴趣画像 + 活跃探针生成 3-5 个搜索词（探针域优先，验证猜测）
//! 2. 各源并行取候选（搜索 + 热门/榜单），库存与已推荐账本去重
//! 3. LLM 批量评估：结合画像打分（score/reason/topic_group），
//!    热门与否不影响评分——只看内容与用户画像的真实匹配度
//! 4. score ≥ 0.5 入库（admission 门槛）；高分 ≥ 0.75 触发惊喜队列
//! 5. 入库标题同时喂给兴趣探针做行为确认

use serde_json::{json, Value};

use super::bilibili::BilibiliClient;
use super::profile::InterestProfile;
use super::sources::bangumi::BangumiSource;
use super::sources::reddit::RedditSource;
use super::sources::v2ex::V2exSource;
use super::sources::weibo::WeiboSource;
use super::sources::x::XSource;
use super::sources::{ContentCandidate, SourceAdapter};
use super::speculator::{InterestSpeculator, SpeculativeState};
use super::store::{ContentItem, ContentStore};

/// admission 最低分
const ADMISSION_MIN_SCORE: f64 = 0.5;
/// 惊喜（主动分享）门槛
pub const DELIGHT_THRESHOLD: f64 = 0.75;
/// 库存上限
const MAX_STORE_ITEMS: usize = 60;
/// 每轮每搜索词各源取数
const SEARCH_PAGE_SIZE: usize = 20;
/// 单轮评估批量上限（控制 LLM 输入长度）
const EVALUATE_BATCH_SIZE: usize = 12;
/// V2EX 限频严格：每轮只取一次热门
const V2EX_POPULAR_LIMIT: usize = 15;
/// Bangumi 榜单每轮取数
const BANGUMI_POPULAR_LIMIT: usize = 15;
/// 微博热搜每轮取数
const WEIBO_POPULAR_LIMIT: usize = 15;
/// Reddit 匿名接口限频严格：每轮只取一次热门
const REDDIT_POPULAR_LIMIT: usize = 15;
/// X (twitter-cli) 每轮取数（cookie 重放，控制请求量）
const X_POPULAR_LIMIT: usize = 10;
/// 新鲜度窗口：发布时间超过该秒数的候选丢弃（pubdate 缺失为 0 的保留，
/// 搜索引擎结果常无时间戳）；兴趣发现场景取宽松的 30 天
const MAX_AGE_SECS: i64 = 30 * 24 * 3600;

/// 新鲜度过滤：丢弃发布时间过旧的候选
///
/// pubdate 缺失（0）的保留——V2EX/微博等源可能不携带时间。
pub fn filter_by_freshness(candidates: Vec<ContentCandidate>) -> Vec<ContentCandidate> {
    let cutoff = chrono::Utc::now().timestamp() - MAX_AGE_SECS;
    candidates
        .into_iter()
        .filter(|c| c.pubdate <= 0 || c.pubdate >= cutoff)
        .collect()
}

/// 评估窗口公平截断：定向搜索结果（source 以 "search:" 开头）保持原序优先，
/// 热门/榜单兜底按平台轮转补齐，避免尾部源（Reddit/X）被固定截断饿死
pub fn truncate_fairly(candidates: Vec<ContentCandidate>, limit: usize) -> Vec<ContentCandidate> {
    if candidates.len() <= limit {
        return candidates;
    }
    let mut search_hits: Vec<ContentCandidate> = Vec::new();
    let mut buckets: Vec<(String, std::collections::VecDeque<ContentCandidate>)> = Vec::new();
    for c in candidates {
        if c.source.starts_with("search:") {
            search_hits.push(c);
        } else {
            if let Some(b) = buckets.iter_mut().find(|(p, _)| *p == c.platform) {
                b.1.push_back(c);
            } else {
                buckets.push((c.platform.clone(), std::collections::VecDeque::from(vec![c])));
            }
        }
    }
    search_hits.truncate(limit);
    let mut out = search_hits;
    while out.len() < limit {
        let mut progressed = false;
        for bucket in &mut buckets {
            if let Some(c) = bucket.1.pop_front() {
                out.push(c);
                progressed = true;
                if out.len() >= limit {
                    break;
                }
            }
        }
        if !progressed {
            break;
        }
    }
    out
}

/// 单轮发现结果摘要
#[derive(Debug, Default, Clone)]
pub struct DiscoverResult {
    pub queries_used: Vec<String>,
    pub candidates_fetched: usize,
    pub admitted: usize,
    /// 达到惊喜门槛的条目
    pub delight: Vec<ContentItem>,
}

/// 执行一轮跨平台发现（发现 + 评估 + 入库 + 探针确认）
pub async fn discover_round(
    char_id: &str,
    client: &BilibiliClient,
    profile: &mut InterestProfile,
) -> DiscoverResult {
    let mut store = ContentStore::load(char_id);
    let spec_state = SpeculativeState::load(char_id);
    let mut result = DiscoverResult::default();

    // 1. 搜索词生成
    let queries = match generate_search_queries(profile, &spec_state).await {
        Some(q) if !q.is_empty() => q,
        _ => {
            // LLM 失败时回退：画像顶层兴趣（纯规则，不阻断发现）
            profile.top_interest_names(3)
        }
    };
    result.queries_used = queries.clone();

    // 2. 多源取候选：跨平台并行（单源慢/失败不拖累整轮，总延迟 = max 而非 sum）
    //    B 站各 query 搜索保持串行（WBI 风控考量），但 B 站块整体与其他平台并行
    let bili_queries = queries.clone();
    let (
        bili_search_results,
        bili_popular_results,
        bangumi_search_results,
        bangumi_popular_results,
        v2ex_results,
        weibo_results,
        reddit_results,
        x_results,
    ) = tokio::join!(
        async {
            let mut out = Vec::new();
            for query in &bili_queries {
                if let Ok(videos) = client.search(query, 1, SEARCH_PAGE_SIZE as u32).await {
                    out.extend(videos.iter().map(ContentCandidate::from_bilibili));
                }
            }
            out
        },
        async {
            client
                .popular(1, 20)
                .await
                .map(|videos| {
                    videos
                        .iter()
                        .map(ContentCandidate::from_bilibili)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        },
        async {
            // Bangumi 搜索只用前两个查询词（条目库语义与视频不同，控制请求量）
            let bangumi = BangumiSource;
            let mut out = Vec::new();
            for query in bili_queries.iter().take(2) {
                out.extend(bangumi.search(query, SEARCH_PAGE_SIZE).await);
            }
            out
        },
        async {
            let bangumi = BangumiSource;
            bangumi.popular(BANGUMI_POPULAR_LIMIT).await
        },
        async {
            let v2ex = V2exSource;
            v2ex.popular(V2EX_POPULAR_LIMIT).await
        },
        async {
            // 微博：游客态热搜 + 首个查询词定向搜索（visitor SUB 缓存于实例内）
            let weibo = WeiboSource::new();
            let mut out = weibo.popular(WEIBO_POPULAR_LIMIT).await;
            if let Some(q) = bili_queries.first() {
                out.extend(weibo.search(q, SEARCH_PAGE_SIZE).await);
            }
            out
        },
        async {
            // Reddit：rdt-cli 优先（未安装/失败回退匿名 JSON），仅每轮一次热门
            let reddit = RedditSource;
            reddit.popular(REDDIT_POPULAR_LIMIT).await
        },
        async {
            // X (Twitter)：twitter-cli cookie 重放（cookie 缺失或 CLI 未安装时静默跳过）
            let x = XSource;
            let mut out = x.popular(X_POPULAR_LIMIT).await;
            if let Some(q) = bili_queries.first() {
                out.extend(x.search(q, X_POPULAR_LIMIT).await);
            }
            out
        }
    );

    let mut candidates: Vec<ContentCandidate> = Vec::new();
    candidates.extend(bili_search_results);
    candidates.extend(bili_popular_results);
    candidates.extend(bangumi_search_results);
    candidates.extend(bangumi_popular_results);
    candidates.extend(v2ex_results);
    candidates.extend(weibo_results);
    candidates.extend(reddit_results);
    candidates.extend(x_results);

    result.candidates_fetched = candidates.len();

    // 跨源去重（平台 + 内容 ID；V2EX latest 与 hot 可能重复）
    // → 新鲜度过滤（丢弃过旧候选）→ 公平截断（定向优先 + 热门轮转）
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|c| {
        !c.content_id.is_empty()
            && seen.insert(c.key())
            && !store.contains_id(&c.platform, &c.content_id)
    });
    let candidates = filter_by_freshness(candidates);
    let candidates = truncate_fairly(candidates, EVALUATE_BATCH_SIZE);
    if candidates.is_empty() {
        return result;
    }

    // 3. LLM 批量评估
    let Some(evaluations) = evaluate_batch(profile, &candidates).await else {
        return result;
    };

    // 4. 入库 + 5. 探针确认
    let mut titles_for_probe = Vec::new();
    for (candidate, eval) in candidates.iter().zip(evaluations.iter()) {
        if eval.0 < ADMISSION_MIN_SCORE {
            continue;
        }
        let item = ContentItem::from_candidate(candidate, eval.0, eval.1.clone(), eval.2.clone());
        titles_for_probe.push(candidate.title.clone());
        if eval.0 >= DELIGHT_THRESHOLD {
            result.delight.push(item.clone());
        }
        store.admit(item, MAX_STORE_ITEMS);
        result.admitted += 1;
    }
    store.save(char_id);

    // 探针行为确认：仅对入库（高分）条目计证据，避免低分内容污染
    if !titles_for_probe.is_empty() {
        InterestSpeculator::observe(char_id, &titles_for_probe);
    }

    tracing::info!(
        "[discovery:{}] 跨平台发现完成：queries={:?} fetched={} admitted={} delight={}",
        char_id,
        result.queries_used,
        result.candidates_fetched,
        result.admitted,
        result.delight.len()
    );
    result
}

/// 外部采集候选统一入库：跨源去重 → LLM 评估 → 入库 + 探针确认
///
/// 供隔离任务 tab 等引擎外采集路径复用（采集本身无 LLM，入库评估与
/// `discover_round` 同一套准入门槛）。返回入库条数。
pub async fn admit_candidates(char_id: &str, candidates: Vec<ContentCandidate>) -> usize {
    let mut store = ContentStore::load(char_id);
    let mut seen = std::collections::HashSet::new();
    let fresh: Vec<ContentCandidate> = candidates
        .into_iter()
        .filter(|c| {
            !c.content_id.is_empty()
                && seen.insert(c.key())
                && !store.contains_id(&c.platform, &c.content_id)
        })
        .collect();
    let fresh = truncate_fairly(filter_by_freshness(fresh), EVALUATE_BATCH_SIZE);
    if fresh.is_empty() {
        return 0;
    }

    let profile = InterestProfile::load(char_id);
    let Some(evaluations) = evaluate_batch(&profile, &fresh).await else {
        return 0;
    };

    let mut titles_for_probe = Vec::new();
    let mut admitted = 0;
    for (candidate, eval) in fresh.iter().zip(evaluations.iter()) {
        if eval.0 < ADMISSION_MIN_SCORE {
            continue;
        }
        let item = ContentItem::from_candidate(candidate, eval.0, eval.1.clone(), eval.2.clone());
        titles_for_probe.push(candidate.title.clone());
        store.admit(item, MAX_STORE_ITEMS);
        admitted += 1;
    }
    store.save(char_id);

    if !titles_for_probe.is_empty() {
        InterestSpeculator::observe(char_id, &titles_for_probe);
    }
    admitted
}

/// LLM 生成搜索词（画像兴趣 + 探针域，探针优先验证）
async fn generate_search_queries(
    profile: &InterestProfile,
    spec_state: &SpeculativeState,
) -> Option<Vec<String>> {
    let probe_domains: Vec<String> = spec_state
        .active
        .iter()
        .filter(|s| s.status == "active")
        .map(|s| s.domain.clone())
        .collect();

    let system = "你是多平台内容搜索词策划（覆盖 B 站视频、Bangumi 动画/书籍/游戏条目、V2EX 技术社区）。\
基于用户兴趣画像和待验证的兴趣猜测，生成精准的中文搜索词。\
搜索词要具体到能搜到优质内容（如「参数化设计 建筑美学」优于「建筑」）。输出严格 JSON。";

    let user = format!(
        "{}\n\n## 待验证的兴趣猜测（优先为这些方向生成搜索词，验证用户是否真的感兴趣）\n{}\n\n\
## 要求\n- 生成 4-5 个搜索词，其中至少 1-2 个来自待验证猜测，其余来自画像兴趣的**新颖切面**（不要每次都搜同样的词）\n\
- 每个词 4-15 字，具体、可搜\n\n严格输出 JSON：{{\"queries\":[\"...\",\"...\"]}}",
        profile.to_prompt_context(),
        if probe_domains.is_empty() {
            "（暂无活跃猜测）".to_string()
        } else {
            probe_domains.join("、")
        },
    );

    let content = super::llm_complete(system, &user, Some(0.8)).await?;
    let value = super::parse_json_tolerant(&content)?;
    let queries: Vec<String> = value
        .get("queries")
        .and_then(|q| q.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|q| q.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty() && s.chars().count() <= 30)
                .take(5)
                .collect()
        })
        .unwrap_or_default();
    if queries.is_empty() {
        None
    } else {
        Some(queries)
    }
}

/// 按分享主题定向发现（知识采集 Share 路径的多平台候选源）
///
/// 以主题为搜索词驱动 B 站 + Bangumi 定向搜索，各源热门兜底，
/// LLM 评估后入库并返回高分条目（含未达惊喜门槛的入库项，调用方按需筛选）。
pub async fn discover_for_topic(
    char_id: &str,
    client: &BilibiliClient,
    profile: &mut InterestProfile,
    topic: &str,
) -> Vec<ContentItem> {
    let mut store = ContentStore::load(char_id);
    let mut candidates: Vec<ContentCandidate> = Vec::new();

    if let Ok(videos) = client.search(topic, 1, SEARCH_PAGE_SIZE as u32).await {
        candidates.extend(videos.iter().map(ContentCandidate::from_bilibili));
    }
    if let Ok(videos) = client.popular(1, 20).await {
        candidates.extend(videos.iter().map(ContentCandidate::from_bilibili));
    }
    let bangumi = BangumiSource;
    candidates.extend(bangumi.search(topic, SEARCH_PAGE_SIZE).await);

    let mut seen = std::collections::HashSet::new();
    candidates.retain(|c| {
        !c.content_id.is_empty()
            && seen.insert(c.key())
            && !store.contains_id(&c.platform, &c.content_id)
    });
    let candidates = truncate_fairly(filter_by_freshness(candidates), EVALUATE_BATCH_SIZE);
    if candidates.is_empty() {
        return Vec::new();
    }

    let Some(evaluations) = evaluate_batch(profile, &candidates).await else {
        return Vec::new();
    };

    let mut titles_for_probe = Vec::new();
    let mut scored: Vec<ContentItem> = Vec::new();
    for (candidate, eval) in candidates.iter().zip(evaluations.iter()) {
        if eval.0 < ADMISSION_MIN_SCORE {
            continue;
        }
        let item = ContentItem::from_candidate(candidate, eval.0, eval.1.clone(), eval.2.clone());
        titles_for_probe.push(candidate.title.clone());
        scored.push(item.clone());
        store.admit(item, MAX_STORE_ITEMS);
    }
    store.save(char_id);

    if !titles_for_probe.is_empty() {
        InterestSpeculator::observe(char_id, &titles_for_probe);
    }
    scored
}

/// LLM 批量评估候选：返回与候选一一对应的 (score, reason, topic_group)
/// 评分标准只看内容与画像的真实匹配度，热门/来源/平台不加分。
///
/// 画像兴趣词对候选标题/描述做本地预匹配，命中词随 items 注入——
/// 为 LLM 提供字面证据锚定：命中条目按命中词评估，未命中条目要求
/// 语义层面确认真实关联，抑制"泛领域沾边给高分"。
async fn evaluate_batch(
    profile: &InterestProfile,
    candidates: &[ContentCandidate],
) -> Option<Vec<(f64, String, String)>> {
    let system = "你要批量评估多个候选内容与一个用户画像的匹配度（候选来自 B 站/Bangumi/V2EX 等不同平台）。\
输出严格 JSON，不要附带解释。";

    // 本地预匹配：画像顶层兴趣词（≥2 字）对 title+description 做包含匹配
    let interest_terms: Vec<String> = profile
        .top_interest_names(10)
        .into_iter()
        .filter(|t| t.chars().count() >= 2)
        .collect();

    let items: Vec<Value> = candidates
        .iter()
        .map(|c| {
            let desc: String = if c.description.chars().count() > 120 {
                c.description.chars().take(120).collect()
            } else {
                c.description.clone()
            };
            let haystack = format!("{} {}", c.title, c.description).to_lowercase();
            let matched: Vec<&str> = interest_terms
                .iter()
                .filter(|t| haystack.contains(&t.to_lowercase()))
                .map(|t| t.as_str())
                .collect();
            json!({
                "id": c.content_id,
                "platform": c.platform,
                "title": c.title,
                "description": desc,
                "author": c.author,
                "url": c.url,
                "duration_secs": c.duration_secs,
                "view_count": c.view_count,
                "like_count": c.like_count,
                "matched_keywords": matched,
            })
        })
        .collect();

    let user = format!(
        "{}\n\n## 评分规则\n\
1. score 为 0-1，只衡量内容与用户画像的真实匹配度及内容价值；不得因为是热门、来自推荐流、来自某个平台就加分，明显不匹配的必须低分\n\
2. 不同平台（视频/条目/帖子）同 schema 统一评分，不因平台类型调整标准\n\
3. 每个候选带 matched_keywords 字段（本地预匹配的画像兴趣词命中结果）：命中非空时该候选与画像存在字面证据，按命中词与内容的实际关联评分；命中为空时无字面证据，必须从语义层面确认内容与画像兴趣的真实关联——仅仅同属一个大领域而缺少实质关联的，score 应低于 0.4\n\
4. 若画像中的兴趣属于猜测验证方向，主题可以陌生，但内容仍需具体可信\n\
5. reason：score ≥ 0.5 的条目写一句不超过 30 字的中文匹配依据；score < 0.5 的条目 reason 写空串 \"\"\n\
6. topic_group：2-4 个中文词的粗分类，同主题不同切面统一用同一个词\n\
7. results 数组长度与输入一致，顺序一一对应，每项原样带回 id\n\n\
严格输出 JSON：{{\"results\":[{{\"id\":\"...\",\"score\":0.78,\"reason\":\"...\",\"topic_group\":\"...\"}}]}}\n\n\
## 候选内容\n{}",
        profile.to_prompt_context(),
        serde_json::to_string(&items).unwrap_or_default(),
    );

    let content = super::llm_complete(system, &user, None).await?;
    let value = super::parse_json_tolerant(&content)?;
    let results = value.get("results")?.as_array()?.clone();

    // 按输入顺序对齐（LLM 可能乱序；按 id 匹配）
    let mut evaluations: Vec<(f64, String, String)> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let matched = results.iter().find(|r| {
            r.get("id")
                .and_then(|b| b.as_str())
                .map(|b| b == candidate.content_id)
                .unwrap_or(false)
        });
        if let Some(m) = matched {
            let score = m.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0);
            let reason = m
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let topic = m
                .get("topic_group")
                .and_then(|t| t.as_str())
                .unwrap_or("未分类")
                .trim()
                .to_string();
            evaluations.push((
                score.clamp(0.0, 1.0),
                reason,
                if topic.is_empty() { "未分类".to_string() } else { topic },
            ));
        } else {
            evaluations.push((0.0, String::new(), "未分类".to_string()));
        }
    }
    Some(evaluations)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(platform: &str, id: &str, source: &str, pubdate: i64) -> ContentCandidate {
        ContentCandidate {
            platform: platform.to_string(),
            content_id: id.to_string(),
            title: format!("标题{id}"),
            description: String::new(),
            author: String::new(),
            url: String::new(),
            cover_url: String::new(),
            duration_secs: 0,
            view_count: 0,
            like_count: 0,
            pubdate,
            source: source.to_string(),
        }
    }

    #[test]
    fn test_filter_by_freshness() {
        let now = chrono::Utc::now().timestamp();
        let fresh = candidate("bilibili", "a", "hot", now - 3600);
        let stale = candidate("bilibili", "b", "hot", now - MAX_AGE_SECS - 60);
        let unknown = candidate("v2ex", "c", "hot", 0);
        let out = filter_by_freshness(vec![fresh.clone(), stale, unknown.clone()]);
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|c| c.content_id == "a"));
        assert!(out.iter().any(|c| c.content_id == "c"));
    }

    #[test]
    fn test_truncate_fairly_round_robin() {
        // B 站热门 5 条 + 尾部源各 2 条；截断到 6：B 站 1 条（热门）+ 两个尾部源轮转
        let mut input = Vec::new();
        for i in 0..5 {
            input.push(candidate("bilibili", &format!("b{i}"), "hot", 0));
        }
        for i in 0..2 {
            input.push(candidate("reddit", &format!("r{i}"), "hot", 0));
        }
        for i in 0..2 {
            input.push(candidate("x", &format!("x{i}"), "hot", 0));
        }
        let out = truncate_fairly(input, 6);
        assert_eq!(out.len(), 6);
        // 尾部源不再被饿死：reddit 与 x 至少各 1 条
        assert!(out.iter().any(|c| c.platform == "reddit"));
        assert!(out.iter().any(|c| c.platform == "x"));
    }

    #[test]
    fn test_truncate_fairly_search_priority() {
        // 定向搜索结果优先于热门兜底
        let mut input = vec![candidate("bilibili", "hot1", "hot", 0)];
        for i in 0..4 {
            input.push(candidate("reddit", &format!("s{i}"), "search:测试", 0));
        }
        let out = truncate_fairly(input, 3);
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|c| c.source.starts_with("search:")));
    }

    #[test]
    fn test_truncate_fairly_under_limit() {
        let input = vec![candidate("bilibili", "a", "hot", 0), candidate("v2ex", "b", "hot", 0)];
        let out = truncate_fairly(input, 10);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn test_parse_queries() {
        let json = r#"{"queries":["机械键盘 客制化","参数化设计 建筑"]}"#;
        let value = crate::discovery::parse_json_tolerant(json).unwrap();
        let queries: Vec<String> = value
            .get("queries")
            .and_then(|q| q.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|q| q.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(queries.len(), 2);
    }
}
