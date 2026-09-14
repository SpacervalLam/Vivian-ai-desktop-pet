//! Self Model —— 角色对自身的统一认知状态。
//!
//! 核心问题：散落的自我状态碎片（PetMindState / ignored_count / quiet_mode /
//! PresenceState / fatigue / FocusState / BehaviorMode）让 prompt 注入和决策
//! 逻辑无法看到完整的"我正在做什么、我今天的节奏如何"。SelfState 把这些
//! 碎片聚合为只读快照，供 prompt 序列化与 proactive 决策查询。
//!
//! 设计原则：
//! - **只读聚合视图**：SelfState 不拥有状态所有权，现有状态留在 ProactiveState /
//!   PresencePersistState / FocusState 等原所有者处。SelfState 持有 Arc 引用，
//!   snapshot() 时统一读取。
//! - **单一持久化字段**：只有 `proactive_initiated_today` 是真空白需新建，
//!   其他字段从原所有者读取。持久化路径：`characters/<char_id>/self_state.json`。
//! - **派生值不持久化**：social_satisfaction 从 intimacy / loneliness / closeness
//!   实时计算，fatigue 从 MoodSnapshot 读取，都不存储。
//!
//! 使用方式：
//! - prompt 注入：`self_state.snapshot().serialize_for_prompt()` → "当前自我状态"段落
//! - proactive 决策：`self_state.snapshot().should_lay_low()` → 是否应该收敛主动行为
//! - 防打扰：`snapshot().proactive_initiated_today >= threshold` → 跳过主动搭话

use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::character_behavior::get_behavior;
use crate::mind::Mind;
use crate::presence::PresenceManager;
use crate::presence::PresenceState;
use crate::proactive::mind_state::PetMindState;
use crate::proactive::ProactiveOrchestrator;
use crate::psychology::PsychologyManager;
use crate::utils::path::get_character_data_dir;

// ── 持久化结构（仅 proactive_initiated_today 是新字段） ──

/// SelfState 持久化部分（按 char_id 分桶）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SelfStatePersist {
    /// 今日主动发起次数（用户主动消息不计数）
    #[serde(default)]
    proactive_initiated_today: u32,
    /// 上次重置日期（YYYY-MM-DD），跨日时重置计数
    #[serde(default)]
    last_reset_date: String,
}

/// "当前正在做什么"的统一枚举（折叠 task_in_progress / FocusState / BehaviorMode）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentActivity {
    /// 空闲，没有特定任务
    Idle,
    /// 正在与用户对话
    Talking,
    /// 凝神模式（深度思考）
    Focusing,
    /// 后台知识采集（Busy）
    GatheringKnowledge,
    /// 后台记忆沉淀（Rest）
    ConsolidatingMemory,
    /// 影随模式
    FollowingCursor,
    /// 守护模式
    Guardian,
    /// 陪伴模式
    Companion,
}

impl CurrentActivity {
    pub fn as_str(&self) -> &'static str {
        match self {
            CurrentActivity::Idle => "idle",
            CurrentActivity::Talking => "talking",
            CurrentActivity::Focusing => "focusing",
            CurrentActivity::GatheringKnowledge => "gathering_knowledge",
            CurrentActivity::ConsolidatingMemory => "consolidating_memory",
            CurrentActivity::FollowingCursor => "following_cursor",
            CurrentActivity::Guardian => "guardian",
            CurrentActivity::Companion => "companion",
        }
    }
}

