//! 用户事实画像层
//!
//! 独立于 MemoryManager 的结构化用户事实存储。
//!
//! 核心能力：
//! - 从每轮对话中实时提取显式用户事实（低温度 LLM，走 `memory` 路由）
//! - 智能合并：旧值优先，空白字段直接填充，冲突时 LLM 仲裁
//! - 检索时作为独立召回源注入 prompt
//!
//! 持久化：`%APPDATA%\vivian\characters\<char_id>\user_facts.json`（按角色隔离）

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};
use crate::utils::path;
use crate::types::response::ChatMessage;

/// 用户事实类型
/// L0 基础身份（5 字段）+ L0.5 结构化偏好（5 字段）+ L2 自由事实
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserFactType {
    // === L0 稳定身份 ===
    /// 姓名
    Name,
    /// 年龄
    Age,
    /// 性别
    Gender,
    /// 职业
    Occupation,
    /// 所在地
    Location,
    // === L0.5 结构化偏好 ===
    /// 生日（如 "1995-08-15"）
    Birthday,
    /// 作息习惯（如 "通常 23 点睡 7 点起"）
    SleepSchedule,
    /// 常用网站（如 "B站看番"、"GitHub"）
    FavoriteWebsite,
    /// 喜欢的游戏（如 "原神"、"塞尔达"）
    FavoriteGame,
    /// 其他长期兴趣（如 "摄影"、"钢琴"）
    Hobby,
    // === L2 自由事实 ===
    /// 自由事实（如"养了一只猫"、"在准备考试"）
    Custom,
}

impl UserFactType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "name" | "姓名" | "名字" => Some(Self::Name),
            "age" | "年龄" => Some(Self::Age),
            "gender" | "性别" => Some(Self::Gender),
            "occupation" | "职业" | "工作" => Some(Self::Occupation),
            "location" | "所在地" | "位置" => Some(Self::Location),
            "birthday" | "生日" => Some(Self::Birthday),
            "sleep_schedule" | "sleep" | "作息" => Some(Self::SleepSchedule),
            "favorite_website" | "website" | "网站" => Some(Self::FavoriteWebsite),
            "favorite_game" | "game" | "游戏" => Some(Self::FavoriteGame),
            "hobby" | "兴趣" | "爱好" => Some(Self::Hobby),
            "custom" | "其他" => Some(Self::Custom),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Age => "age",
            Self::Gender => "gender",
            Self::Occupation => "occupation",
            Self::Location => "location",
            Self::Birthday => "birthday",
            Self::SleepSchedule => "sleep_schedule",
            Self::FavoriteWebsite => "favorite_website",
            Self::FavoriteGame => "favorite_game",
            Self::Hobby => "hobby",
            Self::Custom => "custom",
        }
    }

    /// 中文显示名
    pub fn label_zh(&self) -> &'static str {
        match self {
            Self::Name => "姓名",
            Self::Age => "年龄",
            Self::Gender => "性别",
            Self::Occupation => "职业",
            Self::Location => "所在地",
            Self::Birthday => "生日",
            Self::SleepSchedule => "作息习惯",
            Self::FavoriteWebsite => "常用网站",
            Self::FavoriteGame => "喜欢的游戏",
            Self::Hobby => "兴趣爱好",
            Self::Custom => "其他",
        }
    }

    /// 是否为唯一字段（每个类型最多 1 条，非 Custom）
    pub fn is_basic(&self) -> bool {
        !matches!(self, Self::Custom)
    }
}

/// 单条用户事实
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserFact {
    /// 事实类型
    pub fact_type: UserFactType,
    /// 事实内容
    pub content: String,
    /// 置信度 0.0-1.0
    pub confidence: f64,
    /// 提取时间戳（Unix 秒）
    pub timestamp: f64,
    /// 来源记忆 ID（可选，用于溯源）
    pub source_memory_id: Option<String>,
    /// LLM 推理过程（可选）
    pub reasoning: Option<String>,
    /// 是否锁定（L0 层用，锁定后不会被自动覆盖或冲突仲裁改写）
    #[serde(default)]
    pub is_pinned: bool,
    /// 来源叙事背景（"为什么"存这条，帮助消歧；对应书中 Advanced JSON Cards 的 backstory）
    #[serde(default)]
    pub backstory: Option<String>,
    /// 主体身份（该事实关于谁，如用户自己/家人/朋友；对应书中 Advanced JSON Cards 的 person）
    #[serde(default)]
    pub person: Option<String>,
    /// 与主体的关系上下文（"为谁"存，避免同名实体混淆）
    #[serde(default)]
    pub relationship: Option<String>,
}

