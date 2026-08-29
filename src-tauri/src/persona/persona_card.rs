//! 人格卡演化系统 — 表达侧面的静默演进
//!
//!
//! - **Core Persona 不可覆盖**：`IdentityLayer`（姓名/角色/核心原则/禁忌）始终锁定，
//!   卡片只能覆盖第二层（expression / language_style / style_preset）和追加指令，
//!   不会污染角色身份本质。
//! - **Card 是表达侧面**：一张卡片代表 Vivian 在特定情境下的表达风格
//!   （如「深夜陪伴模式」「代码搭档模式」），可激活/归档/删除。
//! - **冷却机制**：防止人格频繁翻转
//!   - 切换冷却：3 轮（刚切完不能立刻再切）
//!   - 创建冷却：20 轮（避免短时间内大量新卡片）
//!   - 更新冷却：5 轮（避免同一卡片被频繁修改）
//! - **persona_events 审计日志**：所有卡片操作记录到事件日志，可追溯演化历程。
//!
//! 持久化：
//! - 卡片数据：`%APPDATA%\Vivian\persona\cards.json`
//! - 事件日志：`%APPDATA%\Vivian\persona\persona_events.jsonl`

use std::collections::HashMap;
use std::path::PathBuf;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};

use super::schemas::{CharacterExpression, LanguageStyle};

/// 切换冷却（轮次）
const SWITCH_COOLDOWN_TURNS: u32 = 3;
/// 创建冷却（轮次）
const CREATE_COOLDOWN_TURNS: u32 = 20;
/// 更新冷却（轮次）
const UPDATE_COOLDOWN_TURNS: u32 = 5;
/// 最大活跃卡片数（防止卡片膨胀）
const MAX_ACTIVE_CARDS: usize = 10;

/// 卡片状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CardStatus {
    /// 活跃（可激活）
    Active,
    /// 已归档（保留历史但不参与选择）
    Archived,
}

/// 人格卡片 — 表达侧面
///
/// 覆盖物（overlays）均为 Optional：None 表示沿用 Core Persona 默认值。
/// 永远不覆盖 `IdentityLayer`（姓名/角色/核心原则/禁忌）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaCard {
    /// 唯一 ID（uuid v4）
    pub id: String,
    /// 卡片名称（如「深夜陪伴」「代码搭档」）
    pub name: String,
    /// 卡片描述（何时触发、什么风格）
    pub description: String,
    /// 表达参数覆盖（8 维）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression_override: Option<CharacterExpression>,
    /// 语言风格覆盖
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_style_override: Option<LanguageStyle>,
    /// 风格预设覆盖（default / lively / healing / focused / sweet）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_preset: Option<String>,
    /// 额外指令（注入 prompt 的场景化指令）
    #[serde(default)]
    pub extra_instructions: Vec<String>,
    /// 状态
    pub status: CardStatus,
    /// 创建时间戳（秒）
    pub created_at: f64,
    /// 最后更新时间戳（秒）
    pub updated_at: f64,
    /// 最后激活时间戳（秒）
    pub last_activated_at: f64,
}

/// 卡片操作事件（审计日志）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PersonaEvent {
    /// 创建卡片
    Create {
        card_id: String,
        card_name: String,
        at: f64,
        turn: u32,
    },
    /// 更新卡片
    Update {
        card_id: String,
        card_name: String,
        at: f64,
        turn: u32,
    },
    /// 切换激活卡片
    Switch {
        from_card_id: Option<String>,
        to_card_id: String,
        to_card_name: String,
        at: f64,
        turn: u32,
    },
    /// 归档卡片
    Archive {
        card_id: String,
        card_name: String,
        at: f64,
        turn: u32,
    },
    /// 删除卡片
    Delete {
        card_id: String,
        card_name: String,
        at: f64,
        turn: u32,
    },
    /// 取消激活（回到 Core Persona）
    Deactivate {
        from_card_id: String,
        at: f64,
        turn: u32,
    },
}

