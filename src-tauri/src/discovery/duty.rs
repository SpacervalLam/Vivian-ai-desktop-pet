//! 每日刷B站值班 — 日报 + 月度归档 + 每天配额
//!
//! 对齐 `dsh-bili-scroll` 的值班设计：
//! - **日报**（计数的事实来源）：`<char>/discovery/out/<YYYY-MM-DD>.md`，每个挑中项首行写成
//!   `### BV号`，宿主就数这些标题行算当天挑了几条。
//! - **月度归档**：`<char>/discovery/out/digest-<YYYY-MM>.md`（把当月日报合并去重，供以后检索）。
//! - **每天配额**：`quota_per_day`（默认 3），一天最多跑一轮、全天最多挑这么多条。
//!
//! 一天最多一轮；报告读不到时用 `duty_state.json` 里的 `picked_today` 兜底。

use std::path::PathBuf;

use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::discovery::engine::{self, DUTY_MAX_AGE_SECS};
use crate::discovery::profile::InterestProfile;
use crate::discovery::recommend;
use crate::discovery::store::{ContentItem, ContentStore};
use crate::utils::path::get_character_data_dir;

/// 每天默认挑几条
const DEFAULT_QUOTA_PER_DAY: usize = 3;
const MIN_QUOTA: usize = 1;
const MAX_QUOTA: usize = 10;
// 手动与后台触发可能同时进入；锁覆盖检查、发现和记账，防止重复日报/配额。
static DUTY_ROUND_LOCK: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));

/// 值班状态（一天一轮 + 配额；报告是计数的事实来源，这里只兜底）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DutyState {
    pub quota_per_day: usize,
    /// 最近一次跑值班的本地日期 `YYYY-MM-DD`
    pub last_round_date: String,
    /// 今天跑了几轮
    pub rounds_today: u32,
    /// 兜底：报告读不到时的当天已挑条数
    pub picked_today: u32,
    /// 最近一次做月度归档的月份 `YYYY-MM`
    pub last_digest_month: String,
}

impl Default for DutyState {
    fn default() -> Self {
        Self {
            quota_per_day: DEFAULT_QUOTA_PER_DAY,
            last_round_date: String::new(),
            rounds_today: 0,
            picked_today: 0,
            last_digest_month: String::new(),
        }
    }
}

/// 一轮值班的结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DutyOutcome {
    pub ran: bool,
    pub reason: String,
    pub picked: usize,
    pub subtitle_skipped: usize,
    /// 当天日报的绝对路径
    pub report: String,
    /// 本轮生成的月度归档路径（非归档轮为 None）
    pub digest: Option<String>,
}

fn duty_dir(char_id: &str) -> PathBuf {
    get_character_data_dir(char_id).join("discovery")
}
fn state_path(char_id: &str) -> PathBuf {
    duty_dir(char_id).join("duty_state.json")
}
fn out_dir(char_id: &str) -> PathBuf {
    duty_dir(char_id).join("out")
}
fn today_key() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}
fn report_path(char_id: &str, day: &str) -> PathBuf {
    out_dir(char_id).join(format!("{day}.md"))
}

/// 当天日报的绝对路径
pub fn today_report_path(char_id: &str) -> PathBuf {
    report_path(char_id, &today_key())
}

pub fn load_state(char_id: &str) -> DutyState {
    let mut s: DutyState =
        crate::utils::fs::load_json_or_backup(&state_path(char_id)).unwrap_or_default();
    s.quota_per_day = s.quota_per_day.clamp(MIN_QUOTA, MAX_QUOTA);
    s
}

fn save_state(char_id: &str, state: &DutyState) {
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = crate::utils::fs::write_atomic(&state_path(char_id), &json);
    }
}

/// 设置每天配额（钳 1..=10），返回更新后的状态
pub fn set_quota(char_id: &str, quota: usize) -> DutyState {
    let mut s = load_state(char_id);
    s.quota_per_day = quota.clamp(MIN_QUOTA, MAX_QUOTA);
    save_state(char_id, &s);
    s
}

