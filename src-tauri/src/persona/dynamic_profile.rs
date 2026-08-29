//! 智能体动态行为画像
//!
//! 与 `PersonaEngine`（静态基础人设）互补，这里跟踪 Vivian 近期与用户交互的
//! 行为模式，作为"画像双轨制"的动态轨：
//!
//! - **统计层（机械）**：
//!   - `recent_topics`：最近用户消息中提取的关键话题（jieba 分词）
//!   - `recent_user_emotions` / `recent_ai_emotions`：近期情绪分布
//!   - `avg_msg_length`：平均消息长度
//!   - `total_turns`：滚动窗口内总轮次
//! - **语义层（LLM 抽取）**：
//!   - `acquired_behaviors`：Stage 2 反思抽取的语义级行为画像，包含四类：
//!     - 语言风格（如「常用短句+语气词」「偶尔中英混用」）
//!     - 行为举止（如「主动追问项目进度」「用户疲惫时主动安慰」）
//!     - 互动方式（如「喜欢用反问句开启话题」「不直接否定用户」）
//!     - 习得能力（如「学会了查询天气」「能识别用户工作日程」）
//!
//! 设计约束：
//! - 读路径（`format_for_prompt`）不调用 LLM，仅做机械统计 + 读取已持久化的语义层
//! - 写路径：`record_turn` 追加统计层；`merge_acquired_behaviors` 合并语义层
//! - 语义层由 ConsolidationPipeline Stage 2 抽取，BrainChatChain 调用 merge 写入
//!
//! 持久化：`%APPDATA%\Vivian\persona\dynamic_profile.json`

use std::collections::HashMap;
use std::path::PathBuf;

use jieba_rs::Jieba;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};
use crate::utils::path::get_user_data_dir;

/// 全局 jieba 实例（与 retriever.rs / filter.rs 共享）
static JIEBA: Lazy<Jieba> = Lazy::new(Jieba::new);

/// 滚动窗口最大轮次数
const MAX_TURNS: usize = 20;

/// 单条消息保留的最大字符数（避免持久化文件膨胀）
const MAX_MSG_CHARS: usize = 100;

/// 语义层 acquired_behaviors 最大保留条数（FIFO 淘汰）
const MAX_ACQUIRED_BEHAVIORS: usize = 30;

/// 语义级行为去重的描述相似度阈值（Jaccard，jieba 分词）
const ACQUIRED_BEHAVIOR_DEDUP_THRESHOLD: f64 = 0.7;

/// 语义级行为类别
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AcquiredBehaviorCategory {
    /// 语言风格（句式、词汇偏好、语气）
    LanguageStyle,
    /// 行为举止（主动行为、反应模式）
    Behavior,
    /// 互动方式（沟通节奏、提问方式）
    Interaction,
    /// 习得能力（新学会的工具/技能）
    Skill,
}

impl AcquiredBehaviorCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LanguageStyle => "language_style",
            Self::Behavior => "behavior",
            Self::Interaction => "interaction",
            Self::Skill => "skill",
        }
    }

    /// 从 LLM 返回的字符串解析为类别（容错：未知值归为 Behavior）
    pub fn from_str_lossy(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "language_style" | "language" | "style" => Self::LanguageStyle,
            "interaction" | "interact" => Self::Interaction,
            "skill" | "ability" => Self::Skill,
            _ => Self::Behavior,
        }
    }

    /// 中文显示名（用于 prompt 注入）
    pub fn display_zh(&self) -> &'static str {
        match self {
            Self::LanguageStyle => "语言风格",
            Self::Behavior => "行为举止",
            Self::Interaction => "互动方式",
            Self::Skill => "习得能力",
        }
    }
}

/// 语义级行为画像条目（由 Stage 2 反思 LLM 抽取）
///
/// 与统计层互补：统计层回答「最近在聊什么」，语义层回答「Vivian 学到了什么稳定的行为模式」。
/// 从对话中归纳智能体已表现出的行为特征，作为可演化的动态人设层
/// （与 PersonaConfig 的锁定核心相对）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcquiredBehavior {
    /// 行为类别
    pub category: AcquiredBehaviorCategory,
    /// 一句话描述（如「主动追问用户的项目进度」）
    pub description: String,
    /// 支撑此行为推断的来源记忆 ID（可空）
    #[serde(default)]
    pub evidence: Vec<String>,
    /// 置信度 0.0-1.0（LLM 自评）
    #[serde(default = "default_acquired_confidence")]
    pub confidence: f64,
    /// 抽取时间戳（秒）
    pub acquired_at: f64,
}

