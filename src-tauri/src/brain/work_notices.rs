//! 工作智能体 → 陪伴角色的通知暂存。
//!
//! 两条产品线各有独立循环与模型路由，彼此没有共享上下文。陪伴角色要知道工作侧
//! 发生了什么，只能经由这里——而这里**只存事实，不存文案、不做决策**：要不要说、
//! 所有用户办公会话（手动发起或桌宠委派）的关键回调必须主动转达。
//!
//! 两类通知的事实源头不同，实现方式因此也不同：
//!
//! - **等你拍板**（`work_ask_user` 挂起中）：源头是问题注册表的 pending 表，
//!   直接派生即可。另存一份就要跟着它一起过期、一起清理，迟早对不上。
//! - **任务完成报告**：没有现成的持久化源头，在本模块暂存。
//!
//! 三类素材的性子不同，投递路径也不同（"转达检查点" = `proactive::relay_work_notice`）：
//!
//! | 素材 | 从哪来 | 什么时候动嘴 | 走哪条路 |
//! |---|---|---|---|
//! | 完成报告 | `coding_agent` 自动登记 | 用户看不见工作页时补一次 | 提示词段落 + 转达检查点 |
//! | 等你拍板 | 问题注册表派生 | 用户看不见**那条**提问时必须说 | 提示词段落 + 转达检查点（可重复催） |
//! | 必须转达 | `notify_companion` 工具 | 普通任务按可见性门控 | 只走转达检查点 |
//! | 桌宠任务回调 | 阶段/阻塞/最终结果 | 必须主动转达，不受可见性和普通冷却抑制 | 转达检查点，实际投递后按事件确认 |
//!
//! 前两类同时进提示词段落：用户主动找角色说话时素材就在上下文里，不必干等
//! 下一个 tick。第三类刻意不进——它的语义纯粹是"主动说出去"，放进段落只会让
//! 同一件事多出一条并行路径、被说两遍。
//!
//! 完成报告的消费是**两段式**：注入提示词时只把条目「领取」（`claim`，标记已占用
//! 但留在队列里），这一轮真的把回复产出之后再「确认」（`confirm`）才移除。生成失败
//! 就 `release` 放回队列等下个 tick 重试——代价只是被晚说一次，而不是悄悄消失。
//! 这是 `next_alert_for` 早就采用的「先看后取」语义，报告这条路径此前是唯一的例外。
//! 连续 3 次都没说出去才真正丢弃并留一条 warn：重提太多次也伤，但那是"陈旧"这个
//! 第三种情形，不该由"生成失败"来承担。进程重启时所有领取标记作废（`persistent`
//! 会清掉），因为不会有谁来确认它们了。

use std::collections::HashSet;
use std::sync::Arc;
use serde::{Deserialize, Serialize};

use once_cell::sync::Lazy;
use parking_lot::RwLock;

use super::work_question::WorkQuestionRequest;

// Emit desktop notifications independently of companion generation and tick timing.
static NOTIFICATION_APP: Lazy<RwLock<Option<tauri::AppHandle>>> = Lazy::new(|| RwLock::new(None));
type MessageSink = Arc<dyn Fn(&str, &str, &str, &str, &str) + Send + Sync>;
static MESSAGE_SINK: Lazy<RwLock<Option<MessageSink>>> = Lazy::new(|| RwLock::new(None));
pub fn set_message_sink(sink: MessageSink) { *MESSAGE_SINK.write() = Some(sink); }
fn forward_message(char_id: &str, id: &str, session: &str, title: &str, body: &str) {
    let sink = MESSAGE_SINK.read().clone();
    if let Some(sink) = sink { sink(char_id, id, session, title, body); }
}
fn report_message_id(session: &str, at: f64, title: &str, body: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in title.bytes().chain(std::iter::once(0)).chain(body.bytes()) {
        hash ^= u64::from(byte); hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("report:{session}:{}:{hash}",at.to_bits())
}

pub fn set_app_handle(app: tauri::AppHandle) {
    *NOTIFICATION_APP.write() = Some(app);
}

fn desktop_attention_kind(title: &str) -> Option<&'static str> {
    match title {
        "工作结果已返回" | "任务完成" => Some("success"),
        "任务等待你的决定" => Some("info"),
        "任务未完成" | "遇到执行障碍" | "执行已中断" | "模型调用失败" => Some("warning"),
        _ if title.contains("阻塞") || title.contains("受阻") => Some("warning"),
        _ => None,
    }
}