/// 持久化数据
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CardStoreData {
    cards: Vec<PersonaCard>,
    active_card_id: Option<String>,
    /// 对话轮次计数器（每轮 +1，用于冷却判断）
    turn_counter: u32,
    /// 上次切换轮次
    last_switch_turn: u32,
    /// 上次创建轮次
    last_create_turn: u32,
    /// 上次更新轮次（按卡片 ID 索引）
    last_update_turns: HashMap<String, u32>,
}

/// 人格卡片存储 — 管理 CRUD + 冷却 + 激活
pub struct PersonaCardStore {
    inner: RwLock<CardStoreData>,
    persistence_path: PathBuf,
    events_path: PathBuf,
}

impl PersonaCardStore {
    pub fn new(char_id: &str) -> VivianResult<Self> {
        let dir = crate::utils::path::get_character_data_dir(char_id).join("persona");
        std::fs::create_dir_all(&dir)
            .map_err(|e| VivianError::Memory(format!("创建人格卡片目录失败: {e}")))?;
        let persistence_path = dir.join("cards.json");
        let events_path = dir.join("persona_events.jsonl");

        let data = if persistence_path.exists() {
            Self::load_from(&persistence_path)
        } else {
            CardStoreData::default()
        };

        Ok(Self {
            inner: RwLock::new(data),
            persistence_path,
            events_path,
        })
    }

    /// 降级构造：不持久化
    pub fn fallback() -> Self {
        Self {
            inner: RwLock::new(CardStoreData::default()),
            persistence_path: PathBuf::new(),
            events_path: PathBuf::new(),
        }
    }

    fn load_from(path: &std::path::Path) -> CardStoreData {
        match std::fs::read_to_string(path) {
            Ok(content) if !content.trim().is_empty() => {
                serde_json::from_str::<CardStoreData>(&content).unwrap_or_default()
            }
            _ => CardStoreData::default(),
        }
    }

