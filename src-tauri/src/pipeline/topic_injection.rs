use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

const DEFAULT_COOLDOWN_SECONDS: u64 = 3600;
const DEFAULT_DURATION_TURNS: u32 = 5;
const MAX_ACTIVE_TOPICS: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionTopic {
    pub id: String,
    pub keyword: String,
    pub knowledge: String,
    pub priority: f64,
    #[serde(default = "default_duration")]
    pub duration_turns: u32,
    #[serde(default = "default_cooldown")]
    pub cooldown_seconds: u64,
}

fn default_duration() -> u32 {
    DEFAULT_DURATION_TURNS
}

fn default_cooldown() -> u64 {
    DEFAULT_COOLDOWN_SECONDS
}

#[derive(Debug, Clone)]
struct ActiveTopic {
    topic: InjectionTopic,
    _activated_at: Instant,
    remaining_turns: u32,
}

#[derive(Debug, Clone)]
struct CooldownEntry {
    expires_at: Instant,
}

#[derive(Clone)]
pub struct TopicInjectionManager {
    active: Arc<Mutex<Vec<ActiveTopic>>>,
    cooldowns: Arc<Mutex<HashMap<String, CooldownEntry>>>,
    registered: Arc<Mutex<HashMap<String, InjectionTopic>>>,
}