pub fn notify_desktop(char_id: &str, session_id: &str, title: &str) {
    use tauri::Emitter;
    let Some(kind) = desktop_attention_kind(title) else { return; };
    let Some(app) = NOTIFICATION_APP.read().clone() else { return; };
    if crate::commands::inspector::is_viewing_work_page(&app) { return; }
    let _ = app.emit("work:attention", serde_json::json!({
        "character_id": char_id, "session_id": session_id, "title": title, "kind": kind,
    }));
}

/// 完成报告存活时长（秒）。过期的"活干完了"再播报只会突兀。
const REPORT_TTL_SECS: f64 = 300.0;

/// 单角色报告队列上限。
const MAX_REPORTS_PER_CHAR: usize = 8;

/// 一份完成报告最多被"领走"几次。
///
/// 领取后生成失败会把报告放回队列；连续失败到这个次数才放弃。3 次足够跨过一次
/// 网络抖动或一轮模型报错，又不至于让一份说不出去的报告无限占用队列位置。
const MAX_REPORT_ATTEMPTS: u32 = 3;

/// 「必须转达」的存活时长（秒）。
///
/// 比完成报告长得多：这条是工作智能体明确要求告诉用户的事，不该因为
/// 用户恰好离开了二十分钟就被悄悄丢掉。超过这个时长还没说出去才作废——
/// 半小时前的工作状态已经不值得再提。
const ALERT_TTL_SECS: f64 = 1800.0;

/// 单角色「必须转达」队列上限。
const MAX_ALERTS_PER_CHAR: usize = 8;

/// 一条待转告的工作完成事实。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkReport {
    pub session_id: String,
    /// 事实标题（如"任务完成"），不做润色
    pub title: String,
    /// 事实正文
    pub body: String,
}

/// 一条工作智能体明确要求转达给用户的事。
///
/// 与 [`WorkReport`] 的分界：报告是"告知"，说与不说交给陪伴侧的闸门；
/// 这里是"必须说"，因为工作智能体已经显式要求过了。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkAlert {
    pub notice_id: String,
    /// Key callbacks for all user office sessions bypass discretionary gates.
    pub required: bool,
    pub session_id: String,
    pub title: String,
    pub body: String,
}

impl WorkAlert {
    pub fn should_relay(&self, visible_session: Option<&str>, cooled: bool) -> bool {
        self.required || (!cooled && visible_session != Some(self.session_id.as_str()))
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    char_id: String,
    report: WorkReport,
    at: f64,
    /// 已被某一轮生成领走、正在等确认。领取的条目不再参与下一轮注入，但**不删除**：
    /// 生成失败时它还要回来。进程重启会清掉这个标记。
    #[serde(default)]
    claimed: bool,
    /// 已因生成失败被放回的次数。到上限才真正丢弃，避免永不成功的报告一直转圈。
    #[serde(default)]
    attempts: u32,
}

#[derive(Clone, Serialize, Deserialize)]
struct AlertEntry {
    char_id: String,
    alert: WorkAlert,
    at: f64,
}

/// 完成报告暂存表 + 强制转达队列 + 提问提醒标记。
#[derive(Default, Serialize, Deserialize)]
struct NoticeSnapshot {
    entries: Vec<Entry>,
    alerts: Vec<AlertEntry>,
}

fn terminal_notice(title: &str) -> bool {
    matches!(title, "工作结果已返回" | "任务完成" | "任务未完成" | "执行已中断" | "模型调用失败")
}

pub struct WorkNoticeStore {
    path: Option<std::path::PathBuf>,
    persistence_gate: parking_lot::Mutex<()>,
    entries: RwLock<Vec<Entry>>,
    alerts: RwLock<Vec<AlertEntry>>,
    /// 已就某条提问提醒过用户的 question_id。
    ///
    /// 只影响提示词里的引导语气：首次给"提醒用户"，之后改给"他已经知道了，
    /// 不必反复催促"。问题解决后随条目一起被清理，不需要单独的生命周期。
    reminded: RwLock<HashSet<u64>>,
}

impl WorkNoticeStore {
    fn new() -> Self {
        Self {
            path: None,
            persistence_gate: parking_lot::Mutex::new(()),
            entries: RwLock::new(Vec::new()),
            alerts: RwLock::new(Vec::new()),
            reminded: RwLock::new(HashSet::new()),
        }
    }

