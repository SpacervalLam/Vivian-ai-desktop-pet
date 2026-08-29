//! 兴趣探针 — 主动猜测用户可能感兴趣的未知领域
//!
//! 生命周期：Generate → Active → Promote（确认达阈值）/ Reject + Cooldown（TTL 过期）
//! 用户四态反馈：confirm（立即升级）/ reject（30 天冷却）/ defer（7/14 天递进，3 次耗尽进冷却）
//! 行为确认：发现的内容标题/描述与探针域做中文 bigram + 关键词匹配，命中计 1 证据。
//!
//! 探针模式：near（贴近现有兴趣的相邻领域）/ lateral（横向迁移）/
//! bridge（心理学桥接）/ wildcard（大胆猜测）——near 与挑战类分池配额。
//!
//! 存储路径：`<用户数据目录>/characters/<char_id>/discovery/speculative_state.json`

use std::collections::HashSet;
use std::path::PathBuf;

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::utils::path::get_character_data_dir;

use super::profile::InterestProfile;

/// near 池上限
const MAX_ACTIVE_NEAR: usize = 5;
/// 挑战池（lateral/bridge/wildcard）上限
const MAX_ACTIVE_CHALLENGE: usize = 3;
/// 默认 TTL（天）：过期未确认即 reject
const DEFAULT_TTL_DAYS: i64 = 14;
/// reject 冷却（天）
const COOLDOWN_DAYS: i64 = 30;
/// promote 证据阈值
const CONFIRMATION_THRESHOLD: u32 = 3;
/// LLM 自评置信度下限（低于丢弃）
const MIN_CONFIDENCE: f64 = 0.30;
/// defer 递进窗口（天）；第 3 次 defer 耗尽进冷却
const DEFER_DAYS: &[i64] = &[7, 14];
const MAX_DEFERS: u32 = 3;

/// 单个猜测兴趣
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeculativeInterest {
    pub domain: String,
    pub category: String,
    /// 猜测理由（面向用户解释「为什么猜你喜欢这个」）
    pub reason: String,
    /// 具体细分话题（≥2 个才有效）
    pub specifics: Vec<String>,
    /// 探针模式：near / lateral / bridge / wildcard
    pub probe_mode: String,
    pub confidence: f64,
    pub created_at: String,
    pub ttl_days: i64,
    pub confirmation_count: u32,
    pub confirmation_threshold: u32,
    /// 状态：active / confirmed / promoted / rejected / deferred
    pub status: String,
    /// 确认证据（命中的内容标题片段）
    pub confirming_events: Vec<String>,
    pub defer_count: u32,
    pub deferred_until: String,
}

impl SpeculativeInterest {
    pub fn is_challenge(&self) -> bool {
        matches!(self.probe_mode.as_str(), "lateral" | "bridge" | "wildcard")
    }
}

/// 被拒绝的探针（冷却期内不再重新猜测）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CooldownEntry {
    pub domain: String,
    pub cooldown_until: String,
}

/// 探针状态容器
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SpeculativeState {
    pub active: Vec<SpeculativeInterest>,
    pub cooldown: Vec<CooldownEntry>,
    pub last_generation_at: String,
    pub total_promoted: u32,
    pub total_rejected: u32,
}

impl SpeculativeState {
    fn state_path(char_id: &str) -> PathBuf {
        get_character_data_dir(char_id)
            .join("discovery")
            .join("speculative_state.json")
    }

    pub fn load(char_id: &str) -> Self {
        let path = Self::state_path(char_id);
        crate::utils::fs::load_json_or_backup(&path).unwrap_or_default()
    }