/// L1 近期状态层 — 用户最近的目标、项目、偏好（带轮次衰减）
///
/// 与 L0（basic_data 稳定身份）和 L2（custom_facts 长期事实）互补：
/// L1 记录的是"最近在忙什么""最近关心什么"，会随时间被新状态覆盖。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct L1RecentState {
    /// 最近目标（如"准备考研""找工作""做完项目X"）
    #[serde(default)]
    pub recent_goals: Vec<String>,
    /// 当前项目（如"在开发一个网站""在写毕业论文"）
    #[serde(default)]
    pub current_projects: Vec<String>,
    /// 近期偏好（如"最近在听后摇""最近迷上原神"）
    #[serde(default)]
    pub recent_preferences: Vec<String>,
    /// 生成时间戳
    #[serde(default)]
    pub generated_at: f64,
    /// 累计轮次计数（用于判断近期状态的新鲜度）
    #[serde(default)]
    pub round_count: u32,
}

/// LLM 抽取结果
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
struct ExtractedFacts {
    #[serde(default)]
    detected: bool,
    #[serde(default)]
    items: Vec<ExtractedItem>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct ExtractedItem {
    /// 类型字符串（name/age/gender/occupation/location/custom）
    #[serde(rename = "type")]
    item_type: String,
    content: String,
    #[serde(default = "default_confidence")]
    confidence: f64,
    #[serde(default)]
    reasoning: Option<String>,
}

fn default_confidence() -> f64 {
    0.8
}

/// LLM 冲突仲裁结果
#[derive(Debug, Clone, Deserialize)]
struct ConflictResolution {
    /// choose_old / choose_new / merge
    decision: String,
    /// 最终内容
    final_content: String,
    #[serde(default)]
    reason: Option<String>,
}

/// LLM 客户端抽象（与 EnricherLlmClient 同构）
#[async_trait]
pub trait FactLlmClient: Send + Sync {
    async fn complete(&self, prompt: &str) -> VivianResult<String>;
}

/// 为 ModelRouter 实现
#[async_trait]
impl FactLlmClient for crate::providers::ModelRouter {
    async fn complete(&self, prompt: &str) -> VivianResult<String> {
        let messages = vec![ChatMessage::user(prompt.to_string())];
        let schema = {
            let root = schemars::schema_for!(ExtractedFacts);
            serde_json::to_value(&root.schema).unwrap_or_else(|_| serde_json::json!({"type": "object"}))
        };
        self.generate(crate::providers::base::LLMRequest::new("memory", messages).with_json_schema(schema))
            .await
    }
}

/// 用户事实存储
#[derive(Clone)]
pub struct UserFactStore {
    inner: Arc<RwLock<UserFactStoreInner>>,
    llm: Option<Arc<dyn FactLlmClient>>,
}

struct UserFactStoreInner {
    store_path: PathBuf,
    /// L0 稳定身份：5 个固定字段（每个字段最多 1 条）
    basic_data: HashMap<UserFactType, UserFact>,
    /// L1 近期状态：最近目标/项目/偏好（随时间被新状态覆盖）
    recent_state: L1RecentState,
    /// L2 长期事实：自由事实列表
    custom_facts: Vec<UserFact>,
}

impl UserFactStore {
    /// 创建或加载用户事实存储（按角色隔离，避免人设泄露）
    pub fn new(llm: Option<Arc<dyn FactLlmClient>>, char_id: &str) -> VivianResult<Self> {
        let store_path = path::get_character_data_dir(char_id).join("user_facts.json");

        let mut inner = UserFactStoreInner {
            store_path,
            basic_data: HashMap::new(),
            recent_state: L1RecentState::default(),
            custom_facts: Vec::new(),
        };
        inner.load()?;

        Ok(Self {
            inner: Arc::new(RwLock::new(inner)),
            llm,
        })
    }

    /// 兜底构造：LLM 不可用时使用，只读已有数据
    pub fn fallback() -> Self {
        Self {
            inner: Arc::new(RwLock::new(UserFactStoreInner {
                store_path: PathBuf::new(),
                basic_data: HashMap::new(),
                recent_state: L1RecentState::default(),
                custom_facts: Vec::new(),
            })),
            llm: None,
        }
    }