fn default_acquired_confidence() -> f64 {
    0.5
}

impl AcquiredBehavior {
    /// 格式化为 prompt 注入文本（一行）
    pub fn to_prompt_line(&self) -> String {
        format!(
            "- [{}] {}",
            self.category.display_zh(),
            self.description
        )
    }
}

/// 单轮对话记录
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TurnRecord {
    user_input: String,
    ai_response: String,
    user_emotion: String,
    ai_emotion: String,
    timestamp: f64,
}

/// 持久化数据
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProfileData {
    turns: Vec<TurnRecord>,
    /// 语义级行为画像（Stage 2 反思抽取，与 turns 互补）
    #[serde(default)]
    acquired_behaviors: Vec<AcquiredBehavior>,
}

/// 智能体动态行为画像
///
/// 跟踪 Vivian 近期交互行为模式，作为 prompt 注入的动态信号源。
pub struct DynamicBehaviorProfile {
    inner: RwLock<ProfileData>,
    persistence_path: PathBuf,
}

impl DynamicBehaviorProfile {
    /// 加载或创建新的动态行为画像
    pub fn new() -> VivianResult<Self> {
        let dir = get_user_data_dir().join("persona");
        std::fs::create_dir_all(&dir)
            .map_err(|e| VivianError::Memory(format!("创建动态画像目录失败: {e}")))?;
        let path = dir.join("dynamic_profile.json");
        let data = if path.exists() {
            Self::load_from(&path)
        } else {
            ProfileData::default()
        };
        Ok(Self {
            inner: RwLock::new(data),
            persistence_path: path,
        })
    }

    /// 降级构造：不持久化，仅内存
    pub fn fallback() -> Self {
        Self {
            inner: RwLock::new(ProfileData::default()),
            persistence_path: PathBuf::new(),
        }
    }

    fn load_from(path: &std::path::Path) -> ProfileData {
        match std::fs::read_to_string(path) {
            Ok(content) if !content.trim().is_empty() => {
                serde_json::from_str::<ProfileData>(&content).unwrap_or_default()
            }
            _ => ProfileData::default(),
        }
    }