/// SelfState 只读快照（prompt 注入与决策查询用）
#[derive(Debug, Clone, Serialize)]
pub struct SelfStateSnapshot {
    /// 角色当前心理状态（Curious / Bored / Excited / ...）
    pub mind_state: PetMindState,
    /// 在场状态（Online / Busy / Rest / Offline）
    pub presence: PresenceState,
    /// 在场状态持续时间（秒）
    pub presence_since_secs: f64,
    /// "当前正在做什么"统一视图
    pub current_activity: CurrentActivity,
    /// 今日主动发起次数
    pub proactive_initiated_today: u32,
    /// 连续被忽略次数
    pub ignored_count: u32,
    /// 是否处于安静模式
    pub quiet_mode: bool,
    /// 安静模式剩余时间（秒），非安静模式为 0
    pub quiet_mode_remaining_secs: f64,
    /// 疲劳度（0-100，从 MoodSnapshot 派生）
    pub fatigue: f64,
    /// 社交满足度（0.0-1.0，从 intimacy / loneliness / closeness 派生）
    pub social_satisfaction: f64,
    /// 社交冲动（0.0-1.0，从 loneliness + 社交 Goal 优先级 + Attention 聚焦用户 派生）
    ///
    /// Mind Tick 的认知副产品：当 loneliness 高 + 有活跃社交目标 + 注意力在用户身上时，
    /// social_urge 升高，通过 prompt 注入让 LLM 生成更主动的回复。
    pub social_urge: f64,
    /// 最近一次发言距现在的秒数（None = 从未发言）
    pub last_spoken_secs_ago: Option<f64>,
    /// 角色角色化行为参数（用于阈值查询）
    pub behavior_quiet_mode_threshold: u32,
}

impl SelfStateSnapshot {
    /// 是否应该收敛主动行为（防打扰）
    ///
    /// 触发条件：
    /// - 安静模式中
    /// - 今日主动次数 ≥ 角色阈值（默认 8 次）
    /// - 被忽略次数已达安静模式阈值的 80%
    /// - 在场状态为 Rest / Offline
    pub fn should_lay_low(&self) -> bool {
        if self.quiet_mode {
            return true;
        }
        // 今日主动次数上限（角色差异化：Vivian 8 / Nana 12，温柔角色更主动）
        let proactive_cap = if self.behavior_quiet_mode_threshold <= 2 {
            12 // Nana（阈值 2）更主动
        } else {
            8 // Vivian（阈值 5）更克制
        };
        if self.proactive_initiated_today >= proactive_cap {
            return true;
        }
        // 被忽略次数接近阈值（80%）
        let ignored_near_threshold =
            self.ignored_count as f64 >= self.behavior_quiet_mode_threshold as f64 * 0.8;
        if ignored_near_threshold {
            return true;
        }
        // Rest / Offline 不主动
        matches!(self.presence, PresenceState::Rest | PresenceState::Offline)
    }