    fn save_to(&self) -> VivianResult<()> {
        if self.persistence_path.as_os_str().is_empty() {
            return Ok(());
        }
        let data = self.inner.read().clone();
        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| VivianError::Memory(format!("序列化人格卡片失败: {e}")))?;
        let tmp = self.persistence_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)
            .map_err(|e| VivianError::Memory(format!("写入人格卡片临时文件失败: {e}")))?;
        std::fs::rename(&tmp, &self.persistence_path)
            .map_err(|e| VivianError::Memory(format!("替换人格卡片文件失败: {e}")))?;
        Ok(())
    }

    fn append_event(&self, event: &PersonaEvent) {
        if self.events_path.as_os_str().is_empty() {
            return;
        }
        let line = match serde_json::to_string(event) {
            Ok(s) => s,
            Err(_) => return,
        };
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.events_path)
        {
            let _ = writeln!(f, "{line}");
        }
    }

    fn now() -> f64 {
        chrono::Local::now().timestamp() as f64
    }

    // ===== 轮次计数 =====

    /// 递增对话轮次（每轮对话结束时调用）
    pub fn tick_turn(&self) {
        let mut data = self.inner.write();
        data.turn_counter = data.turn_counter.saturating_add(1);
        let turn = data.turn_counter;
        drop(data);
        // 轮次递增不需要每次都落盘（频繁 IO），但为简单起见仍持久化
        let _ = self.save_to();
        tracing::trace!("[PersonaCard] turn -> {turn}");
    }

    pub fn current_turn(&self) -> u32 {
        self.inner.read().turn_counter
    }

    // ===== CRUD =====

    /// 创建新卡片
    ///
    /// 冷却：距上次创建 ≥ `CREATE_COOLDOWN_TURNS` 轮
    pub fn create_card(&self, name: &str, description: &str) -> VivianResult<PersonaCard> {
        let mut data = self.inner.write();
        let turn = data.turn_counter;
        let elapsed = turn.saturating_sub(data.last_create_turn);
        if elapsed < CREATE_COOLDOWN_TURNS {
            return Err(VivianError::Memory(format!(
                "创建冷却中：还需 {} 轮才可创建新卡片",
                CREATE_COOLDOWN_TURNS - elapsed
            )));
        }

        let active_count = data.cards.iter().filter(|c| c.status == CardStatus::Active).count();
        if active_count >= MAX_ACTIVE_CARDS {
            return Err(VivianError::Memory(format!(
                "活跃卡片数已达上限 {MAX_ACTIVE_CARDS}，请先归档或删除旧卡片"
            )));
        }

        let now = Self::now();
        let card = PersonaCard {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            description: description.to_string(),
            expression_override: None,
            language_style_override: None,
            style_preset: None,
            extra_instructions: Vec::new(),
            status: CardStatus::Active,
            created_at: now,
            updated_at: now,
            last_activated_at: 0.0,
        };

        data.cards.push(card.clone());
        data.last_create_turn = turn;
        drop(data);

        self.append_event(&PersonaEvent::Create {
            card_id: card.id.clone(),
            card_name: card.name.clone(),
            at: now,
            turn,
        });
        self.save_to()?;
        tracing::info!("[PersonaCard] 创建卡片: {} ({})", card.name, card.id);
        Ok(card)
    }

    /// 更新卡片覆盖物
    ///
    /// 冷却：距上次更新该卡片 ≥ `UPDATE_COOLDOWN_TURNS` 轮
    pub fn update_card(
        &self,
        card_id: &str,
        expression_override: Option<CharacterExpression>,
        language_style_override: Option<LanguageStyle>,
        style_preset: Option<String>,
        extra_instructions: Option<Vec<String>>,
        description: Option<String>,
    ) -> VivianResult<()> {
        let mut data = self.inner.write();
        let turn = data.turn_counter;
        let last_update = data.last_update_turns.get(card_id).copied().unwrap_or(0);
        let elapsed = turn.saturating_sub(last_update);
        if elapsed < UPDATE_COOLDOWN_TURNS {
            return Err(VivianError::Memory(format!(
                "更新冷却中：还需 {} 轮才可更新此卡片",
                UPDATE_COOLDOWN_TURNS - elapsed
            )));
        }

        let card = data
            .cards
            .iter_mut()
            .find(|c| c.id == card_id)
            .ok_or_else(|| VivianError::Memory(format!("卡片不存在: {card_id}")))?;

        if expression_override.is_some() {
            card.expression_override = expression_override;
        }
        if language_style_override.is_some() {
            card.language_style_override = language_style_override;
        }
        if style_preset.is_some() {
            card.style_preset = style_preset;
        }
        if let Some(instructions) = extra_instructions {
            card.extra_instructions = instructions;
        }
        if let Some(desc) = description {
            card.description = desc;
        }
        card.updated_at = Self::now();
        let event_card = card.clone();
        data.last_update_turns.insert(card_id.to_string(), turn);
        drop(data);

        let event_name = event_card.name.clone();
        self.append_event(&PersonaEvent::Update {
            card_id: event_card.id,
            card_name: event_card.name,
            at: event_card.updated_at,
            turn,
        });
        self.save_to()?;
        tracing::info!("[PersonaCard] 更新卡片: {}", event_name);
        Ok(())
    }

    /// 切换激活卡片
    ///
    /// 冷却：距上次切换 ≥ `SWITCH_COOLDOWN_TURNS` 轮
    /// 传入 None 则取消激活（回到 Core Persona）
    pub fn switch_card(&self, card_id: Option<&str>) -> VivianResult<()> {
        let mut data = self.inner.write();
        let turn = data.turn_counter;
        let elapsed = turn.saturating_sub(data.last_switch_turn);
        if elapsed < SWITCH_COOLDOWN_TURNS {
            return Err(VivianError::Memory(format!(
                "切换冷却中：还需 {} 轮才可切换",
                SWITCH_COOLDOWN_TURNS - elapsed
            )));
        }

        let now = Self::now();
        let prev_active = data.active_card_id.clone();

        match card_id {
            None => {
                // 取消激活
                if let Some(prev) = &prev_active {
                    let event = PersonaEvent::Deactivate {
                        from_card_id: prev.clone(),
                        at: now,
                        turn,
                    };
                    data.active_card_id = None;
                    data.last_switch_turn = turn;
                    drop(data);
                    self.append_event(&event);
                    self.save_to()?;
                    tracing::info!("[PersonaCard] 取消激活，回到 Core Persona");
                }
                return Ok(());
            }
            Some(id) => {
                let (to_card_id, to_card_name) = {
                    let card = data
                        .cards
                        .iter_mut()
                        .find(|c| c.id == id && c.status == CardStatus::Active)
                        .ok_or_else(|| VivianError::Memory(format!("活跃卡片不存在: {id}")))?;
                    card.last_activated_at = now;
                    (card.id.clone(), card.name.clone())
                };
                let event = PersonaEvent::Switch {
                    from_card_id: prev_active.clone(),
                    to_card_id: to_card_id.clone(),
                    to_card_name: to_card_name.clone(),
                    at: now,
                    turn,
                };
                data.active_card_id = Some(id.to_string());
                data.last_switch_turn = turn;
                drop(data);
                self.append_event(&event);
                self.save_to()?;
                tracing::info!("[PersonaCard] 切换激活: {}", to_card_name);
            }
        }
        Ok(())
    }

    /// 归档卡片（保留历史，不参与选择）
    pub fn archive_card(&self, card_id: &str) -> VivianResult<()> {
        let mut data = self.inner.write();
        let card = data
            .cards
            .iter_mut()
            .find(|c| c.id == card_id)
            .ok_or_else(|| VivianError::Memory(format!("卡片不存在: {card_id}")))?;
        card.status = CardStatus::Archived;
        let card_name = card.name.clone();
        let now = Self::now();
        let turn = data.turn_counter;
        // 如果归档的是当前激活卡片，取消激活
        if data.active_card_id.as_deref() == Some(card_id) {
            data.active_card_id = None;
        }
        drop(data);
        self.append_event(&PersonaEvent::Archive {
            card_id: card_id.to_string(),
            card_name,
            at: now,
            turn,
        });
        self.save_to()?;
        tracing::info!("[PersonaCard] 归档卡片: {card_id}");
        Ok(())
    }

    /// 删除卡片
    pub fn delete_card(&self, card_id: &str) -> VivianResult<()> {
        let mut data = self.inner.write();
        let pos = data
            .cards
            .iter()
            .position(|c| c.id == card_id)
            .ok_or_else(|| VivianError::Memory(format!("卡片不存在: {card_id}")))?;
        let card = data.cards.remove(pos);
        let card_name = card.name.clone();
        let now = Self::now();
        let turn = data.turn_counter;
        if data.active_card_id.as_deref() == Some(card_id) {
            data.active_card_id = None;
        }
        data.last_update_turns.remove(card_id);
        drop(data);
        self.append_event(&PersonaEvent::Delete {
            card_id: card_id.to_string(),
            card_name,
            at: now,
            turn,
        });
        self.save_to()?;
        tracing::info!("[PersonaCard] 删除卡片: {card_id}");
        Ok(())
    }

    // ===== 读取 =====

    /// 获取当前激活的卡片（如果有）
    pub fn get_active_card(&self) -> Option<PersonaCard> {
        let data = self.inner.read();
        let active_id = data.active_card_id.as_ref()?;
        data.cards
            .iter()
            .find(|c| c.id == *active_id && c.status == CardStatus::Active)
            .cloned()
    }

    /// 列出所有卡片（可选过滤活跃/归档）
    pub fn list_cards(&self, include_archived: bool) -> Vec<PersonaCard> {
        let data = self.inner.read();
        data.cards
            .iter()
            .filter(|c| include_archived || c.status == CardStatus::Active)
            .cloned()
            .collect()
    }

    /// 获取指定卡片
    pub fn get_card(&self, card_id: &str) -> Option<PersonaCard> {
        self.inner
            .read()
            .cards
            .iter()
            .find(|c| c.id == card_id)
            .cloned()
    }

    /// 读取事件日志（最新的 N 条）
    pub fn read_events(&self, limit: usize) -> Vec<PersonaEvent> {
        if self.events_path.as_os_str().is_empty() {
            return Vec::new();
        }
        let content = match std::fs::read_to_string(&self.events_path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let mut events: Vec<PersonaEvent> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        events.reverse(); // 最新在前
        events.truncate(limit);
        events
    }

    // ===== 冷却查询 =====

    pub fn turns_until_can_switch(&self) -> u32 {
        let data = self.inner.read();
        let elapsed = data.turn_counter.saturating_sub(data.last_switch_turn);
        SWITCH_COOLDOWN_TURNS.saturating_sub(elapsed)
    }

    pub fn turns_until_can_create(&self) -> u32 {
        let data = self.inner.read();
        let elapsed = data.turn_counter.saturating_sub(data.last_create_turn);
        CREATE_COOLDOWN_TURNS.saturating_sub(elapsed)
    }

    pub fn turns_until_can_update(&self, card_id: &str) -> u32 {
        let data = self.inner.read();
        let last = data.last_update_turns.get(card_id).copied().unwrap_or(0);
        let elapsed = data.turn_counter.saturating_sub(last);
        UPDATE_COOLDOWN_TURNS.saturating_sub(elapsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_switch() {
        let store = PersonaCardStore::fallback();
        let card = store.create_card("测试卡", "用于测试").unwrap();
        assert_eq!(store.list_cards(false).len(), 1);
        store.switch_card(Some(&card.id)).unwrap();
        assert_eq!(store.get_active_card().unwrap().id, card.id);
    }

    #[test]
    fn switch_cooldown_blocks_rapid_switch() {
        let store = PersonaCardStore::fallback();
        let c1 = store.create_card("卡1", "").unwrap();
        // 创建消耗一次 create cooldown，但 switch cooldown 从 0 开始
        store.switch_card(Some(&c1.id)).unwrap();
        // 立刻再切应该被冷却挡住
        assert!(store.switch_card(None).is_err());
        // 推进轮次
        for _ in 0..SWITCH_COOLDOWN_TURNS {
            store.tick_turn();
        }
        store.switch_card(None).unwrap();
        assert!(store.get_active_card().is_none());
    }

    #[test]
    fn create_cooldown_blocks_rapid_create() {
        let store = PersonaCardStore::fallback();
        store.create_card("卡1", "").unwrap();
        // 立刻再创建应该被冷却挡住
        assert!(store.create_card("卡2", "").is_err());
        // 推进轮次
        for _ in 0..CREATE_COOLDOWN_TURNS {
            store.tick_turn();
        }
        store.create_card("卡2", "").unwrap();
    }

    #[test]
    fn archive_removes_from_active_selection() {
        let store = PersonaCardStore::fallback();
        let card = store.create_card("测试", "").unwrap();
        store.switch_card(Some(&card.id)).unwrap();
        assert!(store.get_active_card().is_some());
        store.archive_card(&card.id).unwrap();
        assert!(store.get_active_card().is_none());
        // 归档卡片不在活跃列表中
        assert!(store.list_cards(false).is_empty());
        // 但在包含归档的列表中
        assert_eq!(store.list_cards(true).len(), 1);
    }

    #[test]
    fn delete_removes_card() {
        let store = PersonaCardStore::fallback();
        let card = store.create_card("待删", "").unwrap();
        store.delete_card(&card.id).unwrap();
        assert!(store.list_cards(true).is_empty());
    }

    #[test]
    fn update_cooldown_blocks_rapid_update() {
        let store = PersonaCardStore::fallback();
        let card = store.create_card("测试", "").unwrap();
        let expr = CharacterExpression::default();
        // 第一次更新应该成功（last_update_turn 初始为 0，elapsed 远超冷却）
        for _ in 0..UPDATE_COOLDOWN_TURNS {
            store.tick_turn();
        }
        store
            .update_card(&card.id, Some(expr.clone()), None, None, None, None)
            .unwrap();
        // 立刻再更新应该被冷却挡住
        assert!(store
            .update_card(&card.id, Some(expr), None, None, None, None)
            .is_err());
    }
}