    fn persistent(path: std::path::PathBuf) -> Self {
        let mut snapshot = std::fs::read(&path).ok().and_then(|raw| serde_json::from_slice::<NoticeSnapshot>(&raw).ok()).unwrap_or_default();
        let now = chrono::Local::now().timestamp() as f64;
        Self::prune_locked(&mut snapshot.entries, now);
        Self::prune_alerts_locked(&mut snapshot.alerts, now);
        // A crashed process never confirms its claims, so a restart has to
        // hand them back rather than leave them claimed forever.
        for entry in &mut snapshot.entries { entry.claimed = false; }
        let mut store = Self::new();
        store.path = Some(path);
        *store.entries.write() = snapshot.entries;
        *store.alerts.write() = snapshot.alerts;
        store
    }

    fn persist(&self) {
        let Some(path) = &self.path else { return; };
        let _gate = self.persistence_gate.lock();
        let snapshot = NoticeSnapshot { entries:self.entries.read().clone(), alerts:self.alerts.read().clone() };
        if let Err(error) = serde_json::to_vec(&snapshot).map_err(std::io::Error::other)
            .and_then(|raw| crate::utils::fs::atomic_write(path, &raw)) {
            tracing::warn!("[WorkNotices] 保存待转达消息失败: {error}");
        }
    }

    /// 登记一条工作完成事实。
    ///
    /// 同一会话的旧报告会被顶掉——同一会话连续完成多次时，只有最后一次值得转告。
    /// 返回是否登记成功（角色或正文为空时不登记）。
    pub fn push_report(&self, char_id: &str, session_id: &str, title: &str, body: &str) -> bool {
        let body = body.trim();
        if char_id.is_empty() || body.is_empty() {
            return false;
        }
        let title = if title.trim().is_empty() { "任务完成" } else { title.trim() };
        let now = chrono::Local::now().timestamp() as f64;

        let mut entries = self.entries.write();
        Self::prune_locked(&mut entries, now);
        entries.retain(|e| !(e.char_id == char_id && e.report.session_id == session_id));
        entries.push(Entry {
            char_id: char_id.to_string(),
            report: WorkReport {
                session_id: session_id.to_string(),
                title: title.to_string(),
                body: body.to_string(),
            },
            at: now,
            claimed: false,
            attempts: 0,
        });
        // 超限时丢该角色最早的一条
        loop {
            let mine: Vec<(usize, f64)> = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.char_id == char_id)
                .map(|(i, e)| (i, e.at))
                .collect();
            if mine.len() <= MAX_REPORTS_PER_CHAR {
                break;
            }
            let oldest = mine
                .into_iter()
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
                .unwrap_or(0);
            entries.remove(oldest);
        }
        drop(entries);
        self.persist();
        forward_message(char_id, &report_message_id(session_id,now,title,body), session_id, title, body);
        true
    }

    /// 领取该角色尚未转告的完成报告。
    ///
    /// 只标记不删除：这一轮生成成功与否此时还不知道，成功后才由 [`confirm_reports`]
    /// 移除。领走的条目在确认前不会被再次领取，所以同一件事不会在一轮里被念两遍。
    ///
    /// **上一轮领走却没确认的条目在这里先放回**——生成失败、被用户打断、请求被丢弃，
    /// 或进程在生成中途退出，都表现为"领了没确认"。下一次领取就是天然的回收点，
    /// 不需要超时计时器去猜一轮该有多长。这份报告因此只是被晚说一次。
    pub fn claim_reports_for(&self, char_id: &str) -> Vec<WorkReport> {
        if char_id.is_empty() {
            return Vec::new();
        }
        let now = chrono::Local::now().timestamp() as f64;
        let mut entries = self.entries.write();
        let abandoned = Self::requeue_claims_locked(&mut entries, char_id, now);
        Self::prune_locked(&mut entries, now);
        let mut out = Vec::new();
        for entry in entries.iter_mut().filter(|e| e.char_id == char_id) {
            entry.claimed = true;
            out.push(entry.report.clone());
        }
        drop(entries);
        if abandoned > 0 {
            tracing::warn!("[WorkNotices] {char_id}: {abandoned} 条完成报告连续 {MAX_REPORT_ATTEMPTS} 次没能说出去，已丢弃");
        }
        self.persist();
        out
    }