    /// 序列化为 prompt 段落
    ///
    /// 渲染为"当前自我状态"区块，让 LLM 感知自己正在做什么、今天的节奏如何。
    /// 空 activity / 默认值时返回简短文本，避免污染 prompt。
    pub fn serialize_for_prompt(&self, lang: &str) -> String {
        use crate::presence::PresenceState;
        use crate::proactive::mind_state::PetMindState;

        let lang = crate::pipeline::prompt_modules::normalize_lang(lang);

        let mut lines: Vec<String> = Vec::new();

        // What you're doing right now — natural narrative
        let presence_note = match self.presence {
            PresenceState::Online => match self.current_activity.as_str() {
                "" | "idle" => match lang {
                    "zh" => "你闲待着，没做什么特别的事。".to_string(),
                    "ja" => "ぶらぶらしていて、特に何もしていない。".to_string(),
                    _ => "You're hanging around, not doing anything in particular.".to_string(),
                },
                act => match lang {
                    "zh" => format!("你正在{}。", act),
                    "ja" => format!("今{}している。", act),
                    _ => format!("You're {} right now.", act),
                },
            },
            PresenceState::Busy => match self.current_activity.as_str() {
                "" | "idle" => match lang {
                    "zh" => "你在，但有点在忙自己的事。".to_string(),
                    "ja" => "いるけど、自分のことをしている。".to_string(),
                    _ => "You're around but kind of doing your own thing.".to_string(),
                },
                act => match lang {
                    "zh" => format!("你正在{}——有点忙。", act),
                    "ja" => format!("今{}している——少し手が離せない。", act),
                    _ => format!("You're {} right now — a bit preoccupied.", act),
                },
            },
            PresenceState::Rest => match lang {
                "zh" => "你正在休息，放松一下。".to_string(),
                "ja" => "今休憩中、のんびりしている。".to_string(),
                _ => "You're resting right now, taking it easy.".to_string(),
            },
            PresenceState::Offline => match lang {
                "zh" => "你暂时不在。".to_string(),
                "ja" => "今は少し離れている。".to_string(),
                _ => "You're away for the moment.".to_string(),
            },
        };
        lines.push(presence_note);

        // Mind state (only mention when it meaningfully colors the moment)
        match self.mind_state {
            PetMindState::Bored => lines.push(
                match lang {
                    "zh" => "说实话，有点无聊。",
                    "ja" => "正直、ちょっと退屈。",
                    _ => "You're kind of bored, honestly.",
                }
                .to_string(),
            ),
            PetMindState::Curious => lines.push(
                match lang {
                    "zh" => "你的思绪在飘——对什么感到好奇。",
                    "ja" => "思考が彷徨っている——何かに好奇心がある。",
                    _ => "Your mind is wandering — you're curious about something.",
                }
                .to_string(),
            ),
            PetMindState::Excited => lines.push(
                match lang {
                    "zh" => "现在有点兴奋。",
                    "ja" => "今ちょっとワクワクしている。",
                    _ => "You're feeling kind of excited right now.",
                }
                .to_string(),
            ),
            PetMindState::Sleepy => lines.push(
                match lang {
                    "zh" => "有点困了。",
                    "ja" => "眠くなってきた。",
                    _ => "You're getting sleepy.",
                }
                .to_string(),
            ),
            PetMindState::Tired => lines.push(
                match lang {
                    "zh" => "累坏了。",
                    "ja" => "疲れ切っている。",
                    _ => "You're worn out.",
                }
                .to_string(),
            ),
            PetMindState::Playful => lines.push(
                match lang {
                    "zh" => "心情挺玩闹的。",
                    "ja" => "遊び気分だ。",
                    _ => "You're in a playful mood.",
                }
                .to_string(),
            ),
            PetMindState::Caring => lines.push(
                match lang {
                    "zh" => "现在感觉很温柔，想照顾人。",
                    "ja" => "今は優しい気持ちで、世話を焼きたくなる。",
                    _ => "You're feeling soft and caring right now.",
                }
                .to_string(),
            ),
            PetMindState::Content => {} // quiet contentment is default, no need to announce
        }

        // Fatigue — only when notably tired (PetMindState::Tired handles strong fatigue; add nuance)
        if self.fatigue > 70.0 {
            lines.push(
                match lang {
                    "zh" => "真挺累的。",
                    "ja" => "正直かなり疲れた。",
                    _ => "Pretty tired honestly.",
                }
                .to_string(),
            );
        } else if self.fatigue > 50.0 && self.mind_state != PetMindState::Tired {
            lines.push(
                match lang {
                    "zh" => "开始有点累了。",
                    "ja" => "少し疲れてきた。",
                    _ => "Starting to feel a bit worn out.",
                }
                .to_string(),
            );
        }

        // Social state
        if self.social_satisfaction < 0.3 {
            lines.push(
                match lang {
                    "zh" => "今天没怎么聊天——有人陪就好了。",
                    "ja" => "今日はあまり話していない——誰かと話したい気分。",
                    _ => "Haven't chatted much today — wouldn't mind some company.",
                }
                .to_string(),
            );
        } else if self.social_satisfaction > 0.85 {
            lines.push(
                match lang {
                    "zh" => "暂时社交够了。",
                    "ja" => "今は十分社交した。",
                    _ => "You've had enough socializing for now.",
                }
                .to_string(),
            );
        }
        if self.social_urge > 0.7 {
            lines.push(
                match lang {
                    "zh" => "有点想说点什么。",
                    "ja" => "何か言いたい気分。",
                    _ => "Kind of feel like saying something.",
                }
                .to_string(),
            );
        }

        // Quiet mode — don't initiate
        if self.quiet_mode {
            lines.push(
                match lang {
                    "zh" => "暂时自己待着——不会主动搭话。",
                    "ja" => "今は一人でいる——会話は始めない。",
                    _ => "Keeping to yourself for now — not starting conversations.",
                }
                .to_string(),
            );
        }

        // Recently ignored — naturally affects willingness
        if self.ignored_count >= 3 {
            lines.push(
                match lang {
                    "zh" => "最近他有点忽略你。有点难受。",
                    "ja" => "最近少し無視されている。ちょっと傷つく。",
                    _ => "They've been ignoring you a bit lately. It stings.",
                }
                .to_string(),
            );
        } else if self.ignored_count >= 1 {
            lines.push(
                match lang {
                    "zh" => "你最后几条消息没被回复。不勉强了。",
                    "ja" => "最後の数メッセージが返信されなかった。これ以上押し付けない。",
                    _ => "Your last couple of messages went unanswered. Not pushing it.",
                }
                .to_string(),
            );
        }

        if lines.is_empty() {
            return match lang {
                "zh" => "## 当下状态\n你在，做着自己的事。".to_string(),
                "ja" => "## 今の状態\nいて、自分のことをしている。".to_string(),
                _ => "## Right Now\nYou're here, doing your thing.".to_string(),
            };
        }

        let header = match lang {
            "zh" => "## 当下状态",
            "ja" => "## 今の状態",
            _ => "## Right Now",
        };
        format!("{}\n{}", header, lines.join(" "))
    }
}