impl Default for TopicInjectionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TopicInjectionManager {
    pub fn new() -> Self {
        Self {
            active: Arc::new(Mutex::new(Vec::new())),
            cooldowns: Arc::new(Mutex::new(HashMap::new())),
            registered: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register(&self, topic: InjectionTopic) {
        self.registered
            .lock()
            .insert(topic.id.clone(), topic);
    }

    pub fn unregister(&self, id: &str) {
        self.registered.lock().remove(id);
    }

    pub fn list_registered(&self) -> Vec<InjectionTopic> {
        self.registered.lock().values().cloned().collect()
    }

    /// 根据用户输入扫描命中的 topic，激活未在冷却中的最高优先级 topic
    pub fn scan_input(&self, user_input: &str) {
        let now = Instant::now();
        let input_lower = user_input.to_lowercase();

        // 清理过期冷却
        {
            let mut cds = self.cooldowns.lock();
            cds.retain(|_, entry| entry.expires_at > now);
        }

        // 清理已耗尽 turn 的活跃 topic，并释放到冷却
        {
            let mut active = self.active.lock();
            let mut to_cooldown: Vec<(String, u64)> = Vec::new();
            active.retain(|a| {
                if a.remaining_turns == 0 {
                    to_cooldown.push((a.topic.id.clone(), a.topic.cooldown_seconds));
                    false
                } else {
                    true
                }
            });
            drop(active);
            for (id, cd) in to_cooldown {
                self.cooldowns
                    .lock()
                    .insert(id, CooldownEntry {
                        expires_at: now + Duration::from_secs(cd),
                    });
            }
        }

        // 扫描注册表，找出命中的 topic
        let candidates: Vec<InjectionTopic> = {
            let registered = self.registered.lock();
            registered
                .values()
                .filter(|t| {
                    let keyword_lower = t.keyword.to_lowercase();
                    input_lower.contains(&keyword_lower) && keyword_lower.len() >= 2
                })
                .filter(|t| {
                    // 不重复激活已在活跃列表中的 topic
                    !self.active.lock().iter().any(|a| a.topic.id == t.id)
                })
                .filter(|t| {
                    // 不激活在冷却中的 topic
                    !self.cooldowns.lock().contains_key(&t.id)
                })
                .cloned()
                .collect()
        };

        if candidates.is_empty() {
            return;
        }

        // 按优先级降序排序，激活最高优先级的若干个
        let mut sorted = candidates;
        sorted.sort_by(|a, b| {
            b.priority
                .partial_cmp(&a.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut active = self.active.lock();
        for topic in sorted {
            if active.len() >= MAX_ACTIVE_TOPICS {
                break;
            }
            active.push(ActiveTopic {
                topic: topic.clone(),
                _activated_at: now,
                remaining_turns: topic.duration_turns,
            });
            tracing::debug!(
                "[TopicInjection] 激活话题: {} (keyword={}, duration={}turns)",
                topic.id,
                topic.keyword,
                topic.duration_turns
            );
        }
    }

    /// 消费一轮活跃 topic：返回当前应注入的背景知识文本，并将每个活跃 topic 的 remaining_turns -1
    pub fn consume_turn(&self) -> Option<String> {
        let mut active = self.active.lock();
        if active.is_empty() {
            return None;
        }

        let mut sections: Vec<String> = Vec::new();
        for a in active.iter_mut() {
            if a.remaining_turns > 0 {
                sections.push(format!(
                    "【背景知识：{}】\n{}",
                    a.topic.id, a.topic.knowledge
                ));
                a.remaining_turns -= 1;
            }
        }
        drop(active);

        // 清理已耗尽的活跃 topic，并放入冷却
        let now = Instant::now();
        let mut to_cooldown: Vec<(String, u64)> = Vec::new();
        {
            let mut active = self.active.lock();
            active.retain(|a| {
                if a.remaining_turns == 0 {
                    to_cooldown.push((a.topic.id.clone(), a.topic.cooldown_seconds));
                    false
                } else {
                    true
                }
            });
        }
        for (id, cd) in to_cooldown {
            self.cooldowns
                .lock()
                .insert(id, CooldownEntry {
                    expires_at: now + Duration::from_secs(cd),
                });
        }

        if sections.is_empty() {
            None
        } else {
            Some(sections.join("\n\n"))
        }
    }

    /// 仅查询当前应注入的背景知识文本（不消耗 turn）
    pub fn peek_active(&self) -> Option<String> {
        let active = self.active.lock();
        if active.is_empty() {
            return None;
        }
        let mut sections: Vec<String> = Vec::new();
        for a in active.iter() {
            if a.remaining_turns > 0 {
                sections.push(format!(
                    "【背景知识：{}】\n{}",
                    a.topic.id, a.topic.knowledge
                ));
            }
        }
        if sections.is_empty() {
            None
        } else {
            Some(sections.join("\n\n"))
        }
    }

    pub fn active_count(&self) -> usize {
        self.active.lock().len()
    }

    pub fn cooldown_count(&self) -> usize {
        self.cooldowns.lock().len()
    }

    /// 强制停用指定 topic（不进入冷却，可立即重新激活）
    pub fn force_deactivate(&self, id: &str) {
        self.active.lock().retain(|a| a.topic.id != id);
    }

    /// 清空所有活跃 topic 与冷却记录
    pub fn reset(&self) {
        self.active.lock().clear();
        self.cooldowns.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_topic(id: &str, keyword: &str, knowledge: &str, priority: f64) -> InjectionTopic {
        InjectionTopic {
            id: id.to_string(),
            keyword: keyword.to_string(),
            knowledge: knowledge.to_string(),
            priority,
            duration_turns: 2,
            cooldown_seconds: 60,
        }
    }

    #[test]
    fn scan_and_consume_roundtrip() {
        let mgr = TopicInjectionManager::new();
        mgr.register(make_topic("rust", "rust", "Rust is a systems language", 0.8));
        mgr.scan_input("Tell me about rust programming");
        assert_eq!(mgr.active_count(), 1);
        let text1 = mgr.consume_turn().expect("should have section");
        assert!(text1.contains("Rust is a systems language"));
        // 第二轮仍可消费（duration=2）
        let text2 = mgr.consume_turn().expect("should still have section");
        assert!(text2.contains("Rust is a systems language"));
        // 第三轮：turns 已耗尽，应进入冷却
        let text3 = mgr.consume_turn();
        assert!(text3.is_none());
        assert_eq!(mgr.active_count(), 0);
        assert_eq!(mgr.cooldown_count(), 1);
    }

    #[test]
    fn cooldown_blocks_reactivation() {
        let mgr = TopicInjectionManager::new();
        mgr.register(make_topic("rust", "rust", "Rust info", 0.8));
        mgr.scan_input("rust");
        assert_eq!(mgr.active_count(), 1);
        // 消耗两轮让 topic 进入冷却
        mgr.consume_turn();
        mgr.consume_turn();
        assert_eq!(mgr.cooldown_count(), 1);
        // 再次扫描相同输入不应激活
        mgr.scan_input("rust");
        assert_eq!(mgr.active_count(), 0);
    }

    #[test]
    fn no_match_no_activation() {
        let mgr = TopicInjectionManager::new();
        mgr.register(make_topic("rust", "rust", "Rust info", 0.8));
        mgr.scan_input("completely unrelated input");
        assert_eq!(mgr.active_count(), 0);
    }
}