    /// 本轮生成成功：领取过的报告确实进了这一轮回复，可以移除。
    pub fn confirm_reports(&self, char_id: &str) {
        let mut entries = self.entries.write();
        entries.retain(|e| !(e.char_id == char_id && e.claimed));
        drop(entries);
        self.persist();
    }

    /// 把上一轮领走却没确认的条目标回未领取，并计一次失败；到上限的丢弃。
    ///
    /// `at` 一并重置：重试期的条目不该被 TTL 按"原始入队时间"判成陈旧。
    /// "说不出去"和"已经过期"是两件事，各管各的。返回被丢弃的条数。
    fn requeue_claims_locked(entries: &mut Vec<Entry>, char_id: &str, now: f64) -> usize {
        let mut abandoned = 0usize;
        entries.retain_mut(|e| {
            if e.char_id != char_id || !e.claimed {
                return true;
            }
            e.claimed = false;
            e.attempts = e.attempts.saturating_add(1);
            if e.attempts >= MAX_REPORT_ATTEMPTS {
                abandoned += 1;
                return false;
            }
            e.at = now;
            true
        });
        abandoned
    }

    /// 登记一条「必须转达」——工作智能体已显式要求把这件事告诉用户。
    ///
    /// 同一会话的旧条目会被顶掉：同一会话连续要求转达时，最新那条才代表现状。
    pub fn push_alert(&self, char_id: &str, session_id: &str, title: &str, body: &str) -> bool {
        self.push_alert_inner(char_id, session_id, title, body, false)
    }

    pub fn push_callback(&self, char_id: &str, session_id: &str, title: &str, body: &str) -> bool {
        self.push_alert_inner(char_id, session_id, title, body, true)
    }