/// SelfState —— 角色自我状态聚合器
///
/// 持有 proactive / presence / psychology 的 Arc 引用，
/// snapshot() 时统一读取并派生社交满足度等指标。
pub struct SelfState {
    pub char_id: String,
    proactive: Arc<ProactiveOrchestrator>,
    presence: Arc<PresenceManager>,
    psychology: Arc<PsychologyManager>,
    mind: Arc<Mind>,
    persist: RwLock<SelfStatePersist>,
    persistence_path: std::path::PathBuf,
}

impl SelfState {
    /// 从已有组件构建 SelfState
    pub fn new(
        char_id: &str,
        proactive: Arc<ProactiveOrchestrator>,
        presence: Arc<PresenceManager>,
        psychology: Arc<PsychologyManager>,
        mind: Arc<Mind>,
    ) -> Self {
        let char_dir = get_character_data_dir(char_id);
        let path = char_dir.join("self_state.json");
        let persist = match std::fs::read_to_string(&path) {
            Ok(content) => {
                let mut p: SelfStatePersist = serde_json::from_str(&content)
                    .unwrap_or_default();
                // 启动时跨日重置
                let today = today_str();
                if p.last_reset_date != today {
                    p.proactive_initiated_today = 0;
                    p.last_reset_date = today;
                }
                p
            }
            Err(_) => SelfStatePersist {
                proactive_initiated_today: 0,
                last_reset_date: today_str(),
            },
        };

        Self {
            char_id: char_id.to_string(),
            proactive,
            presence,
            psychology,
            mind,
            persist: RwLock::new(persist),
            persistence_path: path,
        }
    }

