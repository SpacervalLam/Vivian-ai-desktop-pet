//! 内容发现与推荐 — 多平台内容采集聚合进 Vivian 既有链路
//!
//! 多平台源（sources/）：bilibili（匿名 WBI）、bangumi（匿名 v0 API）、v2ex（官方 API）
//! + browser_signals（登录态被动采集：桥接用户已登录页面同源 fetch 读取观看历史）。
//!
//! 不设独立后台循环，四个聚合点：
//! 1. **Busy 知识采集**（presence/background_tasks.rs）Share 路径接入
//!    `acquire_delight_candidates`：多平台候选与网页搜索合并竞争，最高分者经
//!    微信面板分享（复用 knowledge_share 30 分钟冷却）。
//! 2. **知识采集周期**顺带 `maintenance_pass`：探针 tick + 低库存跨平台补货 +
//!    登录态历史被动采集（6 小时冷却）。
//! 3. **内心独白兴趣搜索**与 **LLM 采集主题决定**消费 `interest_search_hints`
//!    （画像顶层兴趣 + 活跃探针域动态查询词）。
//! 4. **Bangumi 公开收藏导入**（`bootstrap_from_bangumi`）：公开用户名初始化画像。
//!
//! 数据按角色隔离：`characters/<char_id>/discovery/`（interest_profile.json /
//! content_store.json / speculative_state.json），全部原子写。

pub mod bilibili;
pub mod engine;
pub mod profile;
pub mod recommend;
pub mod sources;
pub mod speculator;
pub mod store;

use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::Value;
use tauri::AppHandle;

use crate::state::AppState;
use crate::types::response::ChatMessage;

use bilibili::BilibiliClient;
use profile::InterestProfile;
use speculator::InterestSpeculator;
use store::{ContentItem, ContentStore};

/// 全局 AppHandle（lib.rs setup 注入）
static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 全局 B 站客户端（WBI 密钥缓存共享）
pub static BILI_CLIENT: Lazy<BilibiliClient> = Lazy::new(BilibiliClient::new);

/// 库存低于此阈值时知识采集顺带补货
pub const DISCOVER_THRESHOLD: usize = 15;

/// 注入 AppHandle（lib.rs setup 调用一次）
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// LLM 直调（经 ModelRouter 的 chat 路由）；无路由 / 失败返回 None
pub async fn llm_complete(system: &str, user: &str, temperature: Option<f64>) -> Option<String> {
    let handle = APP_HANDLE.read().clone()?;
    use tauri::Manager;
    let state = handle.state::<Arc<AppState>>();
    let router = state.model_router.read().clone()?;

    let messages = vec![
        ChatMessage::system(system.to_string()),
        ChatMessage::user(user.to_string()),
    ];
    let mut req = crate::providers::base::LLMRequest::new("chat", messages);
    if let Some(t) = temperature {
        req = req.with_temperature(t);
    }
    match router.generate(req).await {
        Ok(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Err(e) => {
            tracing::debug!("[discovery] LLM 调用失败: {}", e);
            None
        }
    }
}

/// 容错 JSON 解析：剥离代码围栏后取首个完整 JSON 值（对象或数组）
pub fn parse_json_tolerant(content: &str) -> Option<Value> {
    let mut text = content.trim();
    // 剥离 ```json ... ``` 围栏
    if let Some(rest) = text.strip_prefix("```json") {
        text = rest.trim_start();
    } else if let Some(rest) = text.strip_prefix("```") {
        text = rest.trim_start();
    }
    if let Some(pos) = text.rfind("```") {
        text = text[..pos].trim_end();
    }

    // 直接解析
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        if v.is_object() || v.is_array() {
            return Some(v);
        }
    }
    // 截取首个 { 或 [ 起的片段再试
    let start = text.find(|c| c == '{' || c == '[')?;
    let candidate = &text[start..];
    if let Ok(v) = serde_json::from_str::<Value>(candidate) {
        if v.is_object() || v.is_array() {
            return Some(v);
        }
    }
    // 尾部截断容忍：从尾部找最近的闭合括号逐个尝试
    let bytes = candidate.as_bytes();
    let mut end = bytes.len();
    while end > 1 {
        end -= 1;
        if bytes[end] == b'}' || bytes[end] == b']' {
            if let Ok(v) = serde_json::from_str::<Value>(&candidate[..=end]) {
                if v.is_object() || v.is_array() {
                    return Some(v);
                }
            }
        }
    }
    None
}

