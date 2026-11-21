//! 用户提问注册表 — 让模型在工具调用中向用户提问并等待自由文本回答。
//!
//! 与 `ToolConfirmationRegistry`（三态 Allow/Deny 的工具权限确认）不同，这里是
//! **开放问答**：工具广播 `chat:question` 事件给前端，前端弹输入框，用户回答后
//! 经 Tauri 命令回传，注册表通过 oneshot channel 把答案交回正在等待的工具。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::oneshot;

/// 单调递增的问题 ID
static NEXT_QUESTION_ID: AtomicU64 = AtomicU64::new(1);

/// pending 问题的 TTL：超过 10 分钟未响应自动清理
const QUESTION_TTL: Duration = Duration::from_secs(10 * 60);

/// 发送给前端的提问请求 payload
#[derive(Debug, Clone, Serialize)]
pub struct QuestionRequest {
    /// 唯一问题 ID，前端回传时需要带上
    pub question_id: u64,
    /// 模型提出的问题正文
    pub prompt: String,
    /// 可选的答案格式提示（如"数字"/"路径"/"是/否"），前端可据此调整输入框
    pub hint: Option<String>,
    /// 发起提问的角色 ID（多角色场景前端按此路由到对应窗口）
    pub char_id: String,
}

/// pending 问题条目
struct PendingQuestion {
    sender: oneshot::Sender<String>,
    created_at: Instant,
    request: QuestionRequest,
}

/// 用户提问注册表
///
/// 线程安全，可被多个工具执行流共享。带 TTL 惰性清理。
pub struct QuestionRegistry {
    pending: Mutex<HashMap<u64, PendingQuestion>>,
}

impl QuestionRegistry {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// 创建一个提问请求，返回 `(question_id, receiver)`。
    ///
    /// - `question_id`：用于 emit 事件给前端
    /// - `receiver`：await 获取用户回答文本；用户在 TTL 内未响应则收到 `Err`
    pub fn create_question(
        &self,
        prompt: String,
        hint: Option<String>,
        char_id: String,
    ) -> (u64, oneshot::Receiver<String>) {
        let id = NEXT_QUESTION_ID.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.cleanup_expired_locked();
        self.pending.lock().insert(
            id,
            PendingQuestion {
                sender: tx,
                created_at: Instant::now(),
                request: QuestionRequest {
                    question_id: id,
                    prompt,
                    hint,
                    char_id,
                },
            },
        );
        (id, rx)
    }

    /// 列出所有 pending 问题（供远程端轮询展示）。
    pub fn list_pending(&self) -> Vec<QuestionRequest> {
        self.cleanup_expired_locked();
        let pending = self.pending.lock();
        let mut list: Vec<QuestionRequest> =
            pending.values().map(|e| e.request.clone()).collect();
        list.sort_by_key(|r| r.question_id);
        list
    }

    /// 回传用户回答，唤醒正在等待的工具。
    ///
    /// 请求不存在（已超时或重复回答）时返回 false。
    pub fn respond(&self, question_id: u64, answer: String) -> bool {
        if let Some(entry) = self.pending.lock().remove(&question_id) {
            let _ = entry.sender.send(answer);
            true
        } else {
            false
        }
    }

    /// 当前 pending 问题数（诊断用）。
    pub fn pending_count(&self) -> usize {
        self.cleanup_expired_locked();
        self.pending.lock().len()
    }

    /// 惰性清理过期问题。
    fn cleanup_expired_locked(&self) {
        let mut pending = self.pending.lock();
        let now = Instant::now();
        pending.retain(|_, e| now.duration_since(e.created_at) < QUESTION_TTL);
    }
}

impl Default for QuestionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 便捷函数：把注册表作为 AppState 的全局单例暴露（避免重复初始化）。
static QUESTION_REGISTRY: once_cell::sync::Lazy<Arc<QuestionRegistry>> =
    once_cell::sync::Lazy::new(|| Arc::new(QuestionRegistry::new()));

/// 全局注册表访问器（供工具与命令共用同一实例）。
pub fn global_question_registry() -> Arc<QuestionRegistry> {
    Arc::clone(&QUESTION_REGISTRY)
}