    /// 生成只读快照
    pub fn snapshot(&self) -> SelfStateSnapshot {
        let now = chrono::Local::now().timestamp() as f64;

        // 跨日重置
        {
            let today = today_str();
            let mut p = self.persist.write();
            if p.last_reset_date != today {
                p.proactive_initiated_today = 0;
                p.last_reset_date = today;
            }
        }

        let persist = self.persist.read();

        // proactive 状态
        let status = self.proactive.get_status();
        let mind_state_str = status
            .get("mind_state")
            .and_then(|v| v.as_str())
            .unwrap_or("curious");
        let mind_state = PetMindState::from_str(mind_state_str);
        let ignored_count = status
            .get("ignored_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let quiet_mode = status
            .get("quiet_mode")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let quiet_mode_until = status
            .get("quiet_mode_until")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let quiet_mode_remaining = if quiet_mode {
            (quiet_mode_until - now).max(0.0)
        } else {
            0.0
        };

        // behavior_mode
        let behavior_mode_str = status
            .get("behavior_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("none");

        // presence 状态
        let presence_state = self.presence.current();
        let presence_since = self.presence.since();
        let presence_since_secs = (now - presence_since).max(0.0);

        // fatigue 从 MoodSnapshot 读取
        let mood = self.psychology.compute_mood();
        let fatigue = mood.fatigue;

        // social_satisfaction: 从 intimacy / loneliness / closeness 派生
        // 公式：(intimacy + (1 - loneliness) + closeness) / 3
        let relationship = self.psychology.relationship();
        let emotion = self.psychology.emotion();
        let social_satisfaction = ((relationship.intimacy
            + (1.0 - emotion.loneliness.clamp(0.0, 1.0))
            + emotion.closeness.clamp(0.0, 1.0))
            / 3.0)
            .clamp(0.0, 1.0);

        // social_urge: Mind Tick 的认知副产品
        // = loneliness * 0.5 + 社交 Goal 最高优先级 * 0.3 + Attention 聚焦 user * 0.2
        let loneliness = emotion.loneliness.clamp(0.0, 1.0);
        let social_goal_priority = {
            let goals = self.mind.goals.read();
            goals.active_sorted()
                .iter()
                .filter(|g| {
                    let d = g.description.to_lowercase();
                    d.contains("陪伴") || d.contains("聊天") || d.contains("主人")
                        || d.contains("talk") || d.contains("company") || d.contains("user")
                })
                .map(|g| g.priority)
                .next()
                .unwrap_or(0.0)
        };
        let attention_on_user = {
            let att = self.mind.attention.read();
            att.focus.get("user").map(|f| f.weight as f64).unwrap_or(0.0)
        };
        let social_urge = (loneliness * 0.5
            + social_goal_priority * 0.3
            + attention_on_user * 0.2)
            .clamp(0.0, 1.0);

        // last_spoken
        let last_spoken_secs_ago = crate::commands::proactive::last_spoken_ago(&self.char_id);

        // current_activity: 折叠多个碎片
        let current_activity = self.resolve_current_activity(
            presence_state,
            mind_state,
            behavior_mode_str,
        );

        // 角色行为参数
        let behavior = get_behavior(&self.char_id);
        let behavior_quiet_mode_threshold = behavior.quiet_mode_threshold;

        SelfStateSnapshot {
            mind_state,
            presence: presence_state,
            presence_since_secs,
            current_activity,
            proactive_initiated_today: persist.proactive_initiated_today,
            ignored_count,
            quiet_mode,
            quiet_mode_remaining_secs: quiet_mode_remaining,
            fatigue,
            social_satisfaction,
            social_urge,
            last_spoken_secs_ago,
            behavior_quiet_mode_threshold,
        }
    }

    /// 折叠多个碎片为统一的"当前活动"
    fn resolve_current_activity(
        &self,
        presence: PresenceState,
        mind_state: PetMindState,
        behavior_mode: &str,
    ) -> CurrentActivity {
        // 优先级：后台任务 > 凝神模式 > behavior_mode > presence > mind_state

        // 后台任务（Busy = 知识采集 / Rest = 记忆沉淀）
        if presence == PresenceState::Busy {
            return CurrentActivity::GatheringKnowledge;
        }
        if presence == PresenceState::Rest {
            return CurrentActivity::ConsolidatingMemory;
        }

        // behavior_mode
        match behavior_mode {
            "follow" => return CurrentActivity::FollowingCursor,
            "guardian" => return CurrentActivity::Guardian,
            "companion" => return CurrentActivity::Companion,
            _ => {}
        }

        // 凝神模式（通过 mind_state 粗略推断，精确判断需 FocusState 注入）
        if mind_state == PetMindState::Curious {
            // Curious 是默认值，不一定是 focusing，保持 Idle
        }

        CurrentActivity::Idle
    }

    /// 记录一次主动发起（proactive 触发成功后调用）
    pub fn record_proactive_initiative(&self) {
        let mut p = self.persist.write();
        let today = today_str();
        if p.last_reset_date != today {
            p.proactive_initiated_today = 0;
            p.last_reset_date = today;
        }
        p.proactive_initiated_today += 1;
    }

    /// 持久化
    pub fn persist(&self) -> std::io::Result<()> {
        if let Some(parent) = self.persistence_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let p = self.persist.read();
        let content = serde_json::to_string_pretty(&*p)?;
        std::fs::write(&self.persistence_path, content)?;
        Ok(())
    }
}

fn today_str() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}
