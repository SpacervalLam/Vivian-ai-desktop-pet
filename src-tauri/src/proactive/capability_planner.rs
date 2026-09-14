//! Drive → Capability 决策点
//!
//! 把 BehaviorDrive（8 项行为驱动）映射为 ProactiveTrigger 的有序候选列表。
//! 这是 Behavior Planner 的核心：根据当前主导驱动决定「是否行动」「按什么顺序尝试 Capability」。
//!
//! 设计原则：
//! - 心理学系统未注入时回退到 ProactiveTrigger::all() 静态优先级，不破坏既有行为
//! - 主导驱动强度低于阈值时同样回退（避免无意义重排）
//! - Observe / Rest / Avoid 主导时 skip_action=true：不打扰用户，但内心独白仍由 tick 调度
//! - 其余驱动按语义重排触发器：Approach 优先 WelcomeBack，Play 优先 TeasingResponse，Help 优先 HealthReminder 等
//!
//! Capability 选择 ≠ Tool 选择：这里只决定「她想做什么类型的事」，
//! 具体调用哪个 Tool 仍由 LLM 在生成内容时自主决定。

use crate::psychology::{BehaviorDrive, DriveLabel};

use super::triggers::ProactiveTrigger;

/// 一次 tick 的能力规划结果
#[derive(Debug, Clone)]
pub struct CapabilityPlan {
    /// 主导驱动标签（None 表示心理学未注入）
    pub drive_label: Option<DriveLabel>,
    /// 主导驱动强度（0.0-1.0）
    pub drive_strength: f64,
    /// 按驱动重排后的触发器候选（已与配置启用的触发器求交集）
    pub ordered_triggers: Vec<ProactiveTrigger>,
    /// true = 本次 tick 不主动发声（但内心独白可继续）
    pub skip_action: bool,
    /// 决策依据（日志/调试用）
    pub rationale: &'static str,
}

/// 能力规划器
pub struct CapabilityPlanner;

impl CapabilityPlanner {
    /// 主导驱动强度阈值：低于此值视为无明显驱动，回退静态优先级
    const DOMINANT_DRIVE_THRESHOLD: f64 = 0.35;

    /// 根据当前 BehaviorDrive 生成能力规划
    ///
    /// `drive=None`（心理学系统未注入）时回退到 legacy 静态优先级。
    pub fn plan(drive: Option<&BehaviorDrive>) -> CapabilityPlan {
        let Some(drive) = drive else {
            return CapabilityPlan {
                drive_label: None,
                drive_strength: 0.0,
                ordered_triggers: Self::legacy_priority_order(),
                skip_action: false,
                rationale: "psychology_not_injected",
            };
        };

        let (label, value) = drive.dominant();

        if value < Self::DOMINANT_DRIVE_THRESHOLD {
            return CapabilityPlan {
                drive_label: Some(label),
                drive_strength: value,
                ordered_triggers: Self::legacy_priority_order(),
                skip_action: false,
                rationale: "drive_below_threshold",
            };
        }

        // Drive → Capability 语义映射
        // 观察/休息/回避三种「内向」驱动 → 不主动打扰，但内心独白仍可运行
        let (ordered, skip, rationale) = match label {
            DriveLabel::Approach => (
                vec![
                    ProactiveTrigger::WelcomeBack,
                    ProactiveTrigger::Spontaneous,
                    ProactiveTrigger::IdleGreeting,
                    ProactiveTrigger::HourlyGreeting,
                ],
                false,
                "approach",
            ),
            DriveLabel::Express => (
                vec![
                    ProactiveTrigger::Spontaneous,
                    ProactiveTrigger::Icebreaker,
                    ProactiveTrigger::HourlyGreeting,
                    ProactiveTrigger::TopicExtension,
                ],
                false,
                "express",
            ),
            DriveLabel::Play => (
                vec![
                    ProactiveTrigger::TeasingResponse,
                    ProactiveTrigger::Spontaneous,
                    ProactiveTrigger::Icebreaker,
                ],
                false,
                "play",
            ),
            DriveLabel::Help => (
                vec![
                    ProactiveTrigger::HealthReminder,
                    ProactiveTrigger::MemoryRecall,
                ],
                false,
                "help",
            ),
            DriveLabel::Explore => (
                vec![
                    ProactiveTrigger::TopicExtension,
                    ProactiveTrigger::MemoryRecall,
                    ProactiveTrigger::WindowTrigger,
                ],
                false,
                "explore",
            ),
            DriveLabel::Observe => (Vec::new(), true, "observe_silent"),
            DriveLabel::Rest => (Vec::new(), true, "rest_silent"),
            DriveLabel::Avoid => (Vec::new(), true, "avoid_retreat"),
        };

        CapabilityPlan {
            drive_label: Some(label),
            drive_strength: value,
            ordered_triggers: ordered,
            skip_action: skip,
            rationale,
        }
    }