    fn push_alert_inner(&self, char_id: &str, session_id: &str, title: &str, body: &str, required: bool) -> bool {
        let body = body.trim();
        if char_id.is_empty() || body.is_empty() {
            return false;
        }
        let title = if title.trim().is_empty() { "工作进展" } else { title.trim() };
        let now = chrono::Local::now().timestamp() as f64;

        let mut alerts = self.alerts.write();
        Self::prune_alerts_locked(&mut alerts, now);
        if let Some(existing) = alerts.iter_mut().find(|a| a.char_id == char_id && a.alert.session_id == session_id
            && a.alert.title == title && a.alert.body == body) {
            let promoted = required && !existing.alert.required;
            existing.alert.required |= required;
            let message_id = existing.alert.notice_id.clone();
            drop(alerts);
            self.persist();
            if promoted { notify_desktop(char_id, session_id, title); }
            forward_message(char_id, &message_id, session_id, title, body);
            return true;
        }
        // A final result subsumes undelivered milestones and outdated blockers for this run.
        if required && terminal_notice(title) {
            alerts.retain(|a| !(a.char_id == char_id && a.alert.session_id == session_id));
        } else if required {
            // Repeated automatic milestones describe the latest state of that milestone.
            alerts.retain(|a| !(a.char_id == char_id && a.alert.session_id == session_id && a.alert.title == title));
        }
        if !required { alerts.retain(|a| !(a.char_id == char_id && a.alert.session_id == session_id && !a.alert.required)); }
        let message_id = uuid::Uuid::new_v4().to_string();
        alerts.push(AlertEntry {
            char_id: char_id.to_string(),
            alert: WorkAlert {
                notice_id: message_id.clone(), required,
                session_id: session_id.to_string(),
                title: title.to_string(),
                body: body.to_string(),
            },
            at: now,
        });
        while Self::count_for(&alerts, char_id) > MAX_ALERTS_PER_CHAR {
            let oldest = alerts
                .iter()
                .enumerate()
                .filter(|(_, a)| a.char_id == char_id)
                .map(|(i, a)| (i, a.at))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i);
            match oldest {
                Some(i) => {
                    alerts.remove(i);
                }
                None => break,
            }
        }
        drop(alerts);
        self.persist();
        if required { notify_desktop(char_id, session_id, title); }
        forward_message(char_id, &message_id, session_id, title, body);
        true
    }

    /// 取该角色最早一条待转达的事（只看不取，成功说出去之后再 `take_alert`）。
    ///
    /// 刻意不在这里消费：生成可能失败，失败就该下个 tick 重试，
    /// 而不是把"必须转达"悄悄吞掉。
    pub fn next_alert_for(&self, char_id: &str) -> Option<WorkAlert> {
        if char_id.is_empty() {
            return None;
        }
        let now = chrono::Local::now().timestamp() as f64;
        let mut alerts = self.alerts.write();
        Self::prune_alerts_locked(&mut alerts, now);
        alerts.iter().find(|a| a.char_id == char_id && a.alert.required && terminal_notice(&a.alert.title))
            .or_else(|| alerts.iter().find(|a| a.char_id == char_id && a.alert.required && a.alert.title == "任务等待你的决定"))
            .or_else(|| alerts.iter().find(|a| a.char_id == char_id && a.alert.required))
            .or_else(|| alerts.iter().find(|a| a.char_id == char_id))
            .map(|a| a.alert.clone())
    }

    pub fn has_alert(&self, notice_id: &str) -> bool {
        let now = chrono::Local::now().timestamp() as f64;
        let mut alerts = self.alerts.write();
        Self::prune_alerts_locked(&mut alerts, now);
        alerts.iter().any(|a| a.alert.notice_id == notice_id)
    }

    /// A resolved question must not be announced later as a current blocker.
    pub fn clear_attention_callback(&self, session_id: &str) {
        self.alerts.write().retain(|a| !(a.alert.session_id == session_id && a.alert.title == "任务等待你的决定"));
        self.persist();
    }

    /// Acknowledge only the delivered event, preserving callbacks arriving during generation.
    pub fn acknowledge_alert(&self, notice_id: &str) {
        self.alerts.write().retain(|a| a.alert.notice_id != notice_id);
        self.persist();
    }

    /// 一条「必须转达」已成功说出去，移除它。
    pub fn take_alert(&self, session_id: &str) {
        self.alerts
            .write()
            .retain(|a| a.alert.session_id != session_id);
        self.persist();
    }

    /// 该角色是否还有待转告的完成报告。
    ///
    /// 只看不取：报告由提示词段落领取（`claim_reports_for`）并在生成成功后确认，
    /// 这里只是让 tick 判断"值不值得给一次搭话机会"。已领取但还没确认的条目也算在内，
    /// 因为那份报告确实还没说出去。
    pub fn has_report_for(&self, char_id: &str) -> bool {
        if char_id.is_empty() {
            return false;
        }
        let now = chrono::Local::now().timestamp() as f64;
        let mut entries = self.entries.write();
        Self::prune_locked(&mut entries, now);
        entries.iter().any(|e| e.char_id == char_id)
    }

    /// Non-destructive transport view. Receiving is independent of presenting a callback.
    pub fn inbox_facts_for(&self, char_id: &str) -> Vec<(String, WorkReport, i64)> {
        let mut facts: Vec<_> = self.entries.read().iter().filter(|entry| entry.char_id == char_id).map(|entry|
            (report_message_id(&entry.report.session_id,entry.at,&entry.report.title,&entry.report.body),entry.report.clone(),entry.at as i64)).collect();
        facts.extend(self.alerts.read().iter().filter(|entry| entry.char_id == char_id).map(|entry|
            (entry.alert.notice_id.clone(),WorkReport { session_id:entry.alert.session_id.clone(),title:entry.alert.title.clone(),body:entry.alert.body.clone() },entry.at as i64)));
        facts
    }

    /// 会话被取消 / 重置时丢弃其未转告的报告与待转达项。
    pub fn drop_session(&self, session_id: &str) {
        self.entries
            .write()
            .retain(|e| e.report.session_id != session_id);
        self.alerts
            .write()
            .retain(|a| a.alert.session_id != session_id);
        self.persist();
    }

    fn count_for(alerts: &[AlertEntry], char_id: &str) -> usize {
        alerts.iter().filter(|a| a.char_id == char_id).count()
    }

    fn prune_alerts_locked(alerts: &mut Vec<AlertEntry>, now: f64) {
        alerts.retain(|a| now - a.at < ALERT_TTL_SECS);
    }

    /// 标记已就某条提问提醒过用户。
    pub fn mark_attention_reminded(&self, question_id: u64) {
        self.reminded.write().insert(question_id);
    }

    /// 是否已就某条提问提醒过用户。
    pub fn attention_reminded(&self, question_id: u64) -> bool {
        self.reminded.read().contains(&question_id)
    }

    /// 丢弃过期报告。
    fn prune_locked(entries: &mut Vec<Entry>, now: f64) {
        entries.retain(|e| now - e.at < REPORT_TTL_SECS);
    }

    /// 把提醒标记收敛到当前仍然 pending 的提问上。
    ///
    /// 提问被回答 / 取消 / 超时后，它的 id 不该继续占着集合——否则长时间运行
    /// 下来会只增不减。
    fn prune_reminded(&self, live: &[u64]) {
        let mut reminded = self.reminded.write();
        if reminded.is_empty() {
            return;
        }
        let live: HashSet<u64> = live.iter().copied().collect();
        reminded.retain(|id| live.contains(id));
    }
}