/// 数日报里挑了几条：以 `###` 开头、后接 `BV…` 或 `bilibili:` 的标题行（按 id 去重）
///
/// 代码围栏（```）里的内容一律不算 —— 保留报告里贴的日志/代码片段不会被误当成"挑中"。
pub fn count_picked_in_report(md: &str) -> usize {
    let mut ids = std::collections::HashSet::new();
    let mut in_fence = false;
    for line in md.lines() {
        let line = line.trim_start();
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let Some(rest) = line.strip_prefix("###") else {
            continue;
        };
        let token: String = rest
            .trim()
            .chars()
            .take_while(|c| !c.is_whitespace())
            .collect();
        if token.starts_with("BV") || token.starts_with("bilibili:") {
            ids.insert(token);
        }
    }
    ids.len()
}

/// 当天已挑条数：日报是事实来源，`duty_state.json` 兜底
pub fn count_picked_today(char_id: &str) -> usize {
    let day = today_key();
    let from_report = std::fs::read_to_string(report_path(char_id, &day))
        .map(|t| count_picked_in_report(&t))
        .unwrap_or(0);
    let state = load_state(char_id);
    let from_state = if state.last_round_date == day {
        state.picked_today as usize
    } else {
        0
    };
    from_report.max(from_state)
}

/// 当天还能挑几条（报告与兜底取大者）
pub fn daily_remaining(report_count: usize, state: &DutyState, quota: usize) -> usize {
    daily_remaining_on(report_count, state, quota, &today_key())
}

fn daily_remaining_on(report_count: usize, state: &DutyState, quota: usize, day: &str) -> usize {
    let state_count = if state.last_round_date == day {
        state.picked_today as usize
    } else {
        0
    };
    let used = report_count.max(state_count);
    quota.saturating_sub(used)
}

/// 把日报按 `###` 标题行切成块（每块含标题行及其后的正文）
fn split_blocks(md: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in md.lines() {
        if line.trim_start().starts_with("###") {
            if !cur.trim().is_empty() {
                blocks.push(cur.trim_end().to_string());
            }
            cur = String::new();
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(line);
    }
    if !cur.trim().is_empty() {
        blocks.push(cur.trim_end().to_string());
    }
    blocks
}

/// 取一个块的 BV（标题行的首个 token）
fn block_bv(block: &str) -> Option<String> {
    let first = block.lines().next()?.trim_start();
    let rest = first.strip_prefix("###")?;
    let token: String = rest
        .trim()
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// 月度归档：把当月日报合并、按 BV 去重成 `out/digest-<YYYY-MM>.md`
pub fn archive_month(char_id: &str, month: &str) -> Option<PathBuf> {
    let dir = out_dir(char_id);
    let mut days: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(month) && n.ends_with(".md") && !n.starts_with("digest-"))
                .unwrap_or(false)
        })
        .collect();
    days.sort();
    let mut seen = std::collections::HashSet::new();
    let mut out = format!("# 刷B站归档 · {month}\n");
    for p in &days {
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        for block in split_blocks(&text) {
            if let Some(bv) = block_bv(&block) {
                if seen.insert(bv) {
                    out.push('\n');
                    out.push_str(&block);
                    out.push('\n');
                }
            }
        }
    }
    let path = dir.join(format!("digest-{month}.md"));
    crate::utils::fs::write_atomic(&path, &out).ok()?;
    Some(path)
}

/// 往当天日报追加一段（沙箱外运行时目录由这里保证存在）
fn append_report(char_id: &str, day: &str, section: &str) -> Result<(), String> {
    let dir = out_dir(char_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建日报目录失败：{e}"))?;
    let path = report_path(char_id, day);
    let mut text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("读取日报失败：{e}")),
    };
    if text.trim().is_empty() {
        text = format!("# 刷B站日报 · {day}\n");
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(section.trim_end());
    text.push('\n');
    crate::utils::fs::write_atomic(&path, &text).map_err(|e| format!("保存日报失败：{e}"))
}

