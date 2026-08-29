//! 话题追踪 — 冷却管理与话题活跃度衰减。
//!
//! 纯机械化管理，语义判断由 LLM 完成。

// ===== 常量 =====

/// 默认话题活跃度
const DEFAULT_ACTIVENESS: i32 = 10;

/// 默认称呼冷却轮数
const DEFAULT_COOLDOWN_TURNS: i32 = 4;

/// 默认用户称呼
const DEFAULT_USER_NAME: &str = "Master";

// ===== 话题追踪器 =====

/// 话题追踪器 — 追踪称呼冷却 / 话题活跃度衰减（纯机械）
///
/// - 用户称呼管理（`user_name`）
/// - 称呼冷却管理（`name_call_cooldown`，默认 4 轮）
/// - 话题活跃度（`topic_activeness`，每轮机械衰减，下限 0）
pub struct TopicTracker {
    /// 话题活跃度（每轮机械衰减，下限 0）
    pub topic_activeness: i32,
    /// 最近话题发起者
    pub last_topic_initiator: String,
    /// 称呼冷却剩余轮数
    pub name_call_cooldown: i32,
    /// 用户称呼
    pub user_name: String,
}

impl TopicTracker {
    /// 创建新实例
    ///
    /// `user_name` 为 `None` 或空字符串时使用默认称呼 "Master"。
    pub fn new(user_name: Option<String>) -> Self {
        Self {
            topic_activeness: DEFAULT_ACTIVENESS,
            last_topic_initiator: "user".to_string(),
            name_call_cooldown: 0,
            user_name: user_name
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| DEFAULT_USER_NAME.to_string()),
        }
    }

    // ===== 用户称呼 =====

    /// 设置用户称呼（空字符串回退为 "Master"）
    pub fn set_user_name(&mut self, name: &str) {
        self.user_name = if name.is_empty() {
            DEFAULT_USER_NAME.to_string()
        } else {
            name.to_string()
        };
    }

    /// 获取用户称呼
    pub fn get_user_name(&self) -> &str {
        &self.user_name
    }

    // ===== 冷却管理 =====

    /// 是否处于称呼冷却中
    pub fn is_in_cooldown(&self) -> bool {
        self.name_call_cooldown > 0
    }

    /// 冷却计数递减（不低于 0）
    pub fn decrement_cooldown(&mut self) {
        if self.name_call_cooldown > 0 {
            self.name_call_cooldown -= 1;
        }
    }

    /// 重置冷却为指定轮数
    pub fn reset_cooldown(&mut self, turns: i32) {
        self.name_call_cooldown = turns;
    }

    /// 检查回复中是否包含用户名，并更新冷却
    ///
    /// - 包含用户名：重置冷却为默认轮数（4），返回 `true`
    /// - 不包含：冷却递减，返回 `false`
    pub fn check_and_update_cooldown(&mut self, reply: &str) -> bool {
        let contains_name =
            !self.user_name.is_empty() && reply.contains(self.user_name.as_str());
        if contains_name {
            self.reset_cooldown(DEFAULT_COOLDOWN_TURNS);
        } else {
            self.decrement_cooldown();
        }
        contains_name
    }

    // ===== 话题活跃度（纯机械，无语义）=====

    /// 每轮对话机械衰减 topic_activeness（下限 0）
    pub fn decay_activeness(&mut self) {
        if self.topic_activeness > 0 {
            self.topic_activeness -= 1;
        }
    }

    /// 话题是否活跃
    pub fn is_topic_active(&self) -> bool {
        self.topic_activeness > 0
    }

    /// 重置话题活跃度为默认值（10）
    pub fn reset_topic_activeness(&mut self) {
        self.topic_activeness = DEFAULT_ACTIVENESS;
    }

    /// 获取话题活跃度
    pub fn get_topic_activeness(&self) -> i32 {
        self.topic_activeness
    }
}

impl Default for TopicTracker {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_user_name_is_master() {
        let t = TopicTracker::new(None);
        assert_eq!(t.user_name, "Master");
    }

    #[test]
    fn custom_user_name() {
        let t = TopicTracker::new(Some("Vivian".to_string()));
        assert_eq!(t.user_name, "Vivian");
    }

    #[test]
    fn empty_user_name_falls_back() {
        let t = TopicTracker::new(Some(String::new()));
        assert_eq!(t.user_name, "Master");
    }

    #[test]
    fn cooldown_resets_on_name_match() {
        let mut t = TopicTracker::new(Some("Master".to_string()));
        // 先递减一次使冷却进入非零状态前的检查
        assert!(t.check_and_update_cooldown("Hello Master!"));
        assert!(t.is_in_cooldown());
        assert_eq!(t.name_call_cooldown, DEFAULT_COOLDOWN_TURNS);
    }

    #[test]
    fn cooldown_decrements_on_no_match() {
        let mut t = TopicTracker::new(Some("Master".to_string()));
        t.reset_cooldown(3);
        assert!(!t.check_and_update_cooldown("hello there"));
        assert_eq!(t.name_call_cooldown, 2);
    }

    #[test]
    fn activeness_decays_to_zero() {
        let mut t = TopicTracker::new(None);
        assert!(t.is_topic_active());
        for _ in 0..DEFAULT_ACTIVENESS {
            t.decay_activeness();
        }
        assert_eq!(t.topic_activeness, 0);
        assert!(!t.is_topic_active());
        // 下限 0，不再继续衰减
        t.decay_activeness();
        assert_eq!(t.topic_activeness, 0);
    }

    #[test]
    fn reset_topic_activeness_restores_default() {
        let mut t = TopicTracker::new(None);
        t.topic_activeness = 0;
        t.reset_topic_activeness();
        assert_eq!(t.topic_activeness, DEFAULT_ACTIVENESS);
    }
}
