//! 日常话题池 & 兴趣扩展 & 话题冷却池
//!
//! - `DailyTopicPool`：时段 / 星期感知的轻量话题
//! - `InterestExtender`：兴趣标签扩展子话题
//! - `TopicPool`：话题冷却 + 权重 + 持久化（Rust 增强项）
//!   持久化到 `%APPDATA%\Vivian\proactive\topics.json`

use std::collections::HashMap;
use std::path::PathBuf;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use super::super::random_index;
use super::tree::{TopicNode, TopicTree};
use crate::error::{VivianError, VivianResult};
use crate::utils::path::get_user_data_dir;

// ============ 时段话题池 ============

/// 时段标识
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Morning,
    Afternoon,
    Evening,
    Night,
}

impl Period {
    /// 按当前小时推断时段
    pub fn now(hour: u32) -> Self {
        match hour {
            5..=11 => Period::Morning,
            12..=17 => Period::Afternoon,
            18..=22 => Period::Evening,
            _ => Period::Night,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Period::Morning => "morning",
            Period::Afternoon => "afternoon",
            Period::Evening => "evening",
            Period::Night => "night",
        }
    }
}

/// 时段 → 话题列表
static DAILY_POOLS: Lazy<HashMap<&'static str, Vec<&'static str>>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert(
        "morning",
        vec![
            "早安呀~今天有什么计划吗？",
            "早上好！今天元气满满的~",
            "今天早上吃什么了？",
            "新的一天开始啦，加油哦~",
        ],
    );
    m.insert(
        "afternoon",
        vec![
            "下午好~忙什么呢？",
            "下午茶时间到~要不要歇会儿？",
            "今天过得怎么样？",
            "下午容易犯困，注意休息哦~",
        ],
    );
    m.insert(
        "evening",
        vec![
            "晚上好~今天过得开心吗？",
            "累不累呀？想聊聊天~",
            "晚饭吃了吗？",
            "今天有什么有趣的事吗？",
        ],
    );
    m.insert(
        "night",
        vec![
            "这么晚还不睡呀~",
            "熬夜对身体不好哦~",
            "夜深了，在想什么呢？",
            "早点休息吧，明天还有精神~",
        ],
    );
    m
});

/// 星期问候（0=周一 … 6=周日）
static WEEKDAY_TOPICS: [&str; 7] = [
    "新的一周开始啦~这周有什么目标吗？",
    "周一的忙碌模式开启了吗？",
    "周二啦，进入状态了吗？",
    "周三了，这周过半了哦~",
    "周四啦，周末就在眼前！",
    "周五啦~周末有什么计划吗？",
    "周末愉快呀~有什么好玩的吗？",
];

/// 日常话题池
pub struct DailyTopicPool;

impl DailyTopicPool {
    /// 随机获取一句日常话题
    pub fn random(period: Option<Period>) -> Option<&'static str> {
        let p = period.unwrap_or_else(|| {
            let hour = chrono::Local::now().format("%H").to_string().parse::<u32>().unwrap_or(12);
            Period::now(hour)
        });
        let pool = DAILY_POOLS.get(p.as_str())?;
        if pool.is_empty() {
            return None;
        }
        let idx = random_index(pool.len());
        Some(pool[idx])
    }

    /// 获取星期感知问候
    pub fn weekday_greeting() -> Option<&'static str> {
        let weekday = chrono::Local::now().weekday().num_days_from_monday() as usize;
        Some(WEEKDAY_TOPICS[weekday % 7])
    }

    /// 综合时段 + 星期，返回最合适的一句
    pub fn random_with_context() -> Option<&'static str> {
        // 70% 时段话题，30% 星期问候
        let r = super::super::random_f64();
        if r < 0.7 {
            Self::random(None).or_else(Self::weekday_greeting)
        } else {
            Self::weekday_greeting().or_else(|| Self::random(None))
        }
    }
}

// ============ 兴趣扩展 ============

/// 兴趣扩展项
#[derive(Debug, Clone)]
pub struct InterestExtension {
    pub topic: &'static str,
    pub prompt: &'static str,
}