    /// 从对话中提取并更新用户事实（异步，走 LLM）
    pub async fn extract_and_upsert(
        &self,
        user_input: &str,
        ai_response: &str,
        source_memory_id: Option<&str>,
    ) -> VivianResult<Vec<UserFact>> {
        let llm = match &self.llm {
            Some(llm) => llm,
            None => return Ok(Vec::new()),
        };

        // 构建已有事实字符串，注入 prompt 避免重复抽取
        let existing_facts = self.format_existing_facts();
        let prompt = build_extract_prompt(user_input, ai_response, &existing_facts);
        let resp = llm.complete(&prompt).await?;
        let extracted = parse_extract_response(&resp)?;

        if !extracted.detected || extracted.items.is_empty() {
            return Ok(Vec::new());
        }

        let now = current_timestamp();
        let mut new_facts = Vec::new();

        for item in extracted.items {
            let fact_type = match UserFactType::from_str(&item.item_type) {
                Some(t) => t,
                None => UserFactType::Custom,
            };
            let content = item.content.trim().to_string();
            if content.is_empty() {
                continue;
            }

            let fact = UserFact {
                fact_type,
                content: content.clone(),
                confidence: item.confidence.clamp(0.0, 1.0),
                timestamp: now,
                source_memory_id: source_memory_id.map(|s| s.to_string()),
                reasoning: item.reasoning,
                is_pinned: false,
                backstory: Some(backstory_snippet(user_input)),
                person: None,
                relationship: None,
            };

            self.upsert_fact(fact, llm).await?;
            new_facts.push(UserFact {
                fact_type,
                content,
                confidence: item.confidence.clamp(0.0, 1.0),
                timestamp: now,
                source_memory_id: source_memory_id.map(|s| s.to_string()),
                reasoning: None,
                is_pinned: false,
                backstory: Some(backstory_snippet(user_input)),
                person: None,
                relationship: None,
            });
        }

        Ok(new_facts)
    }

    /// 智能合并写入单条事实
    /// - 基础字段：旧值存在且新值不同时，置信度 < 0.9 调 LLM 仲裁；空白字段直接填充
    /// - 自由事实：语义去重（简单包含判断），不重复添加
    async fn upsert_fact(&self, fact: UserFact, llm: &Arc<dyn FactLlmClient>) -> VivianResult<()> {
        // 基础字段：先短暂读锁判断是否有旧值，避免跨越 await 持有写锁
        if fact.fact_type.is_basic() {
            let existing = {
                let inner = self.inner.read();
                inner.basic_data.get(&fact.fact_type).cloned()
            };

            // 旧值不存在，直接写入
            let existing = match existing {
                None => {
                    let mut inner = self.inner.write();
                    inner.basic_data.insert(fact.fact_type, fact);
                    inner.save()?;
                    return Ok(());
                }
                Some(e) => e,
            };

            // L0 锁定保护：旧值被 is_pinned 锁定时，跳过任何覆盖
            if existing.is_pinned {
                return Ok(());
            }

            // 内容相同：更新置信度取高者
            if existing.content == fact.content {
                if fact.confidence > existing.confidence {
                    let mut inner = self.inner.write();
                    inner.basic_data.insert(
                        fact.fact_type,
                        UserFact {
                            confidence: fact.confidence,
                            reasoning: fact.reasoning,
                            ..existing
                        },
                    );
                    inner.save()?;
                }
                return Ok(());
            }

            // 内容不同且高置信度：直接覆盖
            if fact.confidence >= 0.9 {
                let mut inner = self.inner.write();
                inner.basic_data.insert(fact.fact_type, fact);
                inner.save()?;
                return Ok(());
            }

            // 低置信度：锁外调 LLM 仲裁
            let fact_type_str = fact.fact_type.as_str().to_string();
            let resolution = self
                .resolve_conflict(llm, &fact_type_str, &existing.content, &fact.content)
                .await?;
            let final_fact = match resolution.decision.as_str() {
                "choose_old" => existing,
                "choose_new" => fact,
                _ => UserFact {
                    content: resolution.final_content,
                    confidence: fact.confidence.max(existing.confidence),
                    reasoning: resolution.reason,
                    ..fact
                },
            };
            let mut inner = self.inner.write();
            inner.basic_data.insert(final_fact.fact_type, final_fact);
            inner.save()?;
            return Ok(());
        }

        // 自由事实：简单去重（包含关系）
        let mut inner = self.inner.write();
        let already_exists = inner
            .custom_facts
            .iter()
            .any(|f| f.content.contains(&fact.content) || fact.content.contains(&f.content));
        if !already_exists {
            inner.custom_facts.push(fact);
            inner.save()?;
        }
        Ok(())
    }