/// 组装本轮日报段落：每条首行固定 `### BV号`（宿主靠它数配额）
fn build_report_section(items: &[ContentItem], subtitle_skipped: usize) -> String {
    // 标题、作者和模型理由可能带换行，不能让它们伪造计数用的 Markdown 标题。
    let line = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut s = String::new();
    for it in items {
        s.push_str(&format!("### {}\n", line(&it.bvid)));
        s.push_str(&format!("- 标题：{}", line(&it.title)));
        if !it.up_name.is_empty() {
            s.push_str(&format!("\n- UP：{}", line(&it.up_name)));
        }
        s.push_str(&format!("\n- 链接：{}", line(&it.url)));
        if !it.reason.is_empty() {
            s.push_str(&format!("\n- 为什么值得：{}", line(&it.reason)));
        }
        let evidence = match it.content_evidence.as_str() {
            "full_subtitle" => "已读完整字幕（首个分P）",
            "subtitle_excerpt" => "已读字幕前段节选（首个分P）",
            "comments_only" => "标题、简介与评论（未读到字幕）",
            "title_only" => "仅标题与简介（未读到字幕）",
            _ => "历史库存，未记录阅读依据",
        };
        s.push_str(&format!("\n- 阅读依据：{evidence}"));
        s.push_str("\n\n");
    }
    if items.is_empty() {
        s.push_str("本轮未挑选推荐内容。\n\n");
    }
    s.push_str(&format!(
        "已尝试但未取得字幕 {} 条，仍按可用的标题、简介与评论评估；这不代表视频没有语音。\n",
        subtitle_skipped
    ));
    s
}

/// 跑一轮值班：一天最多一轮；发现（近 7 天）→ 按剩余配额挑 → 写日报 → 月度归档
pub async fn run_duty_now(char_id: &str) -> DutyOutcome {
    let _round_guard = DUTY_ROUND_LOCK.lock().await;
    let mut state = load_state(char_id);
    let day = today_key();
    let month = day.get(..7).unwrap_or(&day).to_string();
    let report = report_path(char_id, &day).to_string_lossy().to_string();

    // 一天最多一轮
    if state.last_round_date == day {
        return DutyOutcome {
            ran: false,
            reason: format!("今天（{day}）已经跑过一轮了"),
            picked: count_picked_today(char_id),
            subtitle_skipped: 0,
            report,
            digest: None,
        };
    }

    let quota = state.quota_per_day;
    let already = count_picked_today(char_id);
    let remaining = daily_remaining(already, &state, quota);
    if remaining == 0 {
        // 配额已满：记一轮但不再发现，避免反复触发
        state.last_round_date = day.clone();
        state.rounds_today = 1;
        state.picked_today = already as u32;
        save_state(char_id, &state);
        return DutyOutcome {
            ran: false,
            reason: format!("今天配额（{quota} 条）已满"),
            picked: already,
            subtitle_skipped: 0,
            report,
            digest: None,
        };
    }

    let mut profile = InterestProfile::load(char_id);
    let result =
        engine::discover_bilibili_round(char_id, &crate::discovery::BILI_CLIENT, &mut profile)
            .await;
    profile.save(char_id);

    let picked = recommend::pick_bilibili_recommendations(char_id, remaining, DUTY_MAX_AGE_SECS);
    let mut section = build_report_section(&picked, result.subtitle_skipped);
    for warning in &result.warnings {
        section.push_str(&format!("\n本轮异常：{warning}\n"));
    }
    if let Err(reason) = append_report(char_id, &day, &section) {
        return DutyOutcome {
            ran: true,
            reason,
            picked: already,
            subtitle_skipped: result.subtitle_skipped,
            report,
            digest: None,
        };
    }
    if !picked.is_empty() {
        let mut store = ContentStore::load(char_id);
        store.mark_recommended(
            &picked
                .iter()
                .map(|i| format!("{}:{}", i.platform, i.bvid))
                .collect::<Vec<_>>(),
        );
        store.save(char_id);
    }

    // 尚未写入今天状态；同时保留已知挑选数作为报告后续不可读时的兜底。
    let day_count = count_picked_today(char_id).max(already + picked.len());
    state.last_round_date = day.clone();
    state.rounds_today = 1;
    state.picked_today = day_count as u32;
    // 每次新增日报都刷新当月归档，而非只在月初写一次。
    let digest = archive_month(char_id, &month);
    if digest.is_some() {
        state.last_digest_month = month;
    }
    save_state(char_id, &state);

    tracing::info!(
        "[discovery:{}] 刷B站值班：挑 {} 条（配额 {}）· 未取得字幕 {} 条",
        char_id,
        picked.len(),
        quota,
        result.subtitle_skipped
    );

    DutyOutcome {
        ran: true,
        reason: format!("本轮挑了 {} 条", picked.len()),
        picked: day_count,
        subtitle_skipped: result.subtitle_skipped,
        report,
        digest: digest.map(|p| p.to_string_lossy().to_string()),
    }
}

