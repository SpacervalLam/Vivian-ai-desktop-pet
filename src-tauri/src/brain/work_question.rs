//! 工作智能体询问注册表 — 选择题式提问，挂起 agent loop 等待用户抉择。
//!
//! 与陪伴侧的 `tools::question`（自由文本 `ask_user`）是两套独立机制：这里面向
//! 编程会话，模型给出 2-4 个候选方向，用户在编程面板点选（或自己写、或跳过），
//! 答案经 oneshot channel 交回正在 `await` 的工具，loop 随即继续推进。
//!
//! 答案以**工具返回值**回流
//! （不伪装成新的用户消息，上下文保持 append-only、对 KV cache 友好）；等待
//! 期间不产生任何 token。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// 单调递增的问题 ID。
static NEXT_QUESTION_ID: AtomicU64 = AtomicU64::new(1);

/// pending 问题的 TTL：半小时未响应自动清理（回收 sender，避免 loop 永久挂起）。
///
/// 比陪伴侧的 10 分钟长得多：编程任务常常一轮要很久，用户可能暂时离开。
const WORK_QUESTION_TTL: Duration = Duration::from_secs(30 * 60);

/// 单个候选方向。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkQuestionOption {
    /// 选项标签（简短祈使句）。推荐项的约定是"放第一个 + 标签后缀（推荐）"，
    /// 不额外占 schema 字段。
    pub label: String,
    /// 该方向的一句话说明（展示给用户帮其判断）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// 广播给前端的提问请求 payload（`coding:question` 事件）。
#[derive(Debug, Clone, Serialize)]
pub struct WorkQuestionRequest {
    pub question_id: u64,
    pub session_id: String,
    pub question: String,
    /// 模型给出的判断依据 / 已尝试过什么，帮用户做决定。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    pub options: Vec<WorkQuestionOption>,
    pub multi_select: bool,
}

/// 用户的回答。
///
/// `selected` 存的是 **label** 而非下标——与 deepseek 一致，避免前端与后端
/// 对选项顺序产生耦合。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkQuestionAnswer {
    #[serde(default)]
    pub selected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

impl WorkQuestionAnswer {
    /// 用户是否跳过了这个问题（既没选也没自己写）——模型需自行选一个方向。
    pub fn is_skipped(&self) -> bool {
        self.selected.is_empty() && self.custom.as_deref().unwrap_or("").trim().is_empty()
    }

    /// 渲染成给模型看的文本。
    pub fn render(&self) -> String {
        if let Some(custom) = self.custom.as_deref() {
            let custom = custom.trim();
            if !custom.is_empty() {
                return if self.selected.is_empty() {
                    format!("用户回答：{custom}")
                } else {
                    format!("用户选择了「{}」，并补充：{custom}", self.selected.join("、"))
                };
            }
        }
        if self.selected.is_empty() {
            "用户跳过了这个问题，没有给出选择。请自行判断最合理的方向继续，并在最终总结里说明你选了哪条路。".to_string()
        } else {
            format!("用户选择了：{}", self.selected.join("、"))
        }
    }
}

/// pending 问题条目。
struct PendingWorkQuestion {
    sender: oneshot::Sender<WorkQuestionAnswer>,
    created_at: Instant,
    session_id: String,
    /// 发起该会话的角色 id。
    ///
    /// 提问要不要转告用户由陪伴侧决定，而陪伴侧是按角色组织的，
    /// 所以这里必须直接存下来——只存 session_id 的话，
    /// 想按角色取 pending 就得反过来去查工作会话表。
    char_id: String,
    request: WorkQuestionRequest,
}

/// 工作侧询问注册表（线程安全，带 TTL 惰性清理）。
pub struct WorkQuestionRegistry {
    pending: Mutex<HashMap<u64, PendingWorkQuestion>>,
}

impl WorkQuestionRegistry {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// 创建一个提问请求，返回 `(question_id, receiver)`。
    pub fn create_question(
        &self,
        char_id: String,
        session_id: String,
        question: String,
        context: Option<String>,
        options: Vec<WorkQuestionOption>,
        multi_select: bool,
    ) -> (u64, oneshot::Receiver<WorkQuestionAnswer>) {
        let id = NEXT_QUESTION_ID.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.cleanup_expired_locked();
        self.pending.lock().insert(
            id,
            PendingWorkQuestion {
                sender: tx,
                created_at: Instant::now(),
                session_id: session_id.clone(),
                char_id,
                request: WorkQuestionRequest {
                    question_id: id,
                    session_id,
                    question,
                    context,
                    options,
                    multi_select,
                },
            },
        );
        (id, rx)
    }