    /// 静态优先级（与 ProactiveTrigger::all() 顺序一致）
    fn legacy_priority_order() -> Vec<ProactiveTrigger> {
        ProactiveTrigger::all().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::psychology::{DriveSource, BehaviorDrive};

    fn make_drive(approach: f64, express: f64, play: f64, help: f64, observe: f64, rest: f64, avoid: f64, explore: f64) -> BehaviorDrive {
        BehaviorDrive {
            approach,
            avoid,
            explore,
            express,
            rest,
            observe,
            play,
            help,
            source: DriveSource::Rule,
        }
    }

    #[test]
    fn plan_without_drive_falls_back_to_legacy() {
        let plan = CapabilityPlanner::plan(None);
        assert!(plan.drive_label.is_none());
        assert!(!plan.skip_action);
        assert_eq!(plan.ordered_triggers.len(), ProactiveTrigger::all().len());
        assert_eq!(plan.rationale, "psychology_not_injected");
    }

    #[test]
    fn plan_below_threshold_falls_back_to_legacy() {
        let drive = make_drive(0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1);
        let plan = CapabilityPlanner::plan(Some(&drive));
        assert!(plan.drive_label.is_some());
        assert!(!plan.skip_action);
        assert_eq!(plan.ordered_triggers.len(), ProactiveTrigger::all().len());
        assert_eq!(plan.rationale, "drive_below_threshold");
    }

    #[test]
    fn approach_dominant_prioritizes_welcome_back() {
        let drive = make_drive(0.8, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1);
        let plan = CapabilityPlanner::plan(Some(&drive));
        assert_eq!(plan.drive_label, Some(DriveLabel::Approach));
        assert!(!plan.skip_action);
        assert_eq!(plan.ordered_triggers.first(), Some(&ProactiveTrigger::WelcomeBack));
        assert_eq!(plan.rationale, "approach");
    }

    #[test]
    fn help_dominant_prioritizes_health_reminder() {
        let drive = make_drive(0.1, 0.1, 0.1, 0.9, 0.1, 0.1, 0.1, 0.1);
        let plan = CapabilityPlanner::plan(Some(&drive));
        assert_eq!(plan.drive_label, Some(DriveLabel::Help));
        assert_eq!(plan.ordered_triggers.first(), Some(&ProactiveTrigger::HealthReminder));
    }

    #[test]
    fn observe_dominant_skips_action() {
        let drive = make_drive(0.1, 0.1, 0.1, 0.1, 0.9, 0.1, 0.1, 0.1);
        let plan = CapabilityPlanner::plan(Some(&drive));
        assert!(plan.skip_action);
        assert!(plan.ordered_triggers.is_empty());
        assert_eq!(plan.rationale, "observe_silent");
    }

    #[test]
    fn rest_dominant_skips_action() {
        let drive = make_drive(0.1, 0.1, 0.1, 0.1, 0.1, 0.9, 0.1, 0.1);
        let plan = CapabilityPlanner::plan(Some(&drive));
        assert!(plan.skip_action);
        assert_eq!(plan.rationale, "rest_silent");
    }

    #[test]
    fn avoid_dominant_skips_action() {
        let drive = make_drive(0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.9, 0.1);
        let plan = CapabilityPlanner::plan(Some(&drive));
        assert!(plan.skip_action);
        assert_eq!(plan.rationale, "avoid_retreat");
    }
}