impl Default for WorkNoticeStore {
    fn default() -> Self {
        Self::new()
    }
}

static STORE: Lazy<Arc<WorkNoticeStore>> = Lazy::new(|| Arc::new(WorkNoticeStore::persistent(
    crate::utils::path::get_user_data_dir().join("work_notices.json"))));

/// 全局暂存表访问器（工作侧写入、陪伴侧读取共用同一实例）。
pub fn global() -> Arc<WorkNoticeStore> {
    Arc::clone(&STORE)
}

/// 该角色当前"卡在用户拍板上"的提问。
///
/// 直接派生自问题注册表——与 `WorkQuestionRegistry` 是同一事实的两种视角，
/// 因此不另存状态。附带收敛一次提醒标记，避免已解答的提问把集合撑大。
pub fn pending_attention_for(char_id: &str) -> Vec<WorkQuestionRequest> {
    if char_id.is_empty() {
        return Vec::new();
    }
    let pendings = super::work_question::global_work_question_registry().pending_for_char(char_id);
    let ids: Vec<u64> = pendings.iter().map(|p| p.question_id).collect();
    global().prune_reminded(&ids);
    pendings
}

#[cfg(test)]
mod callback_tests {
    use super::*;

    #[test]
    fn callbacks_are_required_owned_and_acknowledged_individually() {
        let store = WorkNoticeStore::new();
        assert!(store.push_alert("vivian", "other", "普通提醒", "普通办公任务"));
        assert!(store.push_callback("vivian", "task", "阶段完成", "资料收集完毕"));
        let first = store.next_alert_for("vivian").unwrap();
        assert!(first.required);
        assert!(first.should_relay(Some("task"), true));
        assert_eq!(first.session_id, "task");
        assert!(store.next_alert_for("nana").is_none());
        // Duplicate queued facts are suppressed, but a new callback during generation survives.
        store.push_callback("vivian", "task", "阶段完成", "资料收集完毕");
        store.push_callback("vivian", "task", "工作结果已返回", "报告已生成并验证");
        assert!(!store.has_alert(&first.notice_id), "a final result invalidates the old milestone");
        store.acknowledge_alert(&first.notice_id);
        assert!(!store.has_alert(&first.notice_id));
        let next = store.next_alert_for("vivian").unwrap();
        assert_eq!(next.title, "工作结果已返回");
        assert_ne!(first.notice_id, next.notice_id);
        store.drop_session("task");
        let ordinary = store.next_alert_for("vivian").unwrap();
        assert!(!ordinary.required);
        assert!(!ordinary.should_relay(Some("other"), false));
        assert!(!ordinary.should_relay(None, true));
        assert!(ordinary.should_relay(None, false));
    }

    #[test]
    fn resolved_questions_do_not_leave_stale_blocker_callbacks() {
        let store = WorkNoticeStore::new();
        store.push_callback("vivian", "task", "任务等待你的决定", "选择输出格式");
        let question = store.next_alert_for("vivian").unwrap();
        store.push_callback("vivian", "task", "阶段完成", "资料已准备好");
        store.clear_attention_callback("task");
        assert!(!store.has_alert(&question.notice_id));
        assert_eq!(store.next_alert_for("vivian").unwrap().title, "阶段完成");
    }