// ============================================================================
// 聚合 API（Busy 知识采集 / 内心独白调用）
// ============================================================================

/// 兴趣搜索提示词：画像顶层兴趣 + 活跃探针域（供内心独白兴趣搜索与
/// LLM 采集主题决定作为动态锚点；画像为空时由调用方回退硬编码查询）
pub fn interest_search_hints(char_id: &str) -> Vec<String> {
    let profile = InterestProfile::load(char_id);
    let mut hints = profile.top_interest_names(4);
    let probe_domains: Vec<String> = InterestSpeculator::active_probes(char_id)
        .into_iter()
        .map(|p| p.domain)
        .collect();
    hints.extend(probe_domains);
    hints
}

/// 知识采集顺带维护：探针周期 + 低库存跨平台补货 + 登录态历史被动采集
/// 由 `run_knowledge_acquisition` 在主题循环前调用。
pub async fn maintenance_pass(char_id: &str) {
    // 1. 探针周期（生成被 6 小时间隔节流；纯逻辑阶段始终执行）
    let profile = InterestProfile::load(char_id);
    let tick = InterestSpeculator::tick(char_id, &profile).await;
    if !tick.promoted.is_empty() {
        let mut profile = profile;
        for spec in &tick.promoted {
            profile.upsert_interest(
                &spec.domain,
                (0.6 + spec.confidence * 0.3).min(1.0),
                "probe",
            );
        }
        profile.save(char_id);
        tracing::info!(
            "[discovery:{}] 探针升级 {} 个兴趣: {:?}",
            char_id,
            tick.promoted.len(),
            tick.promoted
                .iter()
                .map(|s| s.domain.clone())
                .collect::<Vec<_>>()
        );
    }

    // 2. 登录态被动采集：受控标签页恰好在 B 站时读取观看历史 → LLM 提炼兴趣（6 小时冷却）
    if history_collect_due(char_id) {
        let domains = sources::browser_signals::refresh_interests_from_history(char_id).await;
        if !domains.is_empty() {
            record_history_collect(char_id);
        }
    }

    // 3. 隔离任务 tab 自动发现：小红书/抖音/知乎后台静默采集（平台级 3 小时冷却 +
    //    登录态门槛；扩展未连接/未登录时静默跳过）
    sources::task_tabs::refresh_from_task_tabs(char_id).await;

    // 4. 低库存跨平台补货
    let store = ContentStore::load(char_id);
    if store.available_count() < DISCOVER_THRESHOLD {
        let mut profile = InterestProfile::load(char_id);
        let result = engine::discover_round(char_id, &BILI_CLIENT, &mut profile).await;
        profile.save(char_id);
        tracing::info!(
            "[discovery:{}] 跨平台库存补货：admitted={} delight={}",
            char_id,
            result.admitted,
            result.delight.len()
        );
    }
}

/// 被动历史采集冷却（秒）
const HISTORY_COLLECT_COOLDOWN_SECS: i64 = 6 * 3600;
static LAST_HISTORY_COLLECT: Lazy<RwLock<std::collections::HashMap<String, i64>>> =
    Lazy::new(|| RwLock::new(std::collections::HashMap::new()));

fn history_collect_due(char_id: &str) -> bool {
    let map = LAST_HISTORY_COLLECT.read();
    match map.get(char_id) {
        Some(last) => chrono::Utc::now().timestamp() - last >= HISTORY_COLLECT_COOLDOWN_SECS,
        None => true,
    }
}

fn record_history_collect(char_id: &str) {
    LAST_HISTORY_COLLECT
        .write()
        .insert(char_id.to_string(), chrono::Utc::now().timestamp());
}