    /// 取某会话当前 pending 的提问（前端切换会话 / 重连时用来恢复卡片）。
    pub fn pending_for(&self, session_id: &str) -> Option<WorkQuestionRequest> {
        self.cleanup_expired_locked();
        self.pending
            .lock()
            .values()
            .find(|e| e.session_id == session_id)
            .map(|e| e.request.clone())
    }

    /// 取某角色名下所有 pending 提问（陪伴侧据此判断"有没有事卡在用户那儿"）。
    pub fn pending_for_char(&self, char_id: &str) -> Vec<WorkQuestionRequest> {
        if char_id.is_empty() {
            return Vec::new();
        }
        self.cleanup_expired_locked();
        self.pending
            .lock()
            .values()
            .filter(|e| e.char_id == char_id)
            .map(|e| e.request.clone())
            .collect()
    }

    /// 回传用户回答，唤醒正在等待的工具。
    ///
    /// 校验：选中的 label 必须属于该题选项；
    /// 单选模式下 `custom` 与 `selected` 互斥（自由输入即"我不选你给的"）。
    /// 两者都空视为跳过——合法的收尾方式，让模型自己拍板继续。
    pub fn respond(&self, question_id: u64, answer: WorkQuestionAnswer) -> Result<(), String> {
        let entry = {
            let mut pending = self.pending.lock();
            pending.remove(&question_id)
        };
        let Some(entry) = entry else {
            return Err("该提问已失效（已被回答、取消或超时）".to_string());
        };
        let labels: Vec<&str> = entry
            .request
            .options
            .iter()
            .map(|o| o.label.as_str())
            .collect();
        let mut seen: Vec<&str> = Vec::new();
        for label in &answer.selected {
            if !labels.contains(&label.as_str()) {
                return Err(format!(
                    "选项「{label}」不在本题候选中（可选：{}）",
                    labels.join(" / ")
                ));
            }
            if seen.contains(&label.as_str()) {
                return Err(format!("选项「{label}」重复提交"));
            }
            seen.push(label.as_str());
        }
        let custom = answer.custom.as_deref().unwrap_or("").trim().to_string();
        if !entry.request.multi_select {
            if answer.selected.len() > 1 {
                return Err("本题为单选，最多选一项".to_string());
            }
            if !answer.selected.is_empty() && !custom.is_empty() {
                return Err("单选模式下，选择选项与自由输入只能取其一".to_string());
            }
        }
        let normalized = WorkQuestionAnswer {
            selected: answer.selected,
            custom: if custom.is_empty() { None } else { Some(custom) },
        };
        let _ = entry.sender.send(normalized);
        Ok(())
    }

    /// 会话取消 / 重置时清理其 pending 提问，避免 loop 永久挂起。
    ///
    /// 被移除的条目连同其 sender 一起 drop，正在 `await` 的接收端随即收到
    /// `Err(Canceled)`，loop 得以继续走收尾流程。返回被清理的条数。
    pub fn cancel_session(&self, session_id: &str) -> usize {
        let mut pending = self.pending.lock();
        let before = pending.len();
        pending.retain(|_, e| e.session_id != session_id);
        before - pending.len()
    }

    /// 按 id 撤销单条 pending 提问（广播失败等场景下回收，避免 loop 挂死）。
    pub fn cancel_question(&self, question_id: u64) -> bool {
        self.pending.lock().remove(&question_id).is_some()
    }

    /// 惰性清理过期问题。
    fn cleanup_expired_locked(&self) {
        let mut pending = self.pending.lock();
        let now = Instant::now();
        pending.retain(|_, e| now.duration_since(e.created_at) < WORK_QUESTION_TTL);
    }
}

impl Default for WorkQuestionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

static WORK_QUESTION_REGISTRY: once_cell::sync::Lazy<Arc<WorkQuestionRegistry>> =
    once_cell::sync::Lazy::new(|| Arc::new(WorkQuestionRegistry::new()));

/// 全局注册表访问器（工具与命令共用同一实例）。
pub fn global_work_question_registry() -> Arc<WorkQuestionRegistry> {
    Arc::clone(&WORK_QUESTION_REGISTRY)
}
