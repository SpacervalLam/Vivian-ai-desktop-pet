//! 角色反馈通道 — 系统事件通过角色对话传达，而非 toast
//!
//! - source 优先级体系：formal(3) > system(2) > passive(1)
//! - 正式对话进行时，passive 反馈排队等待
//! - 工具失败/系统错误转角色化台词
//!
//! 核心原则：任何让用户意识到"她其实是功能集合"的瞬间都是出戏。
//! 系统反馈应该通过角色的语气传达，而不是独立 UI 通知。

use std::collections::VecDeque;

use parking_lot::Mutex;
use serde::Serialize;

use super::presentation::PresentationPack;

/// 被动反馈队列状态
struct FeedbackChannelInner {
    /// 排队中的被动反馈（等待 formal 结束后释放）
    queue: VecDeque<PresentationPack>,
    /// 正式对话是否进行中
    is_formal_active: bool,
}

/// 角色反馈通道
///
/// 管理 passive 反馈的排队与释放。
/// 正式对话进行时，passive 反馈入队等待；
/// formal 结束后批量释放，通过 chat:passive 事件推送前端。
pub struct FeedbackChannel {
    inner: Mutex<FeedbackChannelInner>,
}

impl FeedbackChannel {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(FeedbackChannelInner {
                queue: VecDeque::new(),
                is_formal_active: false,
            }),
        }
    }

    /// 标记正式对话开始
    pub fn formal_start(&self) {
        let mut inner = self.inner.lock();
        inner.is_formal_active = true;
    }

    /// 标记正式对话结束，返回排队的被动反馈
    pub fn formal_end(&self) -> Vec<PresentationPack> {
        let mut inner = self.inner.lock();
        inner.is_formal_active = false;
        inner.queue.drain(..).collect()
    }

    /// 入队被动反馈
    ///
    /// 如果 formal 正在进行，入队等待；否则立即返回（调用方应直接推送）。
    /// 返回 Some(pack) 表示需要立即推送，None 表示已入队。
    pub fn enqueue_passive(&self, pack: PresentationPack) -> Option<PresentationPack> {
        let mut inner = self.inner.lock();
        if inner.is_formal_active {
            inner.queue.push_back(pack);
            // 限制队列长度，避免无限增长
            while inner.queue.len() > 6 {
                inner.queue.pop_front();
            }
            None
        } else {
            Some(pack)
        }
    }

    /// 当前队列长度
    pub fn queue_len(&self) -> usize {
        self.inner.lock().queue.len()
    }

    /// 是否有排队中的反馈
    pub fn has_pending(&self) -> bool {
        !self.inner.lock().queue.is_empty()
    }

    /// 清空队列
    pub fn clear(&self) {
        self.inner.lock().queue.clear();
    }
}

impl Default for FeedbackChannel {
    fn default() -> Self {
        Self::new()
    }
}

/// 工具失败转角色化台词
///
/// 把工具执行失败的信息转化为 Vivian 语气的话，
/// 而不是冷冰冰的"工具执行失败"。
pub fn tool_failure_to_character_text(tool: &str, _error: &str) -> String {
    match tool {
        "search_memory" => {
            "嗯…我想不起来了一些事情呢。".to_string()
        }
        "save_memory" => {
            "这个…我没能记住呢，可以再说一次吗？".to_string()
        }
        "web_search" | "web_fetch" => {
            "网络好像有点问题，查不到呢…".to_string()
        }
        "create_todo" | "update_todo" | "delete_todo" => {
            "待办事项没操作成功呢，稍后再试试？".to_string()
        }
        "play_music" | "control_music" => {
            "音乐控制出了点小问题…".to_string()
        }
        _ => {
            String::new()
        }
    }
}