/// 兴趣标签 → 扩展子话题
static INTEREST_EXTENSIONS: Lazy<Vec<(&'static str, Vec<InterestExtension>)>> = Lazy::new(|| {
    vec![
        (
            "编程",
            vec![
                InterestExtension { topic: "Python", prompt: "你平时用 Python 多吗？" },
                InterestExtension { topic: "Web开发", prompt: "有在做什么前端/后端项目吗？" },
                InterestExtension { topic: "开源项目", prompt: "最近有关注什么有趣的开源项目吗？" },
            ],
        ),
        (
            "游戏",
            vec![
                InterestExtension { topic: "独立游戏", prompt: "你喜欢玩独立游戏吗？" },
                InterestExtension { topic: "联机游戏", prompt: "最近在跟朋友一起玩什么？" },
                InterestExtension { topic: "游戏音乐", prompt: "游戏里的音乐有好听的~你有喜欢的吗？" },
            ],
        ),
        (
            "音乐",
            vec![
                InterestExtension { topic: "现场演出", prompt: "最近有去看演唱会或音乐节吗？" },
                InterestExtension { topic: "乐器", prompt: "你会玩什么乐器吗？" },
            ],
        ),
        (
            "电影",
            vec![
                InterestExtension { topic: "动画电影", prompt: "你喜欢看动画电影吗？" },
                InterestExtension { topic: "纪录片", prompt: "有推荐的纪录片吗？" },
            ],
        ),
        (
            "阅读",
            vec![
                InterestExtension { topic: "科幻", prompt: "你喜欢看科幻小说吗？" },
                InterestExtension { topic: "轻小说", prompt: "有在看什么轻小说吗？" },
            ],
        ),
        (
            "运动",
            vec![
                InterestExtension { topic: "跑步", prompt: "你平时有跑步的习惯吗？" },
                InterestExtension { topic: "瑜伽", prompt: "有试过瑜伽吗？" },
            ],
        ),
        (
            "旅行",
            vec![
                InterestExtension { topic: "国内旅行", prompt: "国内有什么想去的地方吗？" },
                InterestExtension { topic: "美食旅行", prompt: "会为了美食去一个地方旅行吗？" },
            ],
        ),
        (
            "AI",
            vec![
                InterestExtension { topic: "大语言模型", prompt: "你有在用 ChatGPT 之类的 AI 工具吗？" },
                InterestExtension { topic: "AI绘画", prompt: "有玩过 AI 绘画吗？" },
            ],
        ),
        (
            "数码",
            vec![
                InterestExtension { topic: "手机", prompt: "最近有换手机的打算吗？" },
                InterestExtension { topic: "相机", prompt: "你喜欢拍照吗？用什么设备？" },
            ],
        ),
    ]
});

/// 兴趣扩展器
pub struct InterestExtender;

impl InterestExtender {
    /// 根据兴趣标签返回可扩展的子话题列表
    pub fn extend(interest_tags: &[String]) -> Vec<InterestExtension> {
        if interest_tags.is_empty() {
            return Vec::new();
        }
        let lower: Vec<String> = interest_tags.iter().map(|t| t.to_lowercase()).collect();
        let mut results = Vec::new();
        let mut seen: Vec<&'static str> = Vec::new();
        for (tag, exts) in INTEREST_EXTENSIONS.iter() {
            if lower.contains(&tag.to_lowercase()) {
                for ext in exts {
                    if !seen.contains(&ext.topic) {
                        seen.push(ext.topic);
                        results.push(ext.clone());
                    }
                }
            }
        }
        results
    }

    /// 随机选一个扩展话题的提问
    pub fn random_extension(interest_tags: &[String]) -> Option<&'static str> {
        let exts = Self::extend(interest_tags);
        if exts.is_empty() {
            return None;
        }
        let idx = random_index(exts.len());
        Some(exts[idx].prompt)
    }
}

// ============ 话题冷却池（Rust 增强项） ============

/// 话题使用记录（持久化）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TopicUsageState {
    /// topic_key ("category/name") → 上次使用时间戳
    #[serde(default)]
    pub last_used: HashMap<String, f64>,
}

/// 话题冷却池：避免重复，按关系阶段/时间/兴趣加权
pub struct TopicPool {
    state: RwLock<TopicUsageState>,
    persistence_path: PathBuf,
    /// 默认冷却秒数（同话题 6 小时内不重复）
    cooldown_seconds: f64,
}

impl TopicPool {
    pub fn new() -> VivianResult<Self> {
        let dir = get_user_data_dir().join("proactive");
        std::fs::create_dir_all(&dir)
            .map_err(|e| VivianError::Memory(format!("创建主动对话目录失败: {e}")))?;
        let path = dir.join("topics.json");
        let state =
            crate::utils::fs::load_json_or_backup::<TopicUsageState>(&path).unwrap_or_default();
        Ok(Self {
            state: RwLock::new(state),
            persistence_path: path,
            cooldown_seconds: 6.0 * 3600.0,
        })
    }

