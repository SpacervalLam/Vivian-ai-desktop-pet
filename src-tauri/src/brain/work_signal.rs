//! work 侧 → companion 侧的唯一转达事实视图。
//!
//! 这里只做**纯派生**：读任务注册表与三个事实源，产出一份结构化信号，
//! 不写任何业务状态、不决定要不要说、不决定由谁说。
//! 「什么时候开口」由 `proactive::relay_work_notice` 决定；
//! 「说出去没有」由宿主在真实投递后按事件 id 确认。
//!
//! 为什么不再往主 prompt 里塞「后台任务」段：那会把工作事实放进角色的
//! Current Mind，还要靠语气文案防止她把工作说成自己干的；而且领取发生在
//! prompt 构建时、确认发生在主对话生成后，主动生成那条路径根本没有确认点。

use serde::Serialize;
use std::collections::HashMap;

/// 一次最多带几件事进 relay 的上下文。多了只会让模型复述成报告腔。
const MAX_EVENTS: usize = 8;
const REPORT_BODY_CHARS: usize = 1200;
const SHORT_BODY_CHARS: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkSignalKind {
    /// 工作侧显式要求转达（含桌宠委派的阶段回调）。
    Alert,
    /// 卡在用户拍板上的提问，派生自问题注册表。
    Attention,
    /// 新任务系统里等输入 / 待验收 / 未送达失败的任务。
    Task,
    /// 工作完成报告。
    Report,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkSignalEvent {
    pub kind: WorkSignalKind,
    /// 带前缀的稳定标识。投递成功后据此精确确认；也是缓存签名的一部分。
    /// `alert:` / `report:` 需要宿主确认，`attention:` / `task:` 靠源状态消失，不需确认。
    pub id: String,
    pub session_id: String,
    pub title: String,
    pub body: String,
    /// 需要用户做什么。只写动作，不替用户决定。
    pub needs: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u32>,
    /// 只有 alert 有：required 会绕过可见性与冷却门控。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    /// 投递优先级，越小越急。不进 facts。
    #[serde(skip)]
    pub rank: u8,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct WorkSignal {
    pub events: Vec<WorkSignalEvent>,
    /// 后台还有活没干完：只是防「宣称完成」的护栏，不是要播报的事件。
    pub has_running_work: bool,
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

impl WorkSignal {
    /// 派生该角色当前所有待转达事实。不写状态。
    pub fn derive(char_id: &str) -> Self {
        if char_id.is_empty() {
            return Self::default();
        }
        let notices = crate::brain::work_notices::global();
        let mut events: Vec<WorkSignalEvent> = Vec::new();

        // 1. 必须转达 / 桌宠回调：已有精确确认语义，保持最高优先级。
        if let Some(alert) = notices.next_alert_for(char_id) {
            let required = alert.required;
            events.push(WorkSignalEvent {
                kind: WorkSignalKind::Alert,
                id: format!("alert:{}", alert.notice_id),
                session_id: alert.session_id,
                title: clip(&alert.title, SHORT_BODY_CHARS),
                body: clip(&alert.body, REPORT_BODY_CHARS),
                needs: if required {
                    "把这件事如实告诉用户".into()
                } else {
                    "顺口提一句".into()
                },
                revision: None,
                required: Some(required),
                rank: if required { 0 } else { 4 },
            });
        }

        // 2. 工作完成报告：只读派生，投递成功后按事件 id 精确删除。
        for (id, report) in notices.reports_for(char_id) {
            events.push(WorkSignalEvent {
                kind: WorkSignalKind::Report,
                id: format!("report:{id}"),
                session_id: report.session_id,
                title: clip(&report.title, SHORT_BODY_CHARS),
                body: clip(&report.body, REPORT_BODY_CHARS),
                needs: "准确保留结果、验证与未完成项，别改写成更乐观的结论".into(),
                revision: None,
                required: None,
                rank: 3,
            });
        }

        // 3. 等你拍板：派生自问题注册表，用户回答后自然消失。
        for question in crate::brain::work_notices::pending_attention_for(char_id) {
            let mut body = clip(&question.question, SHORT_BODY_CHARS);
            if let Some(ctx) = &question.context {
                let ctx = clip(ctx, SHORT_BODY_CHARS);
                if !ctx.trim().is_empty() {
                    body.push_str("\n背景：");
                    body.push_str(&ctx);
                }
            }
            events.push(WorkSignalEvent {
                kind: WorkSignalKind::Attention,
                id: format!("attention:{}", question.question_id),
                session_id: question.session_id,
                title: "等你拍板".into(),
                body,
                needs: "用一句话提醒用户去工作页看一眼；别替他做选择，别把选项念一遍".into(),
                revision: None,
                required: None,
                rank: 1,
            });
        }

        // 4. 新任务系统：只有真的需要人介入的才派生。
        //    WorkJob 上没有 char_id（会话属性不该塞进任务实体），在这里一次性
        //    建立 session → character 映射，避免在状态变化钩子里回查注册表。
        let sessions = crate::commands::coding_agent::CODING_AGENT.list_sessions();
        let mut owners: HashMap<String, String> = HashMap::new();
        let mut has_running_work = false;
        for session in &sessions {
            owners.insert(session.session_id.clone(), session.char_id.clone());
            if session.char_id == char_id
                && session.status == crate::brain::coding_agent::CodingStatus::Running
            {
                has_running_work = true;
            }
        }
        for job in crate::brain::work_jobs::global_work_job_registry().attention() {
            if owners.get(&job.session_id).map(String::as_str) != Some(char_id) {
                continue;
            }
            let (title, needs, rank) = match job.status.as_str() {
                "needs_input" => (
                    "等你补充信息",
                    "把卡住的那一个问题告诉用户，请他去工作页回答",
                    0,
                ),
                "ready_for_review" => (
                    "结果待验收",
                    "告诉用户结果已就绪，请他按当前版本确认；别替他验收",
                    1,
                ),
                "failed" => ("任务未跑完", "如实说明失败，别当成已完成", 2),
                _ => continue,
            };
            let mut body = clip(&job.task, SHORT_BODY_CHARS);
            if let Some(detail) = job.blocker.as_deref().or(job.review_reason.as_deref()) {
                let detail = clip(detail, SHORT_BODY_CHARS);
                if !detail.trim().is_empty() {
                    body.push_str("\n");
                    body.push_str(&detail);
                }
            }
            events.push(WorkSignalEvent {
                kind: WorkSignalKind::Task,
                id: format!("task:{}:{}", job.job_id, job.result_revision),
                session_id: job.session_id,
                title: title.into(),
                body,
                needs: needs.into(),
                revision: Some(job.result_revision),
                required: None,
                rank,
            });
        }

        events.sort_by(|a, b| a.rank.cmp(&b.rank).then_with(|| a.id.cmp(&b.id)));
        events.truncate(MAX_EVENTS);
        Self {
            events,
            has_running_work,
        }
    }

    /// 缓存签名：只看事件身份与运行护栏，不看文案措辞。
    /// 事件变了（新报告 / 新版本 / 新问题）才算新素材，避免同一份事实反复调模型。
    pub fn signature(&self) -> String {
        let mut ids: Vec<&str> = self.events.iter().map(|e| e.id.as_str()).collect();
        ids.sort_unstable();
        format!("{}|{}", ids.join(","), self.has_running_work)
    }

    pub fn facts_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    /// 这一轮该说哪件事。按优先级取第一条，其余留给下一个 tick。
    pub fn highest(&self) -> Option<&WorkSignalEvent> {
        self.events.first()
    }

    /// 用户看得见这条素材所属会话时，是否仍必须转达。
    pub fn should_relay(&self, event: &WorkSignalEvent, visible_session: Option<&str>, cooled: bool) -> bool {
        if event.required == Some(true) {
            return true;
        }
        !cooled && visible_session != Some(event.session_id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: &str, kind: WorkSignalKind, rank: u8) -> WorkSignalEvent {
        WorkSignalEvent {
            kind,
            id: id.into(),
            session_id: "s1".into(),
            title: "标题".into(),
            body: "事实".into(),
            needs: "提醒用户".into(),
            revision: None,
            required: None,
            rank,
        }
    }

    #[test]
    fn highest_prefers_the_most_urgent_event() {
        let mut signal = WorkSignal::default();
        signal.events.push(event("report:1", WorkSignalKind::Report, 3));
        signal.events.push(event("task:2:1", WorkSignalKind::Task, 0));
        signal.events.sort_by(|a, b| a.rank.cmp(&b.rank).then_with(|| a.id.cmp(&b.id)));
        assert_eq!(signal.highest().unwrap().id, "task:2:1");
    }

    #[test]
    fn signature_changes_when_an_event_changes_but_not_when_wording_does() {
        let mut a = WorkSignal::default();
        a.events.push(event("task:2:1", WorkSignalKind::Task, 0));
        let mut b = a.clone();
        b.events[0].body = "换了个说法".into();
        assert_eq!(a.signature(), b.signature());
        b.events[0].id = "task:2:2".into();
        assert_ne!(a.signature(), b.signature());
        b.has_running_work = true;
        assert_ne!(a.signature(), b.signature());
    }

    #[test]
    fn required_bypasses_visibility_and_cooldown_others_do_not() {
        let mut required = event("alert:1", WorkSignalKind::Alert, 0);
        required.required = Some(true);
        let plain = event("task:1:1", WorkSignalKind::Task, 1);
        let signal = WorkSignal::default();
        assert!(signal.should_relay(&required, Some("s1"), true));
        assert!(!signal.should_relay(&plain, Some("s1"), false));
        assert!(!signal.should_relay(&plain, None, true));
        assert!(signal.should_relay(&plain, None, false));
    }

    #[test]
    fn facts_json_omits_internal_rank_and_keeps_running_guard() {
        let mut signal = WorkSignal {
            has_running_work: true,
            ..Default::default()
        };
        signal.events.push(event("task:1:1", WorkSignalKind::Task, 0));
        let value: serde_json::Value = serde_json::from_str(&signal.facts_json()).unwrap();
        assert_eq!(value["has_running_work"], true);
        assert!(value["events"][0].get("rank").is_none());
        assert_eq!(value["events"][0]["kind"], "task");
    }
}