/// 平台显示名（链接卡片 source 字段用）
pub fn platform_display_name(platform: &str) -> &str {
    match platform {
        "bilibili" => "Bilibili",
        "bangumi" => "Bangumi",
        "v2ex" => "V2EX",
        "zhihu" => "知乎",
        "xiaohongshu" => "小红书",
        "douyin" => "抖音",
        "weibo" => "微博",
        "reddit" => "Reddit",
        "twitter" => "X (Twitter)",
        _ => "Web",
    }
}

/// 多平台候选发现（知识采集 Share 路径的候选源）
///
/// 按分享主题驱动 B 站 + Bangumi 定向搜索，LLM 评估后返回惊喜级（≥0.75）候选，
/// 供与网页搜索结果竞争"本次分享哪条"。
pub async fn acquire_delight_candidates(
    char_id: &str,
    topic: &str,
) -> Vec<ContentItem> {
    let mut profile = InterestProfile::load(char_id);
    let candidates = engine::discover_for_topic(char_id, &BILI_CLIENT, &mut profile, topic).await;
    profile.save(char_id);
    candidates
        .into_iter()
        .filter(|c| c.score >= engine::DELIGHT_THRESHOLD)
        .collect()
}

/// Bangumi 公开收藏导入（画像初始化）
///
/// 拉取公开用户名的「看过/在看」条目，LLM 提炼兴趣域写回画像。
/// 返回提炼出的兴趣域；用户名无效/收藏为空/LLM 失败时返回空。
pub async fn bootstrap_from_bangumi(char_id: &str, username: &str) -> Vec<String> {
    let subjects = sources::bangumi::fetch_public_collections(username, 100).await;
    if subjects.is_empty() {
        return Vec::new();
    }

    let system = "你从用户的 Bangumi 公开收藏（看过/在看条目）中提炼兴趣领域。\
只输出能从条目名推断出的兴趣域（如「科幻动画」「TRPG 跑团」「本格推理」），\
不要输出「动画」「游戏」这类空泛词。输出严格 JSON，不要附带解释。";
    let lines: Vec<String> = subjects
        .iter()
        .take(100)
        .map(|s| format!("- {}", s.chars().take(30).collect::<String>()))
        .collect();
    let user = format!(
        "## Bangumi 收藏条目（{} 个）\n{}\n\n\
提炼 3-10 个兴趣域，按出现频度排序。\n\n\
严格输出 JSON：{{\"domains\":[\"...\",\"...\"]}}",
        subjects.len(),
        lines.join("\n"),
    );

    let Some(content) = llm_complete(system, &user, Some(0.4)).await else {
        return Vec::new();
    };
    let Some(value) = parse_json_tolerant(&content) else {
        return Vec::new();
    };
    let domains: Vec<String> = value
        .get("domains")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|d| d.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty() && s.chars().count() <= 30)
                .take(10)
                .collect()
        })
        .unwrap_or_default();
    if domains.is_empty() {
        return Vec::new();
    }

    let mut profile = InterestProfile::load(char_id);
    for (idx, domain) in domains.iter().enumerate() {
        let weight = (0.85 - idx as f64 * 0.05).max(0.5);
        profile.upsert_interest(domain, weight, "bangumi");
    }
    profile.save(char_id);
    tracing::info!(
        "[discovery:{}] Bangumi 收藏导入：{} 条目 → {} 个兴趣域: {:?}",
        char_id,
        subjects.len(),
        domains.len(),
        &domains
    );
    domains
}

/// 为分享候选生成朋友式推荐文案（单条；失败回退到评估理由）
pub async fn expression_for(char_id: &str, item: &ContentItem) -> String {
    let profile = InterestProfile::load(char_id);
    let expressions = recommend::generate_expressions(&profile, std::slice::from_ref(item)).await;
    expressions
        .first()
        .cloned()
        .unwrap_or_else(|| format!("「{}」，感觉你会喜欢", item.title))
}

// ============================================================================
// 工具面 API（discovery_tools 调用）
// ============================================================================

/// 给用户的推荐结果条目
pub struct RecommendationView {
    /// 平台 + 内容 ID 复合键（如 `bilibili:BV1xx`）
    pub bvid: String,
    pub platform: String,
    pub title: String,
    pub url: String,
    pub up_name: String,
    pub duration_secs: u64,
    pub topic_group: String,
    pub expression: String,
    pub score: f64,
}