    /// 标记话题已使用
    pub fn record_used(&self, topic_key: &str, now: f64) {
        self.state
            .write()
            .last_used
            .insert(topic_key.to_string(), now);
    }

    /// 话题是否冷却完毕
    pub fn is_cooled_down(&self, topic_key: &str, now: f64) -> bool {
        let state = self.state.read();
        match state.last_used.get(topic_key) {
            None => true,
            Some(t) => now - t >= self.cooldown_seconds,
        }
    }

    /// 选取一句话题问句（综合冷却 + 兴趣 + 关系阶段权重）
    ///
    /// - `interest_tags`：用户兴趣标签
    /// - `intimacy`：亲密度（0-100），低亲密度过滤"情感/宠物"等私人话题
    /// - `now`：当前时间戳
    pub fn pick_prompt(
        &self,
        interest_tags: &[String],
        intimacy: f64,
        now: f64,
    ) -> Option<&'static str> {
        // 收集候选并加权
        let mut weighted: Vec<(&'static TopicNode, u32)> = Vec::new();
        for node in TopicTree::all_topics() {
            let key = format!("{}/{}", node.category, node.name);
            if !self.is_cooled_down(&key, now) {
                continue;
            }
            // 低亲密度过滤私人话题
            if intimacy < 30.0 && (node.category == "宠物") {
                continue;
            }
            // 权重：基础 1，命中兴趣 ×3，日常话题 ×2（更自然）
            let mut weight: u32 = 1;
            if !interest_tags.is_empty() {
                let lower: Vec<String> = interest_tags.iter().map(|t| t.to_lowercase()).collect();
                if node
                    .interest_tags
                    .iter()
                    .any(|t| lower.contains(&t.to_lowercase()))
                {
                    weight = 3;
                }
            }
            if node.category == "日常" {
                weight += 1;
            }
            weighted.push((node, weight));
        }
        if weighted.is_empty() {
            // 全部冷却时回退到话题树随机（忽略冷却）
            return TopicTree::random_prompt(if interest_tags.is_empty() {
                None
            } else {
                Some(interest_tags)
            });
        }
        let total: u32 = weighted.iter().map(|(_, w)| *w).sum();
        if total == 0 {
            return None;
        }
        let r = super::super::random_f64() * total as f64;
        let mut acc: f64 = 0.0;
        for (node, w) in weighted.iter() {
            acc += *w as f64;
            if r <= acc {
                let prompts = node.prompts;
                if prompts.is_empty() {
                    return None;
                }
                let idx = random_index(prompts.len());
                return Some(prompts[idx]);
            }
        }
        // 兜底
        let last = weighted.last()?;
        let prompts = last.0.prompts;
        if prompts.is_empty() {
            None
        } else {
            let idx = random_index(prompts.len());
            Some(prompts[idx])
        }
    }

    /// 选取并立即标记使用
    pub fn pick_and_record(
        &self,
        interest_tags: &[String],
        intimacy: f64,
        now: f64,
    ) -> Option<&'static str> {
        let prompt = self.pick_prompt(interest_tags, intimacy, now)?;
        // 反查所属话题以记录 key（首次命中即可）
        for node in TopicTree::all_topics() {
            if node.prompts.iter().any(|p| *p == prompt) {
                let key = format!("{}/{}", node.category, node.name);
                self.record_used(&key, now);
                break;
            }
        }
        Some(prompt)
    }

    /// 持久化到磁盘
    pub fn save(&self) -> VivianResult<()> {
        let state = self.state.read().clone();
        let json = serde_json::to_string_pretty(&state)
            .map_err(|e| VivianError::Memory(format!("序列化话题池失败: {e}")))?;
        let tmp = self.persistence_path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)
            .map_err(|e| VivianError::Memory(format!("写入话题池临时文件失败: {e}")))?;
        std::fs::rename(&tmp, &self.persistence_path)
            .map_err(|e| VivianError::Memory(format!("替换话题池文件失败: {e}")))?;
        Ok(())
    }

    /// 获取状态摘要
    pub fn status(&self) -> serde_json::Value {
        let state = self.state.read();
        serde_json::json!({
            "tracked_topics": state.last_used.len(),
            "cooldown_seconds": self.cooldown_seconds,
        })
    }
}

impl Default for TopicPool {
    fn default() -> Self {
        Self::new().unwrap_or_else(|e| {
            tracing::warn!("话题池初始化失败，使用内存模式: {e}");
            Self {
                state: RwLock::new(TopicUsageState::default()),
                persistence_path: PathBuf::from("topics.json"),
                cooldown_seconds: 6.0 * 3600.0,
            }
        })
    }
}

use chrono::Datelike;