    fn save_to(&self) -> VivianResult<()> {
        if self.persistence_path.as_os_str().is_empty() {
            return Ok(());
        }
        let data = self.inner.read().clone();
        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| VivianError::Memory(format!("序列化动态画像失败: {e}")))?;
        let tmp = self.persistence_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)
            .map_err(|e| VivianError::Memory(format!("写入动态画像临时文件失败: {e}")))?;
        std::fs::rename(&tmp, &self.persistence_path)
            .map_err(|e| VivianError::Memory(format!("替换动态画像文件失败: {e}")))?;
        Ok(())
    }

    /// 记录一轮对话（写路径，在 post_process 中调用）
    ///
    /// 追加到滚动窗口，FIFO 淘汰最旧记录，然后持久化。
    pub fn record_turn(
        &self,
        user_input: &str,
        ai_response: &str,
        user_emotion: &str,
        ai_emotion: &str,
    ) {
        let now = chrono::Local::now().timestamp() as f64;
        let record = TurnRecord {
            user_input: truncate_chars(user_input, MAX_MSG_CHARS),
            ai_response: truncate_chars(ai_response, MAX_MSG_CHARS),
            user_emotion: user_emotion.trim().to_string(),
            ai_emotion: ai_emotion.trim().to_string(),
            timestamp: now,
        };

        {
            let mut data = self.inner.write();
            data.turns.push(record);
            if data.turns.len() > MAX_TURNS {
                let excess = data.turns.len() - MAX_TURNS;
                data.turns.drain(0..excess);
            }
        }

        if let Err(e) = self.save_to() {
            tracing::warn!("[DynamicBehaviorProfile] 持久化失败: {}", e);
        }
    }

    /// 格式化为 prompt 注入文本（读路径，不调用 LLM）
    ///
    /// 数据不足（< 3 轮）且无语义层时返回空字符串，避免噪音。
    pub fn format_for_prompt(&self) -> String {
        let data = self.inner.read();
        let has_stats = data.turns.len() >= 3;
        let has_semantic = !data.acquired_behaviors.is_empty();
        if !has_stats && !has_semantic {
            return String::new();
        }

        let mut sections = Vec::new();

        // 统计层（机械）
        if has_stats {
            let turns = &data.turns;

            // 1. 近期话题：从用户消息提取关键词，按频次取 top 5
            let topics = extract_recent_topics(turns);

            // 2. 近期情绪分布
            let user_emotions = top_emotions(&turns.iter().map(|t| t.user_emotion.as_str()).collect::<Vec<_>>());
            let ai_emotions = top_emotions(&turns.iter().map(|t| t.ai_emotion.as_str()).collect::<Vec<_>>());

            sections.push(format!("最近话题：{}", if topics.is_empty() { "（暂无）".to_string() } else { topics.join("、") }));
            sections.push(format!("近期用户情绪：{}", if user_emotions.is_empty() { "（暂无）".to_string() } else { user_emotions.join("、") }));
            sections.push(format!("近期薇薇安情绪：{}", if ai_emotions.is_empty() { "（暂无）".to_string() } else { ai_emotions.join("、") }));
        }

        // 语义层（LLM 抽取的稳定行为模式）
        if has_semantic {
            // 按类别分组，置信度降序，每类最多 3 条避免 prompt 膨胀
            let behavior_block = format_acquired_behaviors_for_prompt(&data.acquired_behaviors);
            if !behavior_block.is_empty() {
                sections.push(behavior_block);
            }
        }

        format!("【薇薇安近期行为画像】\n{}", sections.join("\n"))
    }

    /// 合并 Stage 2 抽取的语义级行为画像（写路径，由 BrainChatChain 调用）
    ///
    /// 去重策略：与既有同类别行为做 Jaccard 相似度（jieba 分词）比对，
    /// 相似度 ≥ `ACQUIRED_BEHAVIOR_DEDUP_THRESHOLD` 时取较高置信度的描述覆盖；
    /// 否则作为新行为追加。FIFO 淘汰超过 `MAX_ACQUIRED_BEHAVIORS` 的最旧条目。
    pub fn merge_acquired_behaviors(&self, new_behaviors: Vec<AcquiredBehavior>) {
        if new_behaviors.is_empty() {
            return;
        }
        {
            let mut data = self.inner.write();
            let now = chrono::Local::now().timestamp() as f64;
            for new_b in new_behaviors {
                let mut new_b = new_b;
                new_b.acquired_at = now;
                // 在同类别下找相似条目
                let mut merged = false;
                for existing in data.acquired_behaviors.iter_mut() {
                    if existing.category == new_b.category {
                        let sim = jaccard_similarity(
                            &tokenize(&existing.description),
                            &tokenize(&new_b.description),
                        );
                        if sim >= ACQUIRED_BEHAVIOR_DEDUP_THRESHOLD {
                            // 取较高置信度的描述覆盖
                            if new_b.confidence > existing.confidence {
                                existing.description = new_b.description.clone();
                                existing.confidence = new_b.confidence;
                            }
                            // 合并 evidence（克隆避免部分移动）
                            for ev in new_b.evidence.iter() {
                                if !existing.evidence.iter().any(|x| x == ev) {
                                    existing.evidence.push(ev.clone());
                                }
                            }
                            merged = true;
                            break;
                        }
                    }
                }
                if !merged {
                    data.acquired_behaviors.push(new_b);
                }
            }
            // FIFO 淘汰：按 acquired_at 升序，超出上限删最旧
            if data.acquired_behaviors.len() > MAX_ACQUIRED_BEHAVIORS {
                data.acquired_behaviors.sort_by(|a, b| {
                    a.acquired_at
                        .partial_cmp(&b.acquired_at)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let excess = data.acquired_behaviors.len() - MAX_ACQUIRED_BEHAVIORS;
                data.acquired_behaviors.drain(0..excess);
            }
        }
        if let Err(e) = self.save_to() {
            tracing::warn!("[DynamicBehaviorProfile] 语义层持久化失败: {}", e);
        }
    }
}

/// 截断字符串到指定字符数
fn truncate_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

/// 格式化语义层行为画像为 prompt 注入块
///
/// 按类别分组展示，每类按置信度降序取 top 3，避免 prompt 膨胀。
fn format_acquired_behaviors_for_prompt(behaviors: &[AcquiredBehavior]) -> String {
    if behaviors.is_empty() {
        return String::new();
    }
    // 按类别分组
    let mut by_category: HashMap<AcquiredBehaviorCategory, Vec<&AcquiredBehavior>> = HashMap::new();
    for b in behaviors {
        by_category.entry(b.category.clone()).or_default().push(b);
    }
    // 每类按置信度降序取 top 3
    let mut lines: Vec<String> = vec!["已习得的行为模式：".to_string()];
    for category in [
        AcquiredBehaviorCategory::LanguageStyle,
        AcquiredBehaviorCategory::Behavior,
        AcquiredBehaviorCategory::Interaction,
        AcquiredBehaviorCategory::Skill,
    ] {
        if let Some(items) = by_category.get(&category) {
            let mut sorted = items.to_vec();
            sorted.sort_by(|a, b| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let top: Vec<&AcquiredBehavior> = sorted.into_iter().take(3).collect();
            for b in top {
                lines.push(b.to_prompt_line());
            }
        }
    }
    if lines.len() == 1 {
        return String::new();
    }
    lines.join("\n")
}

/// Jaccard 相似度（用于语义层行为去重）
///
/// 两个 token 集合的交集/并集，返回 [0, 1]。
fn jaccard_similarity(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let set_a: std::collections::HashSet<&String> = a.iter().collect();
    let set_b: std::collections::HashSet<&String> = b.iter().collect();
    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();
    if union == 0 {
        return 0.0;
    }
    intersection as f64 / union as f64
}

/// 从最近用户消息中提取话题关键词
///
/// 每条消息取 top 2 关键词（jieba 分词，过滤停用词和短词），
/// 全局按频次排序取 top 5。
fn extract_recent_topics(turns: &[TurnRecord]) -> Vec<String> {
    let mut freq: HashMap<String, usize> = HashMap::new();
    for turn in turns {
        let tokens = tokenize(&turn.user_input);
        for t in tokens {
            *freq.entry(t).or_insert(0) += 1;
        }
    }
    let mut sorted: Vec<(String, usize)> = freq.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    sorted.into_iter()
        .take(5)
        .map(|(k, _)| k)
        .collect()
}

/// 统计情绪频次，返回 top 2（过滤空字符串）
fn top_emotions(emotions: &[&str]) -> Vec<String> {
    let mut freq: HashMap<String, usize> = HashMap::new();
    for &e in emotions {
        let e = e.trim();
        if e.is_empty() {
            continue;
        }
        *freq.entry(e.to_string()).or_insert(0) += 1;
    }
    let mut sorted: Vec<(String, usize)> = freq.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    sorted.into_iter()
        .take(2)
        .map(|(k, _)| k)
        .collect()
}

/// jieba 分词（与 retriever.rs 同源，过滤短词）
fn tokenize(text: &str) -> Vec<String> {
    JIEBA
        .cut(text, true)
        .into_iter()
        .map(|s| s.to_lowercase())
        .filter(|s| {
            let chars = s.chars().count();
            // 过滤单字（中文）和过短 token（英文），保留有语义价值的词
            chars > 1 && !s.trim().is_empty()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_turn_appends_to_window() {
        let profile = DynamicBehaviorProfile::fallback();
        profile.record_turn("你好", "你好呀", "happy", "lively");
        assert_eq!(profile.inner.read().turns.len(), 1);
    }

    #[test]
    fn record_turn_fifo_eviction() {
        let profile = DynamicBehaviorProfile::fallback();
        for i in 0..(MAX_TURNS + 5) {
            profile.record_turn(&format!("msg {}", i), "resp", "neutral", "neutral");
        }
        assert_eq!(profile.inner.read().turns.len(), MAX_TURNS);
    }

    #[test]
    fn format_for_prompt_empty_when_insufficient() {
        let profile = DynamicBehaviorProfile::fallback();
        profile.record_turn("hi", "hello", "happy", "lively");
        assert_eq!(profile.format_for_prompt(), "");
    }

    #[test]
    fn format_for_prompt_returns_content() {
        let profile = DynamicBehaviorProfile::fallback();
        for i in 0..5 {
            profile.record_turn(
                &format!("我想聊聊项目X的进度 {}", i),
                "好的，说说看",
                "focused",
                "calm",
            );
        }
        let formatted = profile.format_for_prompt();
        assert!(formatted.contains("薇薇安近期行为画像"));
        assert!(formatted.contains("互动轮次：5"));
    }

    #[test]
    fn truncate_chars_works() {
        assert_eq!(truncate_chars("你好世界test", 3), "你好世");
        assert_eq!(truncate_chars("短", 100), "短");
    }
}
