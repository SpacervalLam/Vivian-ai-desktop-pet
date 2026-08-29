//! Episode —— 经历封包索引层。
//!
//! Episode 是记忆的**聚合视图**，不是事实存储。事实仍然在 DialogueManager、
//! MemoryManager、BeliefStore 里，Episode 只给它们打上"同一段经历"的标签并记录元数据。
//!
//! ## 设计原则
//!
//! - Episode 是轻量索引，不存内容，只存元数据和 ID 引用
//! - 向后兼容：旧记忆 episode_id 为 None，视为"未封包"
//! - 检索时 episode boost 让同 episode 记忆自然上浮，而非强行拉出
//!
//! ## 触发时机
//!
//! 封包由 ConsolidationPipeline Stage 1（新建 SessionSummary 时）触发，
//! 复用已有的主题连续性判断（embedding 相似度 < 阈值 = 新话题 = 新 Episode）。

use std::collections::HashMap;
use std::path::PathBuf;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use super::types::current_timestamp;

/// 单条 Episode 索引 —— 一段经历的元数据和记忆引用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeIndex {
    /// 唯一 ID，格式 "ep_<YYYYMMDD>_<HHMM>_<short_hash>"
    pub episode_id: String,
    /// 这段经历的开始时间（Unix 秒）—— 取最早记忆的 timestamp
    pub started_at: f64,
    /// 这段经历的结束时间（Unix 秒）—— 取最晚记忆的 timestamp
    pub ended_at: f64,
    /// 主题标签（由 Stage 1 LLM 摘要时生成，可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// 参与者列表（如 ["user", "vivian"]）
    #[serde(default)]
    pub participants: Vec<String>,
    /// 摘要（由 Stage 1 SessionSummary 内容复用，不重新生成）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// 重要度 0.0-1.0（取 episode 内记忆的最高 importance）
    #[serde(default)]
    pub importance: f64,
    /// 情绪曲线：(timestamp, mood_tag) 对，按时序排列
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emotion_curve: Vec<(f64, String)>,
    /// 归属此 Episode 的记忆 ID 列表
    #[serde(default)]
    pub memory_ids: Vec<String>,
    /// 从此 Episode 沉淀出的 Belief ID 列表
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub belief_ids: Vec<String>,
    /// 封包创建时间
    pub created_at: f64,
}

/// Episode 存储数据（持久化格式）
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct EpisodeStoreData {
    pub version: u32,
    pub episodes: Vec<EpisodeIndex>,
}

/// Episode 存储 —— 管理所有 Episode 索引
///
/// 持久化为 `episodes.json`，存储在 `characters/<char_id>/memory/` 下。
/// 线程安全：内部使用 RwLock 保护。
pub struct EpisodeStore {
    data: RwLock<EpisodeStoreData>,
    path: PathBuf,
}

impl EpisodeStore {
    /// 创建新 Store，从指定路径加载（若存在）。
    pub fn new(path: PathBuf) -> Self {
        let data = Self::load_from_disk(&path);
        Self {
            data: RwLock::new(data),
            path,
        }
    }

    /// 创建 Episode，把一组记忆 ID 封入新 Episode。
    ///
    /// 参数：
    /// - `memory_ids`: 要封入的记忆 ID 列表
    /// - `timestamps`: 对应记忆的 timestamp 列表（用于计算 started_at/ended_at）
    /// - `importances`: 对应记忆的 importance 列表（用于取最高值）
    /// - `topic`: 可选的主题标签
    /// - `summary`: 可选的摘要文本
    /// - `mood_tags`: 可选的情绪标签列表（与 timestamps 对应）
    ///
    /// 返回新创建的 EpisodeIndex（clone）。
    pub fn seal_episode(
        &self,
        memory_ids: Vec<String>,
        timestamps: &[f64],
        importances: &[f64],
        topic: Option<String>,
        summary: Option<String>,
        mood_tags: &[(f64, String)],
    ) -> EpisodeIndex {
        let now = current_timestamp();
        let episode_id = Self::generate_episode_id(now);

        let started_at = timestamps
            .iter()
            .copied()
            .fold(f64::MAX, f64::min);
        let ended_at = timestamps
            .iter()
            .copied()
            .fold(f64::MIN, f64::max);
        let importance = importances
            .iter()
            .copied()
            .fold(0.0_f64, f64::max);

        let mut emotion_curve: Vec<(f64, String)> = mood_tags.to_vec();
        emotion_curve.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        let episode = EpisodeIndex {
            episode_id,
            started_at,
            ended_at,
            topic,
            participants: vec!["user".to_string(), "vivian".to_string()],
            summary,
            importance,
            emotion_curve,
            memory_ids,
            belief_ids: Vec::new(),
            created_at: now,
        };

        {
            let mut data = self.data.write();
            data.episodes.push(episode.clone());
        }

        // 异步持久化（写失败不阻塞主路径）
        self.persist();

        tracing::info!(
            "[EpisodeStore] 封包 Episode {}：{} 条记忆，时间跨度 {:.0}s",
            episode.episode_id,
            episode.memory_ids.len(),
            episode.ended_at - episode.started_at
        );

        episode
    }