    /// LLM 冲突仲裁
    async fn resolve_conflict(
        &self,
        llm: &Arc<dyn FactLlmClient>,
        fact_type: &str,
        old_value: &str,
        new_value: &str,
    ) -> VivianResult<ConflictResolution> {
        let prompt = format!(
            "你是用户信息合并器。用户的事实字段「{fact_type}」出现了冲突：\n\
            旧值：{old_value}\n\
            新值：{new_value}\n\n\
            请判断：\n\
            1. choose_old：旧值更可信（如新值是误识别、玩笑、假设）\n\
            2. choose_new：新值更可信（如用户主动更正、旧值过时）\n\
            3. merge：合并两者（如「北京」和「海淀区」合并为「北京海淀」）\n\n\
            只输出 JSON：{{\"decision\":\"choose_old|choose_new|merge\",\"final_content\":\"最终内容\",\"reason\":\"原因\"}}"
        );
        let resp = llm.complete(&prompt).await?;
        let cleaned = strip_code_fence(&resp);
        let resolution: ConflictResolution = serde_json::from_str(cleaned).map_err(|e| {
            VivianError::Other(format!("解析冲突仲裁响应失败: {e}"))
        })?;
        Ok(resolution)
    }

    /// 获取所有事实（基础 + 自由），按类型分组
    pub fn get_all_facts(&self) -> (HashMap<UserFactType, UserFact>, Vec<UserFact>) {
        let inner = self.inner.read();
        (inner.basic_data.clone(), inner.custom_facts.clone())
    }

    /// 格式化已有事实为 prompt 注入用字符串（L0 基础字段 + L0.5 偏好字段 + L2 自由事实）
    ///
    /// 用于 `build_extract_prompt` 的 `existing_facts` 参数，提醒 LLM 不要重复抽取。
    /// L1 近期状态属短期信息，不在此列。
    fn format_existing_facts(&self) -> String {
        let inner = self.inner.read();
        let mut parts = Vec::new();
        // L0 + L0.5 唯一字段按固定顺序输出
        for fact_type in &[
            UserFactType::Name,
            UserFactType::Age,
            UserFactType::Gender,
            UserFactType::Occupation,
            UserFactType::Location,
            UserFactType::Birthday,
            UserFactType::SleepSchedule,
            UserFactType::FavoriteWebsite,
            UserFactType::FavoriteGame,
            UserFactType::Hobby,
        ] {
            if let Some(fact) = inner.basic_data.get(fact_type) {
                parts.push(format!("- {}: {}", fact_type.as_str(), fact.content));
            }
        }
        // L2 自由事实（最多 10 条，避免 prompt 过长）
        for fact in inner.custom_facts.iter().take(10) {
            parts.push(format!("- {}", fact.content));
        }
        parts.join("\n")
    }

    /// 获取基础字段值（name/age/gender/occupation/location）
    pub fn get_basic(&self, fact_type: UserFactType) -> Option<String> {
        let inner = self.inner.read();
        inner.basic_data.get(&fact_type).map(|f| f.content.clone())
    }

    /// 锁定/解锁某个基础字段（L0/L0.5 层用，锁定后不会被自动覆盖）
    pub fn set_pinned(&self, fact_type: UserFactType, pinned: bool) -> VivianResult<()> {
        let mut inner = self.inner.write();
        if let Some(fact) = inner.basic_data.get_mut(&fact_type) {
            fact.is_pinned = pinned;
            inner.save()?;
        }
        Ok(())
    }

    /// 手动设置/覆盖一条事实（UI 编辑入口）
    ///
    /// 基础字段（L0/L0.5）：直接写入，is_pinned 由参数指定。
    /// Custom 字段：忽略 pinned，按内容去重后追加。
    pub fn set_fact(&self, fact_type: UserFactType, content: &str, pinned: bool) -> VivianResult<()> {
        let content = content.trim().to_string();
        if content.is_empty() {
            return Ok(());
        }
        let mut inner = self.inner.write();
        if fact_type.is_basic() {
            let now = current_timestamp();
            let existing = inner.basic_data.get(&fact_type).cloned();
            let fact = UserFact {
                fact_type,
                content,
                confidence: 1.0,
                timestamp: now,
                source_memory_id: None,
                reasoning: Some("manual_edit".to_string()),
                is_pinned: pinned || existing.map(|e| e.is_pinned).unwrap_or(false),
                backstory: None,
                person: None,
                relationship: None,
            };
            inner.basic_data.insert(fact_type, fact);
        } else {
            let already_exists = inner
                .custom_facts
                .iter()
                .any(|f| f.content == content);
            if !already_exists {
                let now = current_timestamp();
                inner.custom_facts.push(UserFact {
                    fact_type: UserFactType::Custom,
                    content,
                    confidence: 1.0,
                    timestamp: now,
                    source_memory_id: None,
                    reasoning: Some("manual_edit".to_string()),
                    is_pinned: false,
                    backstory: None,
                    person: None,
                    relationship: None,
                });
            }
        }
        inner.save()
    }

