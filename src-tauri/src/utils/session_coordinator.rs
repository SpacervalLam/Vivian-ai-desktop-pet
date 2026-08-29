use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use crate::dialogue::DialogueManager;
use crate::memory::MemoryManager;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TurnKind {
    UserChat,
    CrossCharacter,
    ProactiveTick,
}

struct TurnSlot {
    kind: TurnKind,
    started_at: Instant,
}

struct CoordinatorInner {
    current_turns: Mutex<HashMap<String, TurnSlot>>,
    pending_user: Mutex<HashMap<String, bool>>,
}

pub struct SessionCoordinator {
    inner: Arc<CoordinatorInner>,
}

impl SessionCoordinator {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(CoordinatorInner {
                current_turns: Mutex::new(HashMap::new()),
                pending_user: Mutex::new(HashMap::new()),
            }),
        }
    }

    fn register_turn(&self, char_id: &str, kind: TurnKind) {
        self.inner.current_turns.lock().insert(
            char_id.to_string(),
            TurnSlot {
                kind,
                started_at: Instant::now(),
            },
        );
    }

    fn clear_pending(&self, char_id: &str) {
        self.inner.pending_user.lock().remove(char_id);
    }

    /// 标记用户输入已到达，让正在运行或即将运行的 proactive tick / cross-character turn 主动让出。
    ///
    /// 应在获取 think_lock 之前调用：若 proactive/cross 正在执行 LLM 调用，
    /// think_lock 会阻塞用户对话；proactive/cross 结束后释放 think_lock，
    /// 用户对话进入；下一次 proactive tick 看到 pending 标记后跳过。
    pub fn signal_user_input(&self, char_id: &str) {
        self.inner
            .pending_user
            .lock()
            .insert(char_id.to_string(), true);
    }

    /// 用户对话 turn：设置 memory + dialogue 的 session_id，登记 turn ownership。
    ///
    /// 返回 RAII Guard，Drop 时自动恢复前一个 session_id 并释放 turn。
    pub fn enter_user_turn(
        &self,
        char_id: &str,
        session_id: &str,
        memory: &Arc<MemoryManager>,
        dialogue: &Arc<DialogueManager>,
    ) -> TurnGuard {
        let prev_memory_sid = memory.get_session_id();
        let prev_dialogue_sid = dialogue.get_session_id();
        memory.set_session_id(Some(session_id.to_string()));
        dialogue.set_session_id(Some(session_id.to_string()));
        self.clear_pending(char_id);
        self.register_turn(char_id, TurnKind::UserChat);
        TurnGuard {
            inner: self.inner.clone(),
            char_id: char_id.to_string(),
            memory: memory.clone(),
            dialogue: dialogue.clone(),
            owns_session_id: true,
            prev_memory_sid,
            prev_dialogue_sid,
        }
    }

    /// 跨角色对话 turn：检查 pending_user 后设置 session_id 并登记 turn ownership。
    ///
    /// 返回 None 表示有用户输入在等待，应跳过本次跨角色对话。
    /// 返回 Some(guard) 时，Drop 时自动恢复前一个 session_id（支持双 Session 热切换）。
    pub fn try_enter_cross_turn(
        &self,
        char_id: &str,
        session_id: &str,
        memory: &Arc<MemoryManager>,
        dialogue: &Arc<DialogueManager>,
    ) -> Option<TurnGuard> {
        {
            let pending = self.inner.pending_user.lock();
            if pending.get(char_id).copied().unwrap_or(false) {
                return None;
            }
        }
        let prev_memory_sid = memory.get_session_id();
        let prev_dialogue_sid = dialogue.get_session_id();
        memory.set_session_id(Some(session_id.to_string()));
        dialogue.set_session_id(Some(session_id.to_string()));
        self.register_turn(char_id, TurnKind::CrossCharacter);
        Some(TurnGuard {
            inner: self.inner.clone(),
            char_id: char_id.to_string(),
            memory: memory.clone(),
            dialogue: dialogue.clone(),
            owns_session_id: true,
            prev_memory_sid,
            prev_dialogue_sid,
        })
    }

    /// 主动 tick turn：检查 sticky preempt。
    ///
    /// 返回 None 表示有用户输入在等待，应跳过本轮 proactive tick。
    /// 不设置 session_id（proactive 不写入对话历史）。
    pub fn try_enter_proactive_turn(
        &self,
        char_id: &str,
        memory: &Arc<MemoryManager>,
        dialogue: &Arc<DialogueManager>,
    ) -> Option<TurnGuard> {
        {
            let pending = self.inner.pending_user.lock();
            if pending.get(char_id).copied().unwrap_or(false) {
                return None;
            }
        }
        {
            let turns = self.inner.current_turns.lock();
            if turns.contains_key(char_id) {
                return None;
            }
        }
        self.register_turn(char_id, TurnKind::ProactiveTick);
        Some(TurnGuard {
            inner: self.inner.clone(),
            char_id: char_id.to_string(),
            memory: memory.clone(),
            dialogue: dialogue.clone(),
            owns_session_id: false,
            prev_memory_sid: None,
            prev_dialogue_sid: None,
        })
    }

    /// 查询指定角色当前 turn 类型（None = 空闲）
    pub fn current_turn_kind(&self, char_id: &str) -> Option<TurnKind> {
        self.inner
            .current_turns
            .lock()
            .get(char_id)
            .map(|s| s.kind)
    }

    /// 查询指定角色当前 turn 已持续时间
    pub fn current_turn_elapsed(&self, char_id: &str) -> Option<std::time::Duration> {
        self.inner
            .current_turns
            .lock()
            .get(char_id)
            .map(|s| s.started_at.elapsed())
    }

    /// 是否有用户输入等待中
    pub fn has_pending_user(&self, char_id: &str) -> bool {
        self.inner
            .pending_user
            .lock()
            .get(char_id)
            .copied()
            .unwrap_or(false)
    }
}

impl Default for SessionCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

pub struct TurnGuard {
    inner: Arc<CoordinatorInner>,
    char_id: String,
    memory: Arc<MemoryManager>,
    dialogue: Arc<DialogueManager>,
    owns_session_id: bool,
    prev_memory_sid: Option<String>,
    prev_dialogue_sid: Option<String>,
}

impl TurnGuard {
    pub fn kind(&self) -> Option<TurnKind> {
        self.inner
            .current_turns
            .lock()
            .get(&self.char_id)
            .map(|s| s.kind)
    }

    pub fn char_id(&self) -> &str {
        &self.char_id
    }
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        if self.owns_session_id {
            self.memory.set_session_id(self.prev_memory_sid.take());
            self.dialogue.set_session_id(self.prev_dialogue_sid.take());
        }
        self.inner.current_turns.lock().remove(&self.char_id);
    }
}
