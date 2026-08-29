//! 表现包结构化输出
//!
//! - 把一次表现的 text/motion/expression/intent/control_actions 打包成原子单元
//! - source 优先级体系（formal > system > passive）
//! - normalize_with() 用指定 manifest 钳制 LLM 幻觉到实际资源

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::manifest::ResourceManifest;

/// 表现来源 — 决定优先级调度
///
/// formal(3) > system(2) > passive(1)
/// 正式对话进行时，passive 反馈排队等待。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PresentationSource {
    /// 正式对话回复（用户发消息触发）
    Formal,
    /// 系统消息（配置错误、API 异常等）
    System,
    /// 被动反馈（工具失败、后台任务完成等）
    Passive,
}

impl PresentationSource {
    /// 优先级数值（越大越优先）
    pub fn priority(&self) -> u8 {
        match self {
            Self::Formal => 3,
            Self::System => 2,
            Self::Passive => 1,
        }
    }
}

impl Default for PresentationSource {
    fn default() -> Self {
        Self::Formal
    }
}

/// 表现包 — 一次表现的完整描述
///
/// 把分散在 PipelineState 的 text/motion/expression/intent/control_actions
/// 打包成原子单元，保证一致性。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresentationPack {
    /// 回复文本（可为空，表示 no_reply）
    pub text: String,
    /// 表情名（经守门员归一化后）
    pub expression: String,
    /// 动作名（经守门员归一化后）
    pub motion: String,
    /// 意图标记：reply / short_reply / no_reply
    pub intent: String,
    /// 桌宠自控指令（由 ControlActionExecutor 执行）
    pub control_actions: Vec<Value>,
    /// 表现来源
    #[serde(default)]
    pub source: PresentationSource,
    /// 流式 ID（用于前端路由）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<String>,
    /// 情感分数（-1.0 ~ 1.0，用于前端展示）
    #[serde(default)]
    pub emotion_score: f64,
}

impl PresentationPack {
    /// 从 PipelineState 的输出层字段构建表现包
    ///
    /// 在 ResponseParsingRunnable 末尾调用，把分散字段打包。
    pub fn from_state_fields(
        text: String,
        expression: String,
        motion: String,
        intent: String,
        control_actions: Vec<Value>,
        emotion_score: f64,
    ) -> Self {
        Self {
            text,
            expression,
            motion,
            intent,
            control_actions,
            source: PresentationSource::Formal,
            stream_id: None,
            emotion_score,
        }
    }

    /// 构建被动反馈表现包（工具失败、后台通知等）
    pub fn passive(text: String) -> Self {
        Self {
            text,
            expression: String::new(),
            motion: String::new(),
            intent: "reply".to_string(),
            control_actions: Vec::new(),
            source: PresentationSource::Passive,
            stream_id: None,
            emotion_score: 0.0,
        }
    }

    /// 构建系统消息表现包（配置错误、API 异常等）
    pub fn system(text: String) -> Self {
        Self {
            text,
            expression: String::new(),
            motion: String::new(),
            intent: "reply".to_string(),
            control_actions: Vec::new(),
            source: PresentationSource::System,
            stream_id: None,
            emotion_score: 0.0,
        }
    }

    /// 用指定 manifest 归一化（用于测试或自定义 manifest）
    pub fn normalize_with(&mut self, m: &ResourceManifest) {
        self.expression = m.normalize_expression(&self.expression);
        self.motion = m.normalize_motion(&self.motion);
    }

    /// 是否为空表现（无文本、无表情、无动作）
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
            && self.expression.is_empty()
            && (self.motion.is_empty() || self.motion == "idle")
            && self.control_actions.is_empty()
    }

    /// 是否需要推送 chat:meta（有表情或非 idle 动作）
    pub fn needs_meta_event(&self) -> bool {
        !self.expression.is_empty() || (self.motion != "idle" && !self.motion.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_priority() {
        assert!(PresentationSource::Formal.priority() > PresentationSource::System.priority());
        assert!(PresentationSource::System.priority() > PresentationSource::Passive.priority());
    }

    #[test]
    fn test_from_state_fields() {
        let pack = PresentationPack::from_state_fields(
            "你好".to_string(),
            "shy".to_string(),
            "idle".to_string(),
            "reply".to_string(),
            vec![],
            0.5,
        );
        assert_eq!(pack.text, "你好");
        assert_eq!(pack.expression, "shy");
        assert_eq!(pack.source, PresentationSource::Formal);
    }

    #[test]
    fn test_passive_pack() {
        let pack = PresentationPack::passive("工具失败了".to_string());
        assert_eq!(pack.source, PresentationSource::Passive);
        assert_eq!(pack.text, "工具失败了");
    }

    #[test]
    fn test_needs_meta_event() {
        let mut pack = PresentationPack::from_state_fields(
            "hi".to_string(),
            "shy".to_string(),
            "idle".to_string(),
            "reply".to_string(),
            vec![],
            0.0,
        );
        assert!(pack.needs_meta_event()); // 有表情

        pack.expression = String::new();
        pack.motion = "test_motion".to_string();
        assert!(pack.needs_meta_event()); // 有非 idle 动作

        pack.motion = "idle".to_string();
        assert!(!pack.needs_meta_event()); // 无表情无动作
    }

    #[test]
    fn test_is_empty() {
        let pack = PresentationPack::from_state_fields(
            String::new(),
            String::new(),
            "idle".to_string(),
            "no_reply".to_string(),
            vec![],
            0.0,
        );
        assert!(pack.is_empty());
    }
}