    /// 删除一条事实
    ///
    /// 基础字段：删除整个条目。
    /// Custom 字段：按内容匹配删除。
    pub fn delete_fact(&self, fact_type: UserFactType, content: Option<&str>) -> VivianResult<()> {
        let mut inner = self.inner.write();
        if fact_type.is_basic() {
            inner.basic_data.remove(&fact_type);
        } else if let Some(content) = content {
            let content = content.trim();
            inner.custom_facts.retain(|f| f.content != content);
        }
        inner.save()
    }

    /// 更新 L1 近期状态（由 Stage 2 路径 3 调用）
    ///
    /// 新状态会覆盖旧状态（L1 层的设计就是"最近"），并累加 round_count。
    pub fn update_recent_state(&self, goals: Vec<String>, projects: Vec<String>, preferences: Vec<String>) -> VivianResult<()> {
        let mut inner = self.inner.write();
        inner.recent_state.recent_goals = goals;
        inner.recent_state.current_projects = projects;
        inner.recent_state.recent_preferences = preferences;
        inner.recent_state.generated_at = current_timestamp();
        inner.recent_state.round_count = inner.recent_state.round_count.saturating_add(1);
        inner.save()
    }

    /// 获取 L1 近期状态快照
    pub fn get_recent_state(&self) -> L1RecentState {
        self.inner.read().recent_state.clone()
    }

    /// 格式化为 prompt 上下文（L0 稳定身份 + L0.5 偏好档案 + L1 近期状态 + L2 长期事实）
    pub fn format_for_prompt(&self) -> String {
        let inner = self.inner.read();
        if inner.basic_data.is_empty()
            && inner.custom_facts.is_empty()
            && inner.recent_state.recent_goals.is_empty()
            && inner.recent_state.current_projects.is_empty()
            && inner.recent_state.recent_preferences.is_empty()
        {
            return String::new();
        }

        let mut lines = Vec::new();
        lines.push("【用户档案】".to_string());

        // L0 稳定身份：5 个基础字段
        let l0_types = [
            UserFactType::Name,
            UserFactType::Age,
            UserFactType::Gender,
            UserFactType::Occupation,
            UserFactType::Location,
        ];
        let l0_has_content = l0_types.iter().any(|t| inner.basic_data.contains_key(t));
        if l0_has_content {
            for fact_type in &l0_types {
                if let Some(fact) = inner.basic_data.get(fact_type) {
                    lines.push(format!("- {}：{}", fact_type.label_zh(), fact.content));
                }
            }
        }

        // L0.5 偏好档案：5 个结构化偏好字段
        let l05_types = [
            UserFactType::Birthday,
            UserFactType::SleepSchedule,
            UserFactType::FavoriteWebsite,
            UserFactType::FavoriteGame,
            UserFactType::Hobby,
        ];
        let l05_has_content = l05_types.iter().any(|t| inner.basic_data.contains_key(t));
        if l05_has_content {
            lines.push(String::new());
            lines.push("【偏好档案】".to_string());
            for fact_type in &l05_types {
                if let Some(fact) = inner.basic_data.get(fact_type) {
                    lines.push(match &fact.backstory {
                        Some(b) if !b.trim().is_empty() => format!(
                            "- {}：{}（来源：{}）",
                            fact_type.label_zh(),
                            fact.content,
                            b
                        ),
                        _ => format!("- {}：{}", fact_type.label_zh(), fact.content),
                    });
                }
            }
        }

        // L1 近期状态：最近目标/项目/偏好
        let rs = &inner.recent_state;
        let l1_has_content = !rs.recent_goals.is_empty()
            || !rs.current_projects.is_empty()
            || !rs.recent_preferences.is_empty();
        if l1_has_content {
            lines.push(String::new());
            lines.push("【近期状态】".to_string());
            if !rs.recent_goals.is_empty() {
                lines.push(format!("- 最近目标：{}", rs.recent_goals.join("、")));
            }
            if !rs.current_projects.is_empty() {
                lines.push(format!("- 当前项目：{}", rs.current_projects.join("、")));
            }
            if !rs.recent_preferences.is_empty() {
                lines.push(format!("- 近期偏好：{}", rs.recent_preferences.join("、")));
            }
        }

        // L2 长期事实：自由事实
        if !inner.custom_facts.is_empty() {
            lines.push(String::new());
            lines.push("【关于用户的其他事实】".to_string());
            for fact in inner.custom_facts.iter().take(10) {
                lines.push(match &fact.backstory {
                    Some(b) if !b.trim().is_empty() => {
                        format!("- {}（来源：{}）", fact.content, b)
                    }
                    _ => format!("- {}", fact.content),
                });
            }
        }

        lines.join("\n")
    }