/// 系统错误转角色化台词
///
/// 把系统级错误（API 配置、网络异常等）转化为角色语气。
pub fn system_error_to_character_text(error_type: &str) -> String {
    match error_type {
        "no_main_api" => {
            "我好像还不太能说话呢…需要先配置好才能聊天哦。".to_string()
        }
        "invalid_api_key" => {
            "API Key 好像不对呢，去设置里检查一下吧。".to_string()
        }
        "insufficient_balance" => {
            "账户余额不足了哦，需要充值后才能继续聊天。".to_string()
        }
        "api_quota_exceeded" => {
            "今天说得有点多了，稍后再聊吧？".to_string()
        }
        "rate_limited" => {
            "等一下下哦，太快了我有点跟不上。".to_string()
        }
        "network_error" => {
            "信号不太好呢，听不太清楚…".to_string()
        }
        "timeout" => {
            "让我再想想…刚才的话好像没说完。".to_string()
        }
        "model_not_found" => {
            "配置的模型好像不存在呢，去设置里看看吧。".to_string()
        }
        "context_length" => {
            "聊得太久了，之前的事情我有点记不住了…开个新话题吧？".to_string()
        }
        "content_policy" => {
            "这个话题我不太能聊呢，换个话题吧？".to_string()
        }
        "server_error" | "overloaded" => {
            "服务器有点忙，等一下下再试试吧。".to_string()
        }
        "circuit_breaker" => {
            "最近出错太多了，先休息一下再聊吧。".to_string()
        }
        _ => {
            "出了点小问题，不过没关系，我在呢。".to_string()
        }
    }
}

/// 被动反馈事件（通过 Tauri 事件推送给前端）
#[derive(Debug, Clone, Serialize)]
pub struct PassiveFeedbackEvent {
    /// 角色台词
    pub text: String,
    /// 来源类型
    pub feedback_type: String,
    /// 时间戳
    pub timestamp: f64,
}

impl PassiveFeedbackEvent {
    pub fn new(text: String, feedback_type: impl Into<String>) -> Self {
        Self {
            text,
            feedback_type: feedback_type.into(),
            timestamp: chrono::Local::now().timestamp_millis() as f64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_formal_blocks_passive() {
        let ch = FeedbackChannel::new();
        ch.formal_start();
        let pack = PresentationPack::passive("test".to_string());
        assert!(ch.enqueue_passive(pack).is_none()); // 入队
        assert_eq!(ch.queue_len(), 1);
    }

    #[test]
    fn test_formal_end_flushes_queue() {
        let ch = FeedbackChannel::new();
        ch.formal_start();
        ch.enqueue_passive(PresentationPack::passive("msg1".to_string()));
        ch.enqueue_passive(PresentationPack::passive("msg2".to_string()));
        let flushed = ch.formal_end();
        assert_eq!(flushed.len(), 2);
        assert_eq!(flushed[0].text, "msg1");
        assert_eq!(flushed[1].text, "msg2");
        assert!(!ch.has_pending());
    }

    #[test]
    fn test_no_formal_returns_immediately() {
        let ch = FeedbackChannel::new();
        let pack = PresentationPack::passive("test".to_string());
        let result = ch.enqueue_passive(pack);
        assert!(result.is_some());
        assert_eq!(ch.queue_len(), 0);
    }

    #[test]
    fn test_queue_max_length() {
        let ch = FeedbackChannel::new();
        ch.formal_start();
        for i in 0..10 {
            ch.enqueue_passive(PresentationPack::passive(format!("msg{i}")));
        }
        assert_eq!(ch.queue_len(), 6); // 上限 6
    }

    #[test]
    fn test_tool_failure_character_text() {
        let text = tool_failure_to_character_text("search_memory", "not found");
        assert!(text.contains("想不起来"));
        assert!(!text.contains("not found"));
    }

    #[test]
    fn test_system_error_character_text() {
        let text = system_error_to_character_text("no_main_api");
        assert!(text.contains("配置"));
        assert!(!text.contains("no_main_api"));
    }

    #[test]
    fn test_unknown_error_fallback() {
        let text = system_error_to_character_text("unknown_error_xyz");
        assert!(!text.is_empty());
    }
}