    /// 将记忆 ID 追加到已有 Episode（用于后续记忆回填）。
    pub fn add_memory_ids(&self, episode_id: &str, ids: &[String]) {
        let mut data = self.data.write();
        if let Some(ep) = data.episodes.iter_mut().find(|e| e.episode_id == episode_id) {
            for id in ids {
                if !ep.memory_ids.contains(id) {
                    ep.memory_ids.push(id.clone());
                }
            }
        }
        drop(data);
        self.persist();
    }

    /// 将 Belief ID 关联到 Episode。
    pub fn add_belief_ids(&self, episode_id: &str, ids: &[String]) {
        let mut data = self.data.write();
        if let Some(ep) = data.episodes.iter_mut().find(|e| e.episode_id == episode_id) {
            for id in ids {
                if !ep.belief_ids.contains(id) {
                    ep.belief_ids.push(id.clone());
                }
            }
        }
        drop(data);
        self.persist();
    }

    /// 按 ID 查找 Episode。
    pub fn get(&self, episode_id: &str) -> Option<EpisodeIndex> {
        let data = self.data.read();
        data.episodes
            .iter()
            .find(|e| e.episode_id == episode_id)
            .cloned()
    }

    /// 按时间范围查找 Episode（started_at 或 ended_at 在 [from, to] 区间内）。
    pub fn find_by_time_range(&self, from: f64, to: f64) -> Vec<EpisodeIndex> {
        let data = self.data.read();
        data.episodes
            .iter()
            .filter(|e| e.started_at <= to && e.ended_at >= from)
            .cloned()
            .collect()
    }

    /// 获取所有 Episode（按创建时间倒序）。
    pub fn all_episodes(&self) -> Vec<EpisodeIndex> {
        let data = self.data.read();
        let mut episodes = data.episodes.clone();
        episodes.sort_by(|a, b| {
            b.created_at
                .partial_cmp(&a.created_at)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        episodes
    }

    /// 获取最近的 N 个 Episode。
    pub fn recent(&self, n: usize) -> Vec<EpisodeIndex> {
        let mut episodes = self.all_episodes();
        episodes.truncate(n);
        episodes
    }

    /// 构建 memory_id → episode_id 的反向索引（用于检索时 episode boost）。
    ///
    /// 返回 HashMap：memory_id → episode_id。
    /// 只在需要时调用，避免常驻内存开销。
    pub fn build_memory_episode_index(&self) -> HashMap<String, String> {
        let data = self.data.read();
        let mut index = HashMap::new();
        for ep in &data.episodes {
            for mid in &ep.memory_ids {
                index.insert(mid.clone(), ep.episode_id.clone());
            }
        }
        index
    }

    /// Episode 总数。
    pub fn count(&self) -> usize {
        self.data.read().episodes.len()
    }

    /// 清空所有 Episode 索引
    ///
    /// 用于「清空记忆」操作：Episode 是记忆的聚合视图，
    /// 私有记忆清空后，Episode 索引也应一并清空。
    pub fn clear_all(&self) {
        let mut data = self.data.write();
        let dropped = data.episodes.len();
        data.episodes.clear();
        drop(data);
        if dropped > 0 {
            self.persist();
            tracing::info!("[EpisodeStore] 已清空 {} 条 Episode 索引", dropped);
        }
    }

    // ── 内部方法 ─────────────────────────────────────────────────────

    /// 生成 Episode ID：ep_<YYYYMMDD>_<HHMM>_<hash6>
    fn generate_episode_id(now: f64) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let secs = now as u64;
        // 简单的日期时间格式化（UTC+8）
        let adjusted = secs + 8 * 3600;
        let days_since_epoch = adjusted / 86400;
        let time_of_day = adjusted % 86400;
        let hours = time_of_day / 3600;
        let minutes = (time_of_day % 3600) / 60;

        // 从 days_since_epoch 算年月日（简化算法）
        let (year, month, day) = days_to_ymd(days_since_epoch);

        let mut hasher = DefaultHasher::new();
        now.to_bits().hash(&mut hasher);
        let hash = format!("{:x}", hasher.finish());
        let short_hash = &hash[..6.min(hash.len())];

        format!(
            "ep_{:04}{:02}{:02}_{:02}{:02}_{}",
            year, month, day, hours, minutes, short_hash
        )
    }

    fn load_from_disk(path: &PathBuf) -> EpisodeStoreData {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or(EpisodeStoreData {
                version: 1,
                episodes: Vec::new(),
            }),
            Err(_) => EpisodeStoreData {
                version: 1,
                episodes: Vec::new(),
            },
        }
    }