    /// 清空所有事实
    pub fn clear(&self) -> VivianResult<()> {
        let mut inner = self.inner.write();
        inner.basic_data.clear();
        inner.recent_state = L1RecentState::default();
        inner.custom_facts.clear();
        inner.save()
    }
}

impl UserFactStoreInner {
    fn load(&mut self) -> VivianResult<()> {
        if !self.store_path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&self.store_path).map_err(|e| {
            VivianError::Other(format!("读取用户事实文件失败: {e}"))
        })?;
        if content.trim().is_empty() {
            return Ok(());
        }

        #[derive(Deserialize)]
        struct StoreFile {
            #[serde(default)]
            basic_data: Vec<UserFact>,
            #[serde(default)]
            recent_state: L1RecentState,
            #[serde(default)]
            custom_facts: Vec<UserFact>,
        }

        let data: StoreFile = serde_json::from_str(&content).map_err(|e| {
            VivianError::Other(format!("解析用户事实文件失败: {e}"))
        })?;

        self.basic_data.clear();
        for fact in data.basic_data {
            if fact.fact_type.is_basic() {
                self.basic_data.insert(fact.fact_type, fact);
            }
        }
        self.recent_state = data.recent_state;
        self.custom_facts = data.custom_facts;
        Ok(())
    }

    fn save(&self) -> VivianResult<()> {
        let basic: Vec<&UserFact> = self.basic_data.values().collect();
        let data = serde_json::json!({
            "basic_data": basic,
            "recent_state": &self.recent_state,
            "custom_facts": &self.custom_facts,
        });

        let tmp = self.store_path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&data).unwrap_or_default())
            .map_err(|e| VivianError::Other(format!("写入用户事实文件失败: {e}")))?;
        std::fs::rename(&tmp, &self.store_path)
            .map_err(|e| VivianError::Other(format!("重命名用户事实文件失败: {e}")))?;
        Ok(())
    }
}

