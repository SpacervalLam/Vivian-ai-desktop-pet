use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct LeaderPriority {
    online: bool,
    present: bool,
    active: bool,
    rank: u8,
}

impl LeaderPriority {
    fn compare(&self, other: &Self) -> std::cmp::Ordering {
        self.online
            .cmp(&other.online)
            .then(self.present.cmp(&other.present))
            .then(self.active.cmp(&other.active))
            .then(other.rank.cmp(&self.rank))
    }
}

struct LeaderSlot {
    leader_id: String,
    term: u64,
    last_heartbeat: Instant,
    priority: LeaderPriority,
}

struct CoordinatorInner {
    current: RwLock<Option<LeaderSlot>>,
}

pub struct ProactiveLeaderCoordinator {
    inner: Arc<CoordinatorInner>,
}

const LEADER_TIMEOUT: Duration = Duration::from_secs(45);
const LEADER_RENEW_INTERVAL: Duration = Duration::from_secs(10);

impl ProactiveLeaderCoordinator {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(CoordinatorInner {
                current: RwLock::new(None),
            }),
        }
    }

    fn compute_priority(
        char_id: &str,
        is_online: bool,
        is_present: bool,
        is_active: bool,
    ) -> LeaderPriority {
        let rank = match char_id {
            "vivian" => 0,
            "nana" => 1,
            _ => 2,
        };
        LeaderPriority {
            online: is_online,
            present: is_present,
            active: is_active,
            rank,
        }
    }

    /// 尝试获取或续租 leader 身份。
    ///
    /// 返回 true 表示当前角色持有 leader 身份（本次 tick 拥有发言权）。
    /// 选举规则：
    /// - 当前 leader 心跳正常且优先级不低于自己 → 让位
    /// - 当前 leader 心跳超时 → 自己抢占
    /// - 当前 leader 优先级低于自己 → 抢占
    /// - 无 leader → 自己当选
    pub fn try_acquire_or_renew(
        &self,
        char_id: &str,
        is_online: bool,
        is_present: bool,
        is_active: bool,
    ) -> bool {
        let my_priority = Self::compute_priority(char_id, is_online, is_present, is_active);
        let now = Instant::now();
        let mut current = self.inner.current.write();

        if let Some(slot) = current.as_ref() {
            if slot.leader_id == char_id {
                *current = Some(LeaderSlot {
                    leader_id: char_id.to_string(),
                    term: slot.term,
                    last_heartbeat: now,
                    priority: my_priority,
                });
                return true;
            }

            let heartbeat_age = now.duration_since(slot.last_heartbeat);
            if heartbeat_age > LEADER_TIMEOUT {
                let new_term = slot.term + 1;
                tracing::info!(
                    "[ProactiveLeader] {} 抢占 leader（term={}，旧 leader {} 心跳超时 {:.0}s）",
                    char_id,
                    new_term,
                    slot.leader_id,
                    heartbeat_age.as_secs()
                );
                *current = Some(LeaderSlot {
                    leader_id: char_id.to_string(),
                    term: new_term,
                    last_heartbeat: now,
                    priority: my_priority,
                });
                return true;
            }

            if my_priority.compare(&slot.priority) == std::cmp::Ordering::Greater {
                let new_term = slot.term + 1;
                tracing::info!(
                    "[ProactiveLeader] {} 抢占 leader（term={}，优先级高于 {}）",
                    char_id,
                    new_term,
                    slot.leader_id
                );
                *current = Some(LeaderSlot {
                    leader_id: char_id.to_string(),
                    term: new_term,
                    last_heartbeat: now,
                    priority: my_priority,
                });
                return true;
            }

            false
        } else {
            tracing::info!(
                "[ProactiveLeader] {} 当选 leader（term=1，无竞争者）",
                char_id
            );
            *current = Some(LeaderSlot {
                leader_id: char_id.to_string(),
                term: 1,
                last_heartbeat: now,
                priority: my_priority,
            });
            true
        }
    }

    /// 主动让位（Rest/Offline/Busy 状态切换时调用）
    pub fn resign(&self, char_id: &str) {
        let mut current = self.inner.current.write();
        if let Some(slot) = current.as_ref() {
            if slot.leader_id == char_id {
                tracing::info!(
                    "[ProactiveLeader] {} 主动让出 leader（term={}）",
                    char_id,
                    slot.term
                );
                *current = None;
            }
        }
    }

    /// 查询当前 leader
    pub fn current_leader(&self) -> Option<String> {
        self.inner.current.read().as_ref().map(|s| s.leader_id.clone())
    }

    /// 查询当前 leader 心跳年龄
    pub fn leader_heartbeat_age(&self) -> Option<Duration> {
        self.inner
            .current
            .read()
            .as_ref()
            .map(|s| s.last_heartbeat.elapsed())
    }

    /// 是否需要续租（距上次心跳超过 RENEW_INTERVAL）
    pub fn needs_renewal(&self) -> bool {
        match self.inner.current.read().as_ref() {
            Some(slot) => slot.last_heartbeat.elapsed() > LEADER_RENEW_INTERVAL,
            None => true,
        }
    }
}

impl Default for ProactiveLeaderCoordinator {
    fn default() -> Self {
        Self::new()
    }
}