/// 生成推荐（选取 + 文案 + 标记已推荐）
pub async fn recommend_for_user(
    char_id: &str,
    limit: usize,
) -> Result<Vec<RecommendationView>, String> {
    let mut picked = recommend::pick_recommendations(char_id, limit);
    if picked.is_empty() {
        // 库存不足时立即补一轮再取（同步等待，保证用户请求有响应）
        let mut profile = InterestProfile::load(char_id);
        let _ = engine::discover_round(char_id, &BILI_CLIENT, &mut profile).await;
        profile.save(char_id);
        picked = recommend::pick_recommendations(char_id, limit);
        if picked.is_empty() {
            return Err("暂无可用推荐（候选均与画像不匹配，稍后再试）".to_string());
        }
    }
    build_views(char_id, picked).await
}

async fn build_views(
    char_id: &str,
    picked: Vec<ContentItem>,
) -> Result<Vec<RecommendationView>, String> {
    let profile = InterestProfile::load(char_id);
    let expressions = recommend::generate_expressions(&profile, &picked).await;

    let mut store = ContentStore::load(char_id);
    let keys: Vec<String> = picked
        .iter()
        .map(|i| format!("{}:{}", i.platform, i.bvid))
        .collect();
    store.mark_recommended(&keys);
    store.save(char_id);

    let views = picked
        .iter()
        .zip(expressions.iter())
        .map(|(item, expr)| RecommendationView {
            bvid: format!("{}:{}", item.platform, item.bvid),
            platform: item.platform.clone(),
            title: item.title.clone(),
            url: item.url.clone(),
            up_name: item.up_name.clone(),
            duration_secs: item.duration_secs,
            topic_group: item.topic_group.clone(),
            expression: expr.clone(),
            score: item.score,
        })
        .collect();
    Ok(views)
}

/// 用户反馈闭环：like → 画像兴趣强化；dislike → 画像降权 + 主题入不喜欢
/// 返回 (是否命中, 应用说明)
pub fn apply_feedback(char_id: &str, target: &str, feedback: &str) -> (bool, String) {
    let feedback = feedback.trim().to_lowercase();
    if !matches!(feedback.as_str(), "like" | "dislike" | "neutral") {
        return (false, "feedback 必须是 like / dislike / neutral".to_string());
    }

    let mut store = ContentStore::load(char_id);
    let Some((bvid, topic_group)) = store.apply_feedback(target, &feedback) else {
        return (false, format!("库存中未找到目标内容: {}", target));
    };
    store.save(char_id);

    let mut profile = InterestProfile::load(char_id);
    match feedback.as_str() {
        "like" => {
            profile.upsert_interest(&topic_group, 0.75, "feedback");
            profile.exploration_openness = (profile.exploration_openness + 0.03).min(1.0);
        }
        "dislike" => {
            profile.decay_interest(&topic_group, 0.25);
            profile.add_dislike(&topic_group);
        }
        _ => {}
    }
    profile.save(char_id);
    tracing::info!(
        "[discovery:{}] 反馈 {} → {}（topic={}）",
        char_id,
        feedback,
        bvid,
        topic_group
    );
    (true, format!("已记录 {} 反馈，后续推荐会随之调整", feedback))
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json_tolerant_plain() {
        let v = parse_json_tolerant(r#"{"queries":["a","b"]}"#).unwrap();
        assert_eq!(v["queries"][0], "a");
    }

    #[test]
    fn test_parse_json_tolerant_fenced() {
        let v = parse_json_tolerant("```json\n{\"x\":1}\n```").unwrap();
        assert_eq!(v["x"], 1);
    }

    #[test]
    fn test_parse_json_tolerant_prefix_noise() {
        let v = parse_json_tolerant("好的，结果如下：\n{\"x\":1}").unwrap();
        assert_eq!(v["x"], 1);
    }

    #[test]
    fn test_parse_json_tolerant_array() {
        let v = parse_json_tolerant("[{\"bvid\":\"BV1\",\"score\":0.8}]").unwrap();
        assert!(v.is_array());
    }
}