    pub fn save(&self, char_id: &str) {
        let path = Self::state_path(char_id);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("json.tmp");
        if let Ok(json) = serde_json::to_string_pretty(self) {
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    /// 活跃探针的 near / 挑战池计数
    fn slot_counts(&self) -> (usize, usize) {
        let active: Vec<&SpeculativeInterest> =
            self.active.iter().filter(|s| s.status == "active").collect();
        let near = active.iter().filter(|s| !s.is_challenge()).count();
        let challenge = active.iter().filter(|s| s.is_challenge()).count();
        (near, challenge)
    }

    /// 指定模式是否还有空槽
    fn slot_available(&self, is_challenge: bool) -> bool {
        let (near, challenge) = self.slot_counts();
        if is_challenge {
            challenge < MAX_ACTIVE_CHALLENGE
        } else {
            near < MAX_ACTIVE_NEAR
        }
    }
}

/// 探针 tick 结果
#[derive(Debug, Default, Clone)]
pub struct TickResult {
    pub promoted: Vec<SpeculativeInterest>,
    pub rejected: Vec<SpeculativeInterest>,
    pub generated: Vec<SpeculativeInterest>,
}

/// 兴趣探针管理器
pub struct InterestSpeculator;

impl InterestSpeculator {
    /// 周期入口：expire → promote → revive →（必要时）generate
    /// LLM 失败时静默跳过生成，纯逻辑阶段仍然执行。
    pub async fn tick(char_id: &str, profile: &InterestProfile) -> TickResult {
        let state = SpeculativeState::load(char_id);
        let now = Utc::now();
        let mut result = TickResult::default();

        // 1. 过期清理
        let (rejected, state) = expire_stale(state, now);
        result.rejected = rejected;

        // 2. 升级达阈值探针
        let (promoted, mut state) = promote_ready(state);
        result.promoted = promoted;

        // 3. 恢复 defer 到期的探针（最后执行，避免同轮升级）
        state = revive_deferred(state, now);

        // 4. 生成新探针（间隔 6 小时 + 至少 2 个空槽才值得 LLM 调用）
        let (near, challenge) = state.slot_counts();
        let free_slots = (MAX_ACTIVE_NEAR.saturating_sub(near))
            + (MAX_ACTIVE_CHALLENGE.saturating_sub(challenge));
        let interval_ok = state
            .last_generation_at
            .parse::<chrono::DateTime<Utc>>()
            .map(|last| now - last > Duration::hours(6))
            .unwrap_or(true);
        if free_slots >= 2 && interval_ok {
            if let Some(candidates) = generate_candidates(&state, profile).await {
                let generated = merge_candidates(&mut state, candidates, profile);
                result.generated = generated;
            }
            state.last_generation_at = now.to_rfc3339();
        }

        state.save(char_id);
        result
    }

    /// 用发现的内容标题/描述确认活跃探针（bigram/关键词匹配，无 LLM）
    /// 返回被命中的探针域列表（供画像 upsert 证据）。
    pub fn observe(char_id: &str, titles: &[String]) -> Vec<String> {
        let mut state = SpeculativeState::load(char_id);
        let mut hit_domains = Vec::new();
        let texts: Vec<String> = titles.iter().map(|t| t.to_lowercase()).collect();

        for spec in state.active.iter_mut() {
            if spec.status != "active" {
                continue;
            }
            let mut hit = false;
            for text in &texts {
                if text_matches_speculation(text, &spec.domain, &spec.category) {
                    hit = true;
                    break;
                }
            }
            if hit {
                spec.confirmation_count += 1;
                if let Some(t) = titles.first() {
                    let short: String = t.chars().take(50).collect();
                    spec.confirming_events.push(short);
                }
                hit_domains.push(spec.domain.clone());
            }
        }
        if !hit_domains.is_empty() {
            state.save(char_id);
        }
        hit_domains
    }

    /// 用户显式确认 → 立即标记 confirmed（下轮 tick promote 为正式兴趣）
    pub fn user_confirm(char_id: &str, domain: &str) -> bool {
        let mut state = SpeculativeState::load(char_id);
        let now = Utc::now().to_rfc3339();
        let mut found = false;
        for spec in state.active.iter_mut() {
            if spec.domain.eq_ignore_ascii_case(domain) && spec.status == "active" {
                spec.confirmation_count = spec.confirmation_threshold;
                spec.status = "confirmed".to_string();
                spec.created_at = now; // 重置 TTL 窗口，避免下轮被过期清理抢先
                found = true;
                break;
            }
        }
        if found {
            state.save(char_id);
        }
        found
    }

    /// 用户显式拒绝 → 30 天冷却
    pub fn user_reject(char_id: &str, domain: &str) -> bool {
        let mut state = SpeculativeState::load(char_id);
        let now = Utc::now();
        let mut found = false;
        state.active.retain(|spec| {
            if spec.domain.eq_ignore_ascii_case(domain) && spec.status == "active" {
                state.total_rejected += 1;
                state.cooldown.push(CooldownEntry {
                    domain: spec.domain.clone(),
                    cooldown_until: (now + Duration::days(COOLDOWN_DAYS)).to_rfc3339(),
                });
                found = true;
                false
            } else {
                true
            }
        });
        if found {
            state.save(char_id);
        }
        found
    }

    /// 用户暂时忽略 → 递进 snooze；第 MAX_DEFERS 次耗尽进冷却
    /// 返回 Some(结果说明) 表示找到了目标探针
    pub fn user_defer(char_id: &str, domain: &str) -> Option<String> {
        let mut state = SpeculativeState::load(char_id);
        let now = Utc::now();
        let mut outcome: Option<String> = None;
        let mut retained = Vec::new();
        for mut spec in state.active.drain(..) {
            if spec.domain.eq_ignore_ascii_case(domain) && spec.status == "active" {
                spec.defer_count += 1;
                if spec.defer_count >= MAX_DEFERS {
                    state.total_rejected += 1;
                    state.cooldown.push(CooldownEntry {
                        domain: spec.domain.clone(),
                        cooldown_until: (now + Duration::days(COOLDOWN_DAYS)).to_rfc3339(),
                    });
                    outcome = Some("exhausted".to_string());
                } else {
                    let window =
                        DEFER_DAYS[(spec.defer_count as usize - 1).min(DEFER_DAYS.len() - 1)];
                    spec.status = "deferred".to_string();
                    spec.deferred_until = (now + Duration::days(window)).to_rfc3339();
                    outcome = Some(format!("deferred:{}", window));
                    retained.push(spec);
                }
            } else {
                retained.push(spec);
            }
        }
        state.active = retained;
        if outcome.is_some() {
            state.save(char_id);
        }
        outcome
    }

    /// 当前应向用户展示的活跃探针（低证据优先，让新方向先浮出）
    pub fn next_probe(char_id: &str) -> Option<SpeculativeInterest> {
        let state = SpeculativeState::load(char_id);
        state
            .active
            .iter()
            .filter(|s| s.status == "active")
            .min_by_key(|s| s.confirmation_count)
            .cloned()
    }

    /// 所有活跃探针（供 get_interest_probes 工具列出）
    pub fn active_probes(char_id: &str) -> Vec<SpeculativeInterest> {
        SpeculativeState::load(char_id)
            .active
            .into_iter()
            .filter(|s| s.status == "active")
            .collect()
    }
}

// ============================================================================
// 纯逻辑生命周期函数
// ============================================================================

/// TTL 过期 → reject + cooldown；同时清理过期 cooldown
fn expire_stale(
    mut state: SpeculativeState,
    now: chrono::DateTime<Utc>,
) -> (Vec<SpeculativeInterest>, SpeculativeState) {
    let mut rejected = Vec::new();
    let mut retained = Vec::new();
    for spec in state.active.drain(..) {
        if spec.status != "active" {
            retained.push(spec);
            continue;
        }
        let expired = spec
            .created_at
            .parse::<chrono::DateTime<Utc>>()
            .map(|created| now > created + Duration::days(spec.ttl_days))
            .unwrap_or(false);
        if expired {
            state.total_rejected += 1;
            state.cooldown.push(CooldownEntry {
                domain: spec.domain.clone(),
                cooldown_until: (now + Duration::days(COOLDOWN_DAYS)).to_rfc3339(),
            });
            rejected.push(spec);
        } else {
            retained.push(spec);
        }
    }
    state.active = retained;
    // 清理过期冷却
    state.cooldown.retain(|c| {
        c.cooldown_until
            .parse::<chrono::DateTime<Utc>>()
            .map(|until| now <= until)
            .unwrap_or(false)
    });
    (rejected, state)
}

/// 升级：行为确认达阈值（active）或用户已确认（confirmed）→ promoted
fn promote_ready(mut state: SpeculativeState) -> (Vec<SpeculativeInterest>, SpeculativeState) {
    let mut promoted = Vec::new();
    let mut retained = Vec::new();
    for spec in state.active.drain(..) {
        let ready = (spec.status == "active"
            && spec.confirmation_count >= spec.confirmation_threshold)
            || spec.status == "confirmed";
        if ready {
            state.total_promoted += 1;
            promoted.push(spec);
        } else {
            retained.push(spec);
        }
    }
    state.active = retained;
    (promoted, state)
}

/// 恢复 defer 到期的探针（重置 TTL 窗口；阈值附近证据钳到阈值-1 强制重新浮出）
fn revive_deferred(mut state: SpeculativeState, now: chrono::DateTime<Utc>) -> SpeculativeState {
    for spec in state.active.iter_mut() {
        if spec.status != "deferred" {
            continue;
        }
        let due = spec
            .deferred_until
            .parse::<chrono::DateTime<Utc>>()
            .map(|until| now >= until)
            .unwrap_or(false);
        if due {
            spec.status = "active".to_string();
            spec.created_at = now.to_rfc3339();
            spec.deferred_until = String::new();
            if spec.confirmation_count >= spec.confirmation_threshold {
                spec.confirmation_count = spec.confirmation_threshold.saturating_sub(1);
            }
        }
    }
    state
}

// ============================================================================
// 匹配（无 LLM）
// ============================================================================

/// 提取中文连续串的 bigram 集合
fn chinese_bigrams(text: &str) -> HashSet<String> {
    let mut bigrams = HashSet::new();
    for run in split_chinese_runs(text) {
        let chars: Vec<char> = run.chars().collect();
        if chars.len() < 2 {
            continue;
        }
        for i in 0..chars.len() - 1 {
            bigrams.insert(format!("{}{}", chars[i], chars[i + 1]));
        }
    }
    bigrams
}

/// 按非中文字符切出中文连续串
fn split_chinese_runs(text: &str) -> Vec<String> {
    let mut runs = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if ('\u{4e00}'..='\u{9fff}').contains(&c) {
            current.push(c);
        } else if !current.is_empty() {
            runs.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

/// 事件文本与探针域的匹配：子串 / 关键词切分 / bigram 重叠（≥2 且域 bigram ≥4）
fn text_matches_speculation(event_text: &str, domain: &str, category: &str) -> bool {
    let domain_lower = domain.to_lowercase();
    if !domain_lower.is_empty() && event_text.contains(&domain_lower) {
        return true;
    }
    let category_lower = category.to_lowercase();
    if category_lower.chars().count() >= 2 && event_text.contains(&category_lower) {
        return true;
    }
    // 中文关键词切分匹配（按连词/顿号/空格）
    for kw in split_keywords(&domain_lower) {
        if kw.chars().count() >= 2 && event_text.contains(&kw) {
            return true;
        }
    }
    // bigram 兜底：长复合词（无分隔符）场景
    let mut all_bigrams = chinese_bigrams(&domain_lower);
    all_bigrams.extend(chinese_bigrams(&category_lower));
    if all_bigrams.len() < 4 {
        return false;
    }
    let event_bigrams = chinese_bigrams(event_text);
    all_bigrams.intersection(&event_bigrams).count() >= 2
}

/// 按常见分隔符切分关键词
fn split_keywords(text: &str) -> Vec<String> {
    text.split(|c: char| matches!(c, '与' | '和' | '·' | '、' | '/' | ' ' | '及'))
        .map(|p| p.trim().to_string())
        .filter(|p| p.chars().count() >= 2)
        .collect()
}

// ============================================================================
// LLM 生成
// ============================================================================

/// LLM 生成猜测兴趣候选（失败返回 None）
async fn generate_candidates(
    state: &SpeculativeState,
    profile: &InterestProfile,
) -> Option<Vec<SpeculativeInterest>> {
    let existing: Vec<String> = state
        .active
        .iter()
        .map(|s| s.domain.clone())
        .chain(state.cooldown.iter().map(|c| c.domain.clone()))
        .collect();
    let confirmed = profile.top_interest_names(12);
    let (near, challenge) = state.slot_counts();
    let near_slots = MAX_ACTIVE_NEAR.saturating_sub(near);
    let challenge_slots = MAX_ACTIVE_CHALLENGE.saturating_sub(challenge);

    let system = "你是用户的兴趣探索专家。基于用户已有兴趣，用心理学桥接逻辑猜测用户可能感兴趣但从未接触过的领域。\
猜对方向会升级为正式兴趣，猜错会安静退出——大胆但有依据地猜。\
输出严格 JSON，不要附带解释。";

    let user = format!(
        "{}\n\n## 已确认兴趣（不要重复这些，要猜相邻或桥接的新领域）\n{}\n\n\
## 已在猜测中的方向（排除）\n{}\n\n## 要求\n\
- 生成 {} 个 near（贴近现有兴趣的相邻新领域）+ {} 个挑战方向（lateral 横向迁移 / bridge 心理学桥接 / wildcard 大胆猜测）\n\
- 每个方向：domain（2-6 字中文域）、category（上级分类）、reason（一句话解释为什么猜用户会喜欢，引用具体已有兴趣作桥接依据，至少 20 字）、\
specifics（2-4 个具体细分话题）、probe_mode、confidence（0-1 自评把握）\n\
- 严禁 domain 与已确认兴趣或排除列表相同或包含\n\n\
严格输出 JSON：{{\"speculations\":[{{\"domain\":\"...\",\"category\":\"...\",\"reason\":\"...\",\"specifics\":[\"...\"],\"probe_mode\":\"near|lateral|bridge|wildcard\",\"confidence\":0.5}}]}}",
        profile.to_prompt_context(),
        if confirmed.is_empty() { "（暂无）".to_string() } else { confirmed.join("、") },
        if existing.is_empty() { "（无）".to_string() } else { existing.join("、") },
        near_slots.max(1),
        challenge_slots.max(1),
    );

    let content = super::llm_complete(system, &user, Some(0.9)).await?;
    parse_speculations(&content)
}

/// 解析 LLM 响应为候选列表（含质量门控）
fn parse_speculations(content: &str) -> Option<Vec<SpeculativeInterest>> {
    let value = super::parse_json_tolerant(content)?;
    let items = value
        .get("speculations")
        .and_then(|s| s.as_array())
        .cloned()
        .or_else(|| value.as_array().cloned())?;
    let now = Utc::now().to_rfc3339();
    let mut candidates = Vec::new();
    for item in items {
        let domain = item
            .get("domain")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if domain.is_empty() {
            continue;
        }
        let reason = item
            .get("reason")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let specifics: Vec<String> = item
            .get("specifics")
            .and_then(|s| s.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(|v| v.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let confidence = item
            .get("confidence")
            .and_then(|c| c.as_f64())
            .unwrap_or(0.4);
        let probe_mode = normalize_probe_mode(
            item.get("probe_mode")
                .and_then(|p| p.as_str())
                .unwrap_or("near"),
        );

        // 质量门控：置信度 / 细分数量 / 理由长度
        if confidence < MIN_CONFIDENCE || specifics.len() < 2 || reason.chars().count() < 20 {
            continue;
        }

        candidates.push(SpeculativeInterest {
            domain,
            category: item
                .get("category")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .trim()
                .to_string(),
            reason,
            specifics,
            probe_mode,
            confidence,
            created_at: now.clone(),
            ttl_days: DEFAULT_TTL_DAYS,
            confirmation_count: 0,
            confirmation_threshold: CONFIRMATION_THRESHOLD,
            status: "active".to_string(),
            confirming_events: Vec::new(),
            defer_count: 0,
            deferred_until: String::new(),
        });
    }
    if candidates.is_empty() {
        None
    } else {
        Some(candidates)
    }
}

/// 候选合并：与画像/活跃/冷却去重（子串与 bigram 重叠双重判定），按槽位配额入池
fn merge_candidates(
    state: &mut SpeculativeState,
    candidates: Vec<SpeculativeInterest>,
    profile: &InterestProfile,
) -> Vec<SpeculativeInterest> {
    // 去重术语集：画像兴趣 + 活跃探针 + 冷却
    let mut existing_terms: Vec<String> =
        profile.interests.iter().map(|i| i.domain.clone()).collect();
    existing_terms.extend(state.active.iter().map(|s| s.domain.clone()));
    existing_terms.extend(state.cooldown.iter().map(|c| c.domain.clone()));

    let mut generated = Vec::new();
    for candidate in candidates {
        let domain_lower = candidate.domain.to_lowercase();
        if existing_terms
            .iter()
            .any(|t| is_duplicate_term(&domain_lower, &t.to_lowercase()))
        {
            continue;
        }
        if !state.slot_available(candidate.is_challenge()) {
            continue;
        }
        existing_terms.push(candidate.domain.clone());
        state.active.push(candidate.clone());
        generated.push(candidate);
    }
    generated
}

/// 重复判定：相同 / 互为子串 / bigram 重叠 ≥2（且候选 bigram ≥4）
fn is_duplicate_term(candidate: &str, existing: &str) -> bool {
    if candidate.is_empty() || existing.is_empty() {
        return false;
    }
    if candidate == existing || candidate.contains(existing) || existing.contains(candidate) {
        return true;
    }
    let cand_bigrams = chinese_bigrams(candidate);
    if cand_bigrams.len() < 4 {
        return false;
    }
    cand_bigrams
        .intersection(&chinese_bigrams(existing))
        .count()
        >= 2
}

fn normalize_probe_mode(raw: &str) -> String {
    match raw.trim().to_lowercase().as_str() {
        "lateral" | "bridge" | "wildcard" => raw.trim().to_lowercase(),
        _ => "near".to_string(),
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(domain: &str, mode: &str, count: u32) -> SpeculativeInterest {
        SpeculativeInterest {
            domain: domain.to_string(),
            category: String::new(),
            reason: "x".repeat(20),
            specifics: vec!["a".to_string(), "b".to_string()],
            probe_mode: mode.to_string(),
            confidence: 0.5,
            created_at: Utc::now().to_rfc3339(),
            ttl_days: DEFAULT_TTL_DAYS,
            confirmation_count: count,
            confirmation_threshold: CONFIRMATION_THRESHOLD,
            status: "active".to_string(),
            confirming_events: Vec::new(),
            defer_count: 0,
            deferred_until: String::new(),
        }
    }

    #[test]
    fn test_chinese_bigrams() {
        let b = chinese_bigrams("参数化设计");
        assert!(b.contains("参数"));
        assert!(b.contains("数化"));
        assert_eq!(b.len(), 4);
    }

    #[test]
    fn test_text_match_substring() {
        assert!(text_matches_speculation(
            "安藤忠雄清水混凝土建筑讲解",
            "建筑美学",
            ""
        ));
        // bigram 重叠：无子串命中时靠 bigram
        assert!(text_matches_speculation(
            "comfyui图像生成入门教程",
            "AI图像生成工作流",
            ""
        ));
        assert!(!text_matches_speculation("今晚吃什么", "建筑美学", ""));
    }

    #[test]
    fn test_promote_ready() {
        let mut state = SpeculativeState::default();
        state.active.push(spec("A", "near", CONFIRMATION_THRESHOLD));
        state.active.push(spec("B", "near", 1));
        let (promoted, state) = promote_ready(state);
        assert_eq!(promoted.len(), 1);
        assert_eq!(promoted[0].domain, "A");
        assert_eq!(state.active.len(), 1);
    }

    #[test]
    fn test_expire_stale() {
        let mut state = SpeculativeState::default();
        let mut s = spec("旧领域", "near", 0);
        s.created_at = (Utc::now() - Duration::days(30)).to_rfc3339();
        state.active.push(s);
        let (rejected, state) = expire_stale(state, Utc::now());
        assert_eq!(rejected.len(), 1);
        assert_eq!(state.cooldown.len(), 1);
        assert!(state.active.is_empty());
    }

    #[test]
    fn test_is_duplicate_term() {
        assert!(is_duplicate_term("comfyui工作流", "ComfyUI工作流拆解"));
        assert!(is_duplicate_term("机械键盘", "机械键盘"));
        assert!(!is_duplicate_term("建筑美学", "量子物理"));
    }

    #[test]
    fn test_parse_speculations_quality_gate() {
        let json = r#"{"speculations":[
            {"domain":"建筑美学","category":"艺术","reason":"用户喜欢结构与空间的秩序感，这类审美与参数化设计同源，桥接自然","specifics":["参数化设计","混凝土美学"],"probe_mode":"bridge","confidence":0.6},
            {"domain":"低置信","category":"x","reason":"短","specifics":["a"],"probe_mode":"near","confidence":0.1}
        ]}"#;
        let parsed = parse_speculations(json).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].domain, "建筑美学");
        assert_eq!(parsed[0].probe_mode, "bridge");
    }

    #[test]
    fn test_merge_respects_slots() {
        let mut state = SpeculativeState::default();
        for i in 0..MAX_ACTIVE_NEAR {
            state.active.push(spec(&format!("n{}", i), "near", 0));
        }
        let profile = InterestProfile::default();
        let candidates = vec![
            spec("新near域", "near", 0),
            spec("新挑战域", "wildcard", 0),
        ];
        let generated = merge_candidates(&mut state, candidates, &profile);
        // near 池已满，仅挑战域入选
        assert_eq!(generated.len(), 1);
        assert_eq!(generated[0].domain, "新挑战域");
    }
}