    fn persist(&self) {
        let data = self.data.read();
        let json = match serde_json::to_string_pretty(&*data) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!("[EpisodeStore] 序列化失败: {}", e);
                return;
            }
        };
        drop(data);

        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // 原子写入：tmp + rename
        let tmp_path = self.path.with_extension("tmp");
        if let Err(e) = std::fs::write(&tmp_path, json.as_bytes()) {
            tracing::warn!("[EpisodeStore] 写入临时文件失败: {}", e);
            return;
        }
        if let Err(e) = std::fs::rename(&tmp_path, &self.path) {
            tracing::warn!("[EpisodeStore] rename 失败: {}", e);
            let _ = std::fs::remove_file(&tmp_path);
        }
    }
}

/// 从 Unix epoch 天数计算年月日（简化算法，够用到 2100 年）
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // 基于 1970-01-01
    let mut remaining = days as i64;
    let mut year = 1970i64;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }

    let leap = is_leap_year(year);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];

    let mut month = 1i64;
    for &md in &month_days {
        if remaining < md {
            break;
        }
        remaining -= md;
        month += 1;
    }

    (year as u64, month as u64, remaining as u64 + 1)
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_episode_id_format() {
        let id = EpisodeStore::generate_episode_id(1720000000.0);
        assert!(id.starts_with("ep_"));
        assert!(id.len() > 10);
    }

    #[test]
    fn days_to_ymd_epoch() {
        // 1970-01-01 = day 0
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        // 2024-01-01 = day 19723
        assert_eq!(days_to_ymd(19723), (2024, 1, 1));
    }

    #[test]
    fn seal_and_retrieve() {
        let dir = std::env::temp_dir().join("vivian_episode_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("episodes.json");

        let store = EpisodeStore::new(path.clone());
        assert_eq!(store.count(), 0);

        let ep = store.seal_episode(
            vec!["mem_a".to_string(), "mem_b".to_string()],
            &[1000.0, 2000.0],
            &[0.5, 0.8],
            Some("考试".to_string()),
            Some("用户考试失利".to_string()),
            &[(1000.0, "sad".to_string()), (2000.0, "comforted".to_string())],
        );

        assert_eq!(store.count(), 1);
        assert!(ep.episode_id.starts_with("ep_"));
        assert_eq!(ep.memory_ids.len(), 2);
        assert!((ep.started_at - 1000.0).abs() < 0.1);
        assert!((ep.ended_at - 2000.0).abs() < 0.1);
        assert!((ep.importance - 0.8).abs() < 0.01);

        // 反向索引
        let index = store.build_memory_episode_index();
        assert_eq!(index.get("mem_a").unwrap(), &ep.episode_id);
        assert_eq!(index.get("mem_b").unwrap(), &ep.episode_id);

        // 时间范围查找
        let found = store.find_by_time_range(500.0, 1500.0);
        assert_eq!(found.len(), 1);

        // 清理
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_belief_ids() {
        let dir = std::env::temp_dir().join("vivian_episode_test2");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("episodes.json");

        let store = EpisodeStore::new(path.clone());
        let ep = store.seal_episode(
            vec!["mem_a".to_string()],
            &[1000.0],
            &[0.5],
            None,
            None,
            &[],
        );

        store.add_belief_ids(&ep.episode_id, &["belief_1".to_string()]);
        let updated = store.get(&ep.episode_id).unwrap();
        assert_eq!(updated.belief_ids, vec!["belief_1".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