/// 周期维护里调用：便宜守卫（一天一轮）后跑值班；已跑过则返回 None
pub async fn maybe_run_daily(char_id: &str) -> Option<DutyOutcome> {
    if load_state(char_id).last_round_date == today_key() {
        return None;
    }
    Some(run_duty_now(char_id).await)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::bilibili::VideoInfo;

    fn item(bvid: &str, score: f64, topic: &str) -> ContentItem {
        ContentItem::from_video(
            &VideoInfo {
                bvid: bvid.to_string(),
                title: format!("标题{bvid}"),
                description: String::new(),
                up_name: "up".to_string(),
                cover_url: String::new(),
                duration_secs: 300,
                view_count: 1000,
                like_count: 100,
                pubdate: 0,
                source: "search:测试".to_string(),
            },
            score,
            "匹配画像".to_string(),
            topic.to_string(),
        )
    }

    #[test]
    fn test_count_picked_in_report() {
        let md = "# 日报\n\n### BV1aaaaaaaaa\n- 标题：x\n\n### bilibili:BV2bbbbbbbbb\n- 标题：y\n\n### 只是提到的视频\n- 不算\n\n```\n### BV9codefence\n```\n";
        // 只数 `###` 标题行且 token 以 BV / bilibili: 开头；代码围栏里的不算，非标题行也不算
        assert_eq!(count_picked_in_report(md), 2);
        assert_eq!(count_picked_in_report(""), 0);
    }

    #[test]
    fn test_daily_remaining_prefers_max() {
        let mut state = DutyState::default();
        state.last_round_date = today_key();
        state.picked_today = 2;
        // 报告数更大时以报告为准
        assert_eq!(daily_remaining(3, &state, 5), 2);
        // 兜底更大时以兜底为准
        assert_eq!(daily_remaining(0, &state, 5), 3);
        // 超额时不为负
        assert_eq!(daily_remaining(9, &state, 5), 0);
    }

    #[test]
    fn yesterday_does_not_consume_today_quota() {
        let state = DutyState {
            last_round_date: "2026-10-19".into(),
            picked_today: 3,
            ..Default::default()
        };
        assert_eq!(daily_remaining_on(0, &state, 3, "2026-10-20"), 3);
        assert_eq!(daily_remaining_on(1, &state, 3, "2026-10-20"), 2);
        assert_eq!(daily_remaining_on(0, &state, 3, "2026-10-19"), 0);
    }

    #[test]
    fn report_labels_subtitle_excerpt() {
        let mut video = item("BV1aaaaaaaaa", 0.8, "科技");
        video.content_evidence = "subtitle_excerpt".into();
        let report = build_report_section(&[video], 0);
        assert!(report.contains("已读字幕前段节选"));
        assert!(!report.contains("已读完整字幕"));
    }

    #[test]
    fn external_text_cannot_forge_picked_headings() {
        let mut video = item("BV1aaaaaaaaa", 0.8, "科技");
        video.title = "标题\n### BV2bbbbbbbbb".into();
        video.reason = "匹配兴趣\n### BV3ccccccccc".into();
        let report = build_report_section(&[video], 0);
        assert_eq!(count_picked_in_report(&report), 1);
    }

    #[test]
    fn test_split_blocks_and_bv() {
        let md = "# 头\n### BV1aaaaaaaaa\n- 标题：x\n### BV2bbbbbbbbb\n- 标题：y\n";
        let blocks = split_blocks(md);
        assert_eq!(blocks.len(), 3); // 头部块 + 两个条目块
        let bvs: Vec<String> = blocks.iter().filter_map(|b| block_bv(b)).collect();
        assert_eq!(bvs, vec!["BV1aaaaaaaaa", "BV2bbbbbbbbb"]);
    }

    #[test]
    fn test_build_report_section_headings() {
        let s = build_report_section(&[item("BV1aaaaaaaaa", 0.8, "科技")], 3);
        assert!(s.starts_with("### BV1aaaaaaaaa\n"));
        assert!(s.contains("未取得字幕 3 条"));
        assert!(s.contains("阅读依据：历史库存"));
        // 空列表也留下无字幕统计
        let empty = build_report_section(&[], 0);
        assert!(empty.contains("未取得字幕 0 条"));
    }

    #[test]
    fn test_set_quota_clamps() {
        let s = DutyState {
            quota_per_day: 99,
            ..Default::default()
        };
        assert_eq!(s.quota_per_day.clamp(MIN_QUOTA, MAX_QUOTA), MAX_QUOTA);
    }
}
