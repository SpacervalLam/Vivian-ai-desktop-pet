//! 工作智能体 → 陪伴角色的通知暂存。
//!
//! 两条产品线各有独立循环与模型路由，彼此没有共享上下文。陪伴角色要知道工作侧
//! 发生了什么，只能经由这里——而这里**只存事实，不存文案、不做决策**：要不要说、
//! 什么时候说、用什么口吻说，一概由陪伴侧自己的提示词与门控决定。
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
//! | 必须转达 | `notify_companion` 工具 | 用户看不见该会话时必须说 | 只走转达检查点 |
//!
//! 前两类同时进提示词段落：用户主动找角色说话时素材就在上下文里，不必干等
//! 下一个 tick。第三类刻意不进——它的语义纯粹是"主动说出去"，放进段落只会让
//! 同一件事多出一条并行路径、被说两遍。
//!
//! 完成报告在**注入提示词时即消费**（`background_tasks` 段落每轮都会注入，
//! 不消费会让角色反复念同一件事）。代价是"拼好了提示词但本轮生成失败"会丢掉
//! 这份报告——相比反复重提同一件事，这个损失更小，且有 TTL 兜住上限。

use std::collections::HashSet;
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::RwLock;

use super::work_question::WorkQuestionRequest;

/// 完成报告存活时长（秒）。过期的"活干完了"再播报只会突兀。
const REPORT_TTL_SECS: f64 = 300.0;

/// 单角色报告队列上限。
const MAX_REPORTS_PER_CHAR: usize = 8;

/// 「必须转达」的存活时长（秒）。
///
/// 比完成报告长得多：这条是工作智能体明确要求告诉用户的事，不该因为
/// 用户恰好离开了二十分钟就被悄悄丢掉。超过这个时长还没说出去才作废——
/// 半小时前的工作状态已经不值得再提。
const ALERT_TTL_SECS: f64 = 1800.0;

/// 单角色「必须转达」队列上限。
const MAX_ALERTS_PER_CHAR: usize = 8;

/// 一条待转告的工作完成事实。
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct WorkAlert {
    pub session_id: String,
    pub title: String,
    pub body: String,
}

struct Entry {
    char_id: String,
    report: WorkReport,
    at: f64,
}

struct AlertEntry {
    char_id: String,
    alert: WorkAlert,
    at: f64,
}

/// 完成报告暂存表 + 强制转达队列 + 提问提醒标记。
pub struct WorkNoticeStore {
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
            entries: RwLock::new(Vec::new()),
            alerts: RwLock::new(Vec::new()),
            reminded: RwLock::new(HashSet::new()),
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
        true
    }

    /// 取该角色尚未转告过的完成报告，取走即移除。
    pub fn take_reports_for(&self, char_id: &str) -> Vec<WorkReport> {
        if char_id.is_empty() {
            return Vec::new();
        }
        let now = chrono::Local::now().timestamp() as f64;
        let mut entries = self.entries.write();
        Self::prune_locked(&mut entries, now);
        let mut out = Vec::new();
        entries.retain(|e| {
            if e.char_id == char_id {
                out.push(e.report.clone());
                false
            } else {
                true
            }
        });
        out
    }

    /// 登记一条「必须转达」——工作智能体已显式要求把这件事告诉用户。
    ///
    /// 同一会话的旧条目会被顶掉：同一会话连续要求转达时，最新那条才代表现状。
    pub fn push_alert(&self, char_id: &str, session_id: &str, title: &str, body: &str) -> bool {
        let body = body.trim();
        if char_id.is_empty() || body.is_empty() {
            return false;
        }
        let title = if title.trim().is_empty() { "工作进展" } else { title.trim() };
        let now = chrono::Local::now().timestamp() as f64;

        let mut alerts = self.alerts.write();
        Self::prune_alerts_locked(&mut alerts, now);
        alerts.retain(|a| !(a.char_id == char_id && a.alert.session_id == session_id));
        alerts.push(AlertEntry {
            char_id: char_id.to_string(),
            alert: WorkAlert {
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
        alerts
            .iter()
            .filter(|a| a.char_id == char_id)
            .min_by(|a, b| a.at.total_cmp(&b.at))
            .map(|a| a.alert.clone())
    }

    /// 一条「必须转达」已成功说出去，移除它。
    pub fn take_alert(&self, session_id: &str) {
        self.alerts
            .write()
            .retain(|a| a.alert.session_id != session_id);
    }

    /// 该角色是否还有待转告的完成报告。
    ///
    /// 只看不取：报告由提示词段落消费（`take_reports_for`），
    /// 这里只是让 tick 判断"值不值得给一次搭话机会"。
    pub fn has_report_for(&self, char_id: &str) -> bool {
        if char_id.is_empty() {
            return false;
        }
        let now = chrono::Local::now().timestamp() as f64;
        let mut entries = self.entries.write();
        Self::prune_locked(&mut entries, now);
        entries.iter().any(|e| e.char_id == char_id)
    }

    /// 会话被取消 / 重置时丢弃其未转告的报告与待转达项。
    pub fn drop_session(&self, session_id: &str) {
        self.entries
            .write()
            .retain(|e| e.report.session_id != session_id);
        self.alerts
            .write()
            .retain(|a| a.alert.session_id != session_id);
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

static STORE: Lazy<Arc<WorkNoticeStore>> = Lazy::new(|| Arc::new(WorkNoticeStore::new()));

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