    #[test]
    fn callbacks_reject_empty_facts_and_bound_each_character_queue() {
        let store = WorkNoticeStore::new();
        assert!(!store.push_callback("", "task", "完成", "真实结果"));
        assert!(!store.push_callback("nana", "task", "完成", "  "));
        for i in 0..20 { store.push_callback("nana", &format!("task-{i}"), "进展", &format!("阶段 {i}")); }
        assert_eq!(store.alerts.read().len(), MAX_ALERTS_PER_CHAR);
        assert!(store.next_alert_for("vivian").is_none());
    }

    #[test]
    fn queued_facts_survive_restart_and_acknowledgement_is_persisted() {
        let path = std::env::temp_dir().join(format!("vivian-notices-{}.json", uuid::Uuid::new_v4()));
        let store = WorkNoticeStore::persistent(path.clone());
        store.push_callback("nana", "work", "任务等待你的决定", "选择格式");
        let notice = store.next_alert_for("nana").unwrap();
        drop(store);
        let restored = WorkNoticeStore::persistent(path.clone());
        assert_eq!(restored.next_alert_for("nana").unwrap().notice_id, notice.notice_id);
        assert!(restored.next_alert_for("vivian").is_none());
        restored.acknowledge_alert(&notice.notice_id);
        assert!(WorkNoticeStore::persistent(path.clone()).next_alert_for("nana").is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn required_callback_promotes_an_existing_alert_without_duplicate_identity() {
        let store = WorkNoticeStore::new();
        store.push_alert("vivian", "task", "发现", "实际事实");
        let old = store.next_alert_for("vivian").unwrap();
        store.push_callback("vivian", "task", "发现", "实际事实");
        let promoted = store.next_alert_for("vivian").unwrap();
        assert_eq!(old.notice_id, promoted.notice_id);
        assert!(promoted.required);
        assert_eq!(store.alerts.read().len(), 1);
    }

    #[test]
    fn reports_are_only_dropped_after_the_reply_is_delivered() {
        let store = WorkNoticeStore::new();
        store.push_report("vivian", "s1", "工作结果已返回", "改了三处");
        let claimed = store.claim_reports_for("vivian");
        assert_eq!(claimed.len(), 1);
        // A claim is not a delivery: a reply that fails must not cost the report.
        assert!(store.has_report_for("vivian"));
        // Each claim builds a new reply prompt; the API has no turn identifier.
        // A new prompt retries the previous unconfirmed claim.
        assert_eq!(store.claim_reports_for("vivian").len(), 1);
        store.confirm_reports("vivian");
        assert!(!store.has_report_for("vivian"));
        assert!(store.claim_reports_for("vivian").is_empty());
    }

    #[test]
    fn a_report_that_never_gets_delivered_is_abandoned_after_the_retry_budget() {
        let store = WorkNoticeStore::new();
        store.push_report("nana", "s1", "工作结果已返回", "结果");
        for _ in 0..MAX_REPORT_ATTEMPTS {
            assert_eq!(store.claim_reports_for("nana").len(), 1);
        }
        assert!(store.claim_reports_for("nana").is_empty(), "an undeliverable report must not cycle forever");
        assert!(!store.has_report_for("nana"));
    }

    #[test]
    fn a_restart_hands_claimed_reports_back_instead_of_swallowing_them() {
        let path = std::env::temp_dir().join(format!("vivian-reports-{}.json", uuid::Uuid::new_v4()));
        let store = WorkNoticeStore::persistent(path.clone());
        store.push_report("vivian", "s1", "工作结果已返回", "结果");
        assert_eq!(store.claim_reports_for("vivian").len(), 1);
        drop(store);
        // A crashed process never confirms, so the claim must not survive it.
        let restored = WorkNoticeStore::persistent(path.clone());
        assert!(restored.has_report_for("vivian"));
        assert_eq!(restored.claim_reports_for("vivian").len(), 1);
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod desktop_attention_tests {
    use super::desktop_attention_kind;

    #[test]
    fn only_actionable_work_events_notify() {
        assert_eq!(desktop_attention_kind("工作结果已返回"), Some("success"));
        assert_eq!(desktop_attention_kind("任务等待你的决定"), Some("info"));
        for title in ["任务未完成", "遇到执行障碍", "执行已中断", "模型调用失败", "任务受阻", "阻塞"] {
            assert_eq!(desktop_attention_kind(title), Some("warning"));
        }
        for title in ["阶段完成", "工作进展", "验证通过"] {
            assert_eq!(desktop_attention_kind(title), None);
        }
    }
}