fn current_timestamp() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn build_extract_prompt(
    user_input: &str,
    ai_response: &str,
    existing_facts: &str,
) -> String {
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
    // 已知事实段落：为空时不输出，避免 prompt 噪声
    let existing_section = |header: &str| -> String {
        if existing_facts.trim().is_empty() {
            String::new()
        } else {
            format!("\n{header}\n{existing_facts}\n\n")
        }
    };
    match lang_norm {
        "en" => format!(
            "You are a user information extractor. Extract explicit factual information about the user from the following conversation.\n\n\
            [User says to me] {user_input}\n\
            [I say to User] {ai_response}\n\n\
            {existing_section}\
            ## Extraction Rules\n\
            1. Only extract facts the user **explicitly stated**; do not extract the AI's words\n\
            2. Only extract facts that hold long-term; do not extract temporary emotions or one-time needs\n\
            3. Prefer missing over wrong; when no clear facts exist, set detected=false\n\
            4. Do not re-extract facts already listed in \"Known Facts\" below\n\n\
            ## Tense Isolation Constraint (Important)\n\
            Only extract the user's **current stable state**; strictly distinguish tenses:\n\
            - Past experience != current state: \"have been to Beijing\" or \"was on a business trip in Shanghai last week\" should not be extracted as location\n\
            - Future wish != current state: \"want to go to Japan\" or \"moving to Shenzhen next month\" should not be extracted as location\n\
            - Future wish != current occupation: \"dream of becoming a doctor\" or \"want to switch to design\" should not be extracted as occupation\n\
            - Past experience != current occupation: \"used to work in a bank\" or \"was in sales last year\" should not be extracted as occupation\n\
            - Historical age != current age: \"I was 25 last year\" should not be directly extracted as age (unless the user explicitly states current age)\n\
            - Temporary identity != long-term identity: \"staying at a friend's place this week\" should not be extracted as location\n\
            Only facts stated by the user in the present tense as stable facts can be extracted (e.g., \"I live in Beijing\", \"I am a programmer\", \"My name is Xiaoming\")\n\n\
            ## Field Types\n\
            - name: user's name/nickname\n\
            - age: age\n\
            - gender: gender\n\
            - occupation: occupation/job/school\n\
            - location: location/city\n\
            - birthday: birthday (e.g., \"1995-08-15\")\n\
            - sleep_schedule: sleep/wake routine (e.g., \"usually sleeps at 23:00, wakes at 7:00\")\n\
            - favorite_website: frequently visited websites (e.g., \"Bilibili for anime\", \"GitHub\")\n\
            - favorite_game: games they play (e.g., \"Genshin Impact\", \"Zelda\")\n\
            - hobby: long-term interests (e.g., \"photography\", \"piano\")\n\
            - custom: other long-term facts (e.g., \"owns a cat\", \"preparing for grad school entrance exam\")\n\n\
            ## Confidence\n\
            - 0.9+: user explicitly self-introduces (\"My name is X\")\n\
            - 0.7-0.8: inferred from conversation but clear (\"I work at Y company\")\n\
            - 0.5-0.6: implicit but inferrable\n\n\
            Output only JSON, no markdown code blocks:\n\
            {{\"detected\":true,\"items\":[{{\"type\":\"name\",\"content\":\"Xiaoming\",\"confidence\":0.95,\"reasoning\":\"user self-introduction\"}}]}}",
            existing_section = existing_section("## Known Facts (avoid duplicate extraction)")
        ),
        "ja" => format!(
            "あなたはユーザー情報抽出器です。以下の会話からユーザーの明示的な事実情報を抽出してください。\n\n\
            [User says to me] {user_input}\n\
            [I say to User] {ai_response}\n\n\
            {existing_section}\
            ## 抽出ルール\n\
            1. ユーザーが**明示的に述べた**事実のみを抽出し、AI の発言は抽出しない\n\
            2. 長期的に成り立つ事実のみを抽出し、一時的な感情や一回限りの要望は抽出しない\n\
            3. 割り切って不足させるほうがマシ、明確な事実がない場合は detected=false\n\
            4. 以下の「既知の事実」に既に含まれる事実を再抽出しない\n\n\
            ## 時制隔離制約（重要）\n\
            ユーザーの**現在の安定した状態**のみを抽出し、時制を厳格に区別すること：\n\
            - 過去の経験 != 現在の状態：「北京に行ったことがある」「先週上海へ出張した」は location として抽出しない\n\
            - 未来の願い != 現在の状態：「日本に行きたい」「来月深圳に引っ越す」は location として抽出しない\n\
            - 未来の願い != 現在の職業：「医者になりたい」「デザインに転職したい」は occupation として抽出しない\n\
            - 過去の経験 != 現在の職業：「以前銀行で働いていた」「去年は営業をしていた」は occupation として抽出しない\n\
            - 過去の年齢 != 現在の年齢：「去年私は25歳だった」は age として直接抽出しない（ユーザーが現在の年齢を明示した場合を除く）\n\
            - 一時的な身分 != 長期的な身分：「今週は友達の家に泊まっている」は location として抽出しない\n\
            ユーザーが現在時制で述べた安定した事実のみ抽出可能（例：「私は北京に住んでいる」「私はプログラマーです」「私は小明と言います」）\n\n\
            ## フィールドタイプ\n\
            - name：ユーザーの名前/ニックネーム\n\
            - age：年齢\n\
            - gender：性別\n\
            - occupation：職業/仕事/学校\n\
            - location：所在地/都市\n\
            - birthday：誕生日（例：「1995-08-15」）\n\
            - sleep_schedule：睡眠/起床の習慣（例：「通常23時に寝て7時に起きる」）\n\
            - favorite_website：よく使うウェブサイト（例：「Bilibiliでアニメを見る」「GitHub」）\n\
            - favorite_game：好きなゲーム（例：「原神」「ゼルダ」）\n\
            - hobby：長期的な趣味（例：「写真」「ピアノ」）\n\
            - custom：その他の長期的な事実（例：「猫を飼っている」「大学院入試を準備している」）\n\n\
            ## 信頼度\n\
            - 0.9+：ユーザーが明確に自己紹介（「私の名前はXです」）\n\
            - 0.7-0.8：会話から推測されるが明確（「Y社に勤めています」）\n\
            - 0.5-0.6：暗黙だが推測可能\n\n\
            JSON のみを出力し、markdown コードブロックは使用しない：\n\
            {{\"detected\":true,\"items\":[{{\"type\":\"name\",\"content\":\"小明\",\"confidence\":0.95,\"reasoning\":\"ユーザーの自己紹介\"}}]}}",
            existing_section = existing_section("## 既知の事実（重複抽出を避ける）")
        ),
        _ => format!(
            "你是用户信息提取器。从下面的对话中提取用户的显式事实信息。\n\n\
            [User says to me] {user_input}\n\
            [I say to User] {ai_response}\n\n\
            {existing_section}\
            ## 提取规则\n\
            1. 只提取用户**明确陈述**的事实，不提取 AI 的话\n\
            2. 只提取能长期成立的事实，不提取临时情绪或一次性需求\n\
            3. 宁缺毋滥，没有明确事实时 detected=false\n\
            4. 不要重复抽取下方「已知事实」中已列出的事实\n\n\
            ## 时态隔离约束（重要）\n\
            仅提取用户**当前稳定的状态**，必须严格区分时态：\n\
            - 过去经历≠当前状态：「去过北京」「上周在上海出差」不应提取为 location\n\
            - 未来愿望≠当前状态：「想去日本」「下个月搬去深圳」不应提取为 location\n\
            - 未来愿望≠当前职业：「梦想成为医生」「想转行做设计」不应提取为 occupation\n\
            - 过去经历≠当前职业：「曾经在银行工作」「去年在做销售」不应提取为 occupation\n\
            - 历史年龄≠当前年龄：「去年我25岁」不应直接提取为 age（除非用户明确说明当前年龄）\n\
            - 临时身份≠长期身份：「这周在朋友家借宿」不应提取为 location\n\
            只有用户以现在时陈述的稳定事实才能提取（如「我住在北京」「我是程序员」「我叫小明」）\n\n\
            ## 字段类型\n\
            - name：用户姓名/昵称\n\
            - age：年龄\n\
            - gender：性别\n\
            - occupation：职业/工作/学校\n\
            - location：所在地/城市\n\
            - birthday：生日（如\"1995-08-15\"）\n\
            - sleep_schedule：作息习惯（如\"通常 23 点睡 7 点起\"）\n\
            - favorite_website：常用网站（如\"B站看番\"、\"GitHub\"）\n\
            - favorite_game：喜欢的游戏（如\"原神\"、\"塞尔达\"）\n\
            - hobby：长期兴趣爱好（如\"摄影\"、\"钢琴\"）\n\
            - custom：其他长期事实（如\"养了一只猫\"、\"在准备考研\"）\n\n\
            ## 置信度\n\
            - 0.9+：用户明确自我介绍（\"我叫X\"）\n\
            - 0.7-0.8：从对话推断但明确（\"我在Y公司上班\"）\n\
            - 0.5-0.6：隐含但可推断\n\n\
            只输出 JSON，不要 markdown 代码块：\n\
            {{\"detected\":true,\"items\":[{{\"type\":\"name\",\"content\":\"小明\",\"confidence\":0.95,\"reasoning\":\"用户自我介绍\"}}]}}",
            existing_section = existing_section("## 已知事实（避免重复抽取）")
        ),
    }
}

fn parse_extract_response(resp: &str) -> VivianResult<ExtractedFacts> {
    let cleaned = strip_code_fence(resp);
    serde_json::from_str(cleaned)
        .map_err(|e| VivianError::Other(format!("解析用户事实提取响应失败: {e}")))
}

fn strip_code_fence(s: &str) -> &str {
    let s = s.trim();
    if s.starts_with("```") {
        let after = s.trim_start_matches("```json").trim_start_matches("```").trim();
        if let Some(end) = after.rfind("```") {
            return after[..end].trim();
        }
        return after;
    }
    s
}

/// 从用户本轮输入截取一段作为事实的来源叙事背景（backstory）。
///
/// 用于消歧：同一条事实在不同语境下含义可能不同，保留"为什么"可帮助后续正确解读。
fn backstory_snippet(user_input: &str) -> String {
    const MAX_CHARS: usize = 120;
    let trimmed = user_input.trim();
    if trimmed.chars().count() <= MAX_CHARS {
        trimmed.to_string()
    } else {
        format!("{}…", trimmed.chars().take(MAX_CHARS).collect::<String>())
    }
}
