//! 内容发现画像命令 —— 用户画像页「兴趣画像」分区的前端接口。
//!
//! 七个命令：
//! - [`get_discovery_profile`]：兴趣画像 + 活跃探针 + 库存统计
//! - [`update_discovery_interest_weight`]：手动调整兴趣域权重
//! - [`remove_discovery_interest`]：删除兴趣域
//! - [`remove_discovery_dislike`]：移除不喜欢主题
//! - [`add_discovery_dislike`]：新增不喜欢主题
//! - [`respond_interest_probe`]：用户对兴趣猜测三态回应（confirm/reject/defer）
//! - [`bootstrap_from_bangumi`]：Bangumi 公开收藏导入（画像初始化）

use serde::{Deserialize, Serialize};

use crate::discovery::profile::InterestProfile;
use crate::discovery::speculator::{InterestSpeculator, SpeculativeInterest};
use crate::discovery::store::ContentStore;

/// 前端视图：兴趣域
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterestDomainView {
    pub domain: String,
    pub weight: f64,
    /// seed（画像种子）/ feedback（推荐反馈）/ probe（探针升级）
    pub source: String,
    pub evidence_count: u32,
    pub state: String,
}

/// 前端视图：兴趣探针
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeView {
    pub domain: String,
    pub category: String,
    pub reason: String,
    pub specifics: Vec<String>,
    /// near / lateral / bridge / wildcard
    pub probe_mode: String,
    pub confidence: f64,
    pub confirmation_count: u32,
    pub confirmation_threshold: u32,
}

/// 前端视图：完整兴趣画像（画像 + 探针 + 库存统计）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryProfileView {
    pub interests: Vec<InterestDomainView>,
    pub disliked_topics: Vec<String>,
    pub exploration_openness: f64,
    pub probes: Vec<ProbeView>,
    /// 可用库存（未推荐未反馈）
    pub store_available: usize,
    /// 库存总量
    pub store_total: usize,
    /// 已推荐累计
    pub store_recommended: usize,
}

fn probe_to_view(p: &SpeculativeInterest) -> ProbeView {
    ProbeView {
        domain: p.domain.clone(),
        category: p.category.clone(),
        reason: p.reason.clone(),
        specifics: p.specifics.clone(),
        probe_mode: p.probe_mode.clone(),
        confidence: p.confidence,
        confirmation_count: p.confirmation_count,
        confirmation_threshold: p.confirmation_threshold,
    }
}

/// 获取兴趣画像 + 探针 + 库存统计
#[tauri::command]
pub fn get_discovery_profile(character_id: String) -> Result<DiscoveryProfileView, String> {
    let profile = InterestProfile::load(&character_id);
    let probes = InterestSpeculator::active_probes(&character_id);
    let store = ContentStore::load(&character_id);

    Ok(DiscoveryProfileView {
        interests: profile
            .interests
            .iter()
            .filter(|i| i.state == "active")
            .map(|i| InterestDomainView {
                domain: i.domain.clone(),
                weight: i.weight,
                source: i.source.clone(),
                evidence_count: i.evidence_count,
                state: i.state.clone(),
            })
            .collect(),
        disliked_topics: profile.disliked_topics.clone(),
        exploration_openness: profile.exploration_openness,
        probes: probes.iter().map(probe_to_view).collect(),
        store_available: store.available_count(),
        store_total: store.items.len(),
        store_recommended: store.recommended_ledger.len(),
    })
}

/// 手动调整兴趣域权重（0-1）
#[tauri::command]
pub fn update_discovery_interest_weight(
    character_id: String,
    domain: String,
    weight: f64,
) -> Result<(), String> {
    if !(0.0..=1.0).contains(&weight) {
        return Err("weight 必须在 0-1 之间".to_string());
    }
    let mut profile = InterestProfile::load(&character_id);
    let Some(existing) = profile.interests.iter_mut().find(|i| i.domain == domain) else {
        return Err(format!("兴趣域不存在: {domain}"));
    };
    existing.weight = weight;
    if weight >= 0.15 && existing.state != "active" {
        existing.state = "active".to_string();
    }
    profile.save(&character_id);
    Ok(())
}

/// 删除兴趣域
#[tauri::command]
pub fn remove_discovery_interest(character_id: String, domain: String) -> Result<(), String> {
    let mut profile = InterestProfile::load(&character_id);
    let before = profile.interests.len();
    profile.interests.retain(|i| i.domain != domain);
    if profile.interests.len() == before {
        return Err(format!("兴趣域不存在: {domain}"));
    }
    profile.save(&character_id);
    Ok(())
}

/// 移除不喜欢主题
#[tauri::command]
pub fn remove_discovery_dislike(character_id: String, topic: String) -> Result<(), String> {
    let mut profile = InterestProfile::load(&character_id);
    let before = profile.disliked_topics.len();
    profile.disliked_topics.retain(|t| *t != topic);
    if profile.disliked_topics.len() == before {
        return Err(format!("主题不存在: {topic}"));
    }
    profile.save(&character_id);
    Ok(())
}

/// 新增不喜欢主题
#[tauri::command]
pub fn add_discovery_dislike(character_id: String, topic: String) -> Result<(), String> {
    let topic = topic.trim();
    if topic.is_empty() {
        return Err("主题不能为空".to_string());
    }
    let mut profile = InterestProfile::load(&character_id);
    profile.add_dislike(topic);
    profile.save(&character_id);
    Ok(())
}

/// 用户对兴趣猜测三态回应（与 answer_interest_probe 工具同语义）
#[tauri::command]
pub fn respond_interest_probe(
    character_id: String,
    domain: String,
    response: String,
) -> Result<String, String> {
    let response = response.trim().to_lowercase();
    if !matches!(response.as_str(), "confirm" | "reject" | "defer") {
        return Err("response 必须是 confirm / reject / defer".to_string());
    }

    let handled = match response.as_str() {
        "confirm" => {
            let ok = InterestSpeculator::user_confirm(&character_id, &domain);
            if ok {
                let mut profile = InterestProfile::load(&character_id);
                profile.upsert_interest(&domain, 0.85, "probe");
                profile.save(&character_id);
            }
            ok
        }
        "reject" => InterestSpeculator::user_reject(&character_id, &domain),
        _ => InterestSpeculator::user_defer(&character_id, &domain).is_some(),
    };
    if !handled {
        return Err(format!("活跃猜测中不存在: {domain}"));
    }
    Ok(response)
}

/// Bangumi 公开收藏导入（画像初始化）
///
/// 拉取公开用户名的「看过/在看」条目，LLM 提炼兴趣域写回画像。
#[tauri::command]
pub async fn bootstrap_from_bangumi(
    character_id: String,
    username: String,
) -> Result<Vec<String>, String> {
    let username = username.trim();
    if username.is_empty() {
        return Err("用户名不能为空".to_string());
    }
    let domains = crate::discovery::bootstrap_from_bangumi(&character_id, username).await;
    if domains.is_empty() {
        return Err("导入失败：用户名无效、收藏为空或网络不可用".to_string());
    }
    Ok(domains)
}
