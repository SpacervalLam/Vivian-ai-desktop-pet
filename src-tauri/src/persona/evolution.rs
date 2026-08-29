//! 自我进化人设覆盖层（Persona Evolution）
//!
//! 让智能体在反思中自行优化自己的语气/性格，但**不修改原始人设文件**。
//! 只影响最终拼入 prompt 的内容：将成长记录追加到 Character 块末尾，让 LLM
//! 感知"我最近对自己做了什么调整"，从而表现出持续成长、更栩栩如生。
//!
//! 设计要点：
//! - 独立存储于 `characters/<char_id>/persona/evolution.json`，与出厂人设相分离
//! - 恢复出厂：清空覆盖层即可，原始 persona.json / prompts/ 文件永不被触碰
//! - 两次调整间有最小间隔，避免每轮对话都改（成长是渐进的，不是每话说变脸）
//! - 去重 + 条数上限，防止覆盖层无限膨胀

use std::path::PathBuf;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};
use crate::utils::path;

/// 两次自我调整的最小间隔（秒）——默认 6 小时，避免每轮对话都改
const EVOLUTION_MIN_INTERVAL_SECS: f64 = 6.0 * 3600.0;
/// 覆盖层总条数上限
const MAX_TOTAL_ENTRIES: usize = 20;
/// 渲染进 prompt 时展示的最近条数
const RENDER_RECENT: usize = 6;
/// 晋升为正式调整所需的最小支持次数（跨轨迹验证门槛）
///
/// 书中 8.2.2 强调：一次偶发成功不应立即改变长期能力。同一调整必须在多次
/// 独立反思中被重复提出（即获得多条轨迹支持）后才真正生效，避免单次噪音
/// 被固化为长期人格改变。
const REQUIRED_SUPPORT: u32 = 2;
/// 候选调整条数上限（未达门槛的草稿，防止无限堆积）
const MAX_CANDIDATES: usize = 12;

/// 单条自我成长记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionEntry {
    /// 调整时间戳
    pub timestamp: f64,
    /// 类别：tone（语气）/ personality（性格）
    pub kind: String,
    /// 调整内容（第一人称行为指令，如"最近回复可以更活泼一点"）
    pub text: String,
    /// 调整原因（源自哪段对话/体会）
    pub reason: String,
    /// 晋升前累计的支持次数（多少条独立轨迹共同支撑该调整）
    #[serde(default = "default_support")]
    pub support: u32,
}

fn default_support() -> u32 {
    1
}

/// 待晋升的候选调整（尚未达到跨轨迹支持门槛）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionCandidate {
    /// 类别：tone / personality
    pub kind: String,
    /// 调整内容
    pub text: String,
    /// 最后一次被提出的原因
    pub reason: String,
    /// 首次被提出的时间戳
    pub first_seen: f64,
    /// 被独立反思提出的次数
    pub support: u32,
}

/// 自我进化覆盖层
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaEvolution {
    #[serde(default)]
    pub entries: Vec<EvolutionEntry>,
    /// 待晋升的候选（未达门槛，不注入 prompt）
    #[serde(default)]
    pub candidates: Vec<EvolutionCandidate>,
    /// 最后调整时间
    #[serde(default)]
    pub updated_at: f64,
}

impl PersonaEvolution {
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            candidates: Vec::new(),
            updated_at: 0.0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 候选调整条数
    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }

    /// 尝试记录一条成长调整（跨轨迹验证门槛）。
    ///
    /// 返回 `true` 表示该调整已晋升为正式调整并写入覆盖层；返回 `false` 表示
    /// 未生效（文本为空、已是正式调整、尚未达到支持门槛，或受最小间隔限制）。
    ///
    /// 门槛逻辑（书中 8.2.2）：同一调整先在候选区累积支持次数，只有被多次
    /// 独立反思重复提出（≥ `REQUIRED_SUPPORT` 次）后才晋升，防止单次噪音被
    /// 固化为长期人格改变。候选累积不受最小间隔限制，晋升才受其约束。
    pub fn try_add(&mut self, kind: &str, text: &str, reason: &str, now: f64) -> bool {
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        // 已是正式调整：不重复记录
        if self.entries.iter().any(|e| e.text == text) {
            return false;
        }
        // 已存在候选：累积支持次数
        if let Some(c) = self.candidates.iter_mut().find(|c| c.text == text) {
            c.support += 1;
            if !reason.trim().is_empty() {
                c.reason = reason.trim().to_string();
            }
            if c.support < REQUIRED_SUPPORT {
                return false;
            }
            // 达到门槛：晋升仍需受最小间隔约束（成长是渐进的）。首次晋升
            // （updated_at 尚未记录）不受间隔限制。
            let interval_ok =
                self.updated_at == 0.0 || now - self.updated_at >= EVOLUTION_MIN_INTERVAL_SECS;
            if !interval_ok {
                return false;
            }
            self.promote_candidate(text, kind, now);
            return true;
        }
        // 第一次被提出：作为候选记录
        self.candidates.push(EvolutionCandidate {
            kind: kind.to_string(),
            text: text.to_string(),
            reason: reason.trim().to_string(),
            first_seen: now,
            support: 1,
        });
        // 候选上限：淘汰支持度最低且最旧的
        if self.candidates.len() > MAX_CANDIDATES {
            self.candidates.sort_by(|a, b| {
                a.support
                    .cmp(&b.support)
                    .then(b.first_seen.partial_cmp(&a.first_seen).unwrap_or(std::cmp::Ordering::Equal))
            });
            self.candidates.truncate(MAX_CANDIDATES);
        }
        false
    }

    /// 将指定候选晋升为正式调整，并维护覆盖层条数上限。
    fn promote_candidate(&mut self, text: &str, kind: &str, now: f64) {
        let reason = self
            .candidates
            .iter()
            .find(|c| c.text == text)
            .map(|c| c.reason.clone())
            .unwrap_or_default();
        let support = self
            .candidates
            .iter()
            .find(|c| c.text == text)
            .map(|c| c.support)
            .unwrap_or(REQUIRED_SUPPORT);
        self.candidates.retain(|c| c.text != text);
        self.entries.push(EvolutionEntry {
            timestamp: now,
            kind: kind.to_string(),
            text: text.to_string(),
            reason,
            support,
        });
        // 条数上限：按"证据 + 时效"筛选保留（书中 8.3.5：按证据淘汰而非简单截断）。
        // 支持次数更高的调整（得到更多轨迹佐证）优先保留，其次保留较新的。
        if self.entries.len() > MAX_TOTAL_ENTRIES {
            self.entries.sort_by(|a, b| {
                b.support
                    .cmp(&a.support)
                    .then(b.timestamp.partial_cmp(&a.timestamp).unwrap_or(std::cmp::Ordering::Equal))
            });
            self.entries.truncate(MAX_TOTAL_ENTRIES);
            // 恢复时间正序，保持渲染稳定
            self.entries.sort_by(|a, b| a.timestamp.partial_cmp(&b.timestamp).unwrap_or(std::cmp::Ordering::Equal));
        }
        self.updated_at = now;
    }

    /// 渲染为注入 prompt 的"自我成长"文本。
    ///
    /// 只展示最近 `RENDER_RECENT` 条，避免覆盖层撑爆 Character 块。
    pub fn render(&self, lang: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let mut recent = self.entries.clone();
        recent.sort_by(|a, b| a.timestamp.partial_cmp(&b.timestamp).unwrap_or(std::cmp::Ordering::Equal));
        recent.reverse();
        recent.truncate(RENDER_RECENT);

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let (heading, tone_label, personality_label, closing) = match lang_norm {
            "en" => (
                "## Self-Growth (recent adjustments)",
                "tone",
                "personality",
                "These are adjustments you recently made to yourself. Keep them, but never lose your core persona.",
            ),
            "ja" => (
                "## 自己成長（最近の調整）",
                "話し方",
                "性格",
                "これらはあなたが最近自分に施した調整です。維持しつつ、核心の人格は失わないこと。",
            ),
            _ => (
                "## 自我成长（近期调整）",
                "语气",
                "性格",
                "这些是你最近对自己做出的调整，保持它们，但不要失去核心人设。",
            ),
        };

        let mut lines: Vec<String> = Vec::new();
        for e in &recent {
            let label = if e.kind == "personality" { personality_label } else { tone_label };
            let chr = chrono::DateTime::from_timestamp(e.timestamp as i64, 0)
                .map(|dt| dt.format("%m-%d").to_string())
                .unwrap_or_default();
            lines.push(format!("- [{}{}] {}：{}", label, chr, e.text, e.reason));
        }

        Some(format!("{}\n{}\n\n{}", heading, lines.join("\n"), closing))
    }
}

/// 自我进化覆盖层存储：加载/保存/重置
pub struct PersonaEvolutionStore {
    inner: RwLock<PersonaEvolution>,
    path: PathBuf,
}

impl PersonaEvolutionStore {
    /// 创建并加载覆盖层；文件不存在时返回空覆盖层
    pub fn new(char_id: &str) -> VivianResult<Self> {
        let dir = path::get_character_data_dir(char_id).join("persona");
        std::fs::create_dir_all(&dir)
            .map_err(|e| VivianError::Memory(format!("创建人格目录失败: {e}")))?;
        let store_path = dir.join("evolution.json");
        let evolution = crate::utils::fs::load_json_or_backup::<PersonaEvolution>(&store_path)
            .unwrap_or_else(PersonaEvolution::empty);
        Ok(Self {
            inner: RwLock::new(evolution),
            path: store_path,
        })
    }

    /// 降级为空实现（持久化失败时保证主流程不阻塞）
    pub fn fallback() -> Self {
        Self {
            inner: RwLock::new(PersonaEvolution::empty()),
            path: PathBuf::from("evolution.json"),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }

    pub fn last_update(&self) -> f64 {
        self.inner.read().updated_at
    }

    pub fn entries(&self) -> Vec<EvolutionEntry> {
        self.inner.read().entries.clone()
    }

    /// 待晋升的候选调整列表
    pub fn candidates(&self) -> Vec<EvolutionCandidate> {
        self.inner.read().candidates.clone()
    }

    /// 添加一条成长记录，返回是否成功记录
    pub fn add_entry(&self, kind: &str, text: &str, reason: &str) -> bool {
        let now = crate::memory::types::current_timestamp();
        {
            let mut ev = self.inner.write();
            if !ev.try_add(kind, text, reason, now) {
                return false;
            }
        }
        let _ = self.save_inner();
        true
    }

    /// 恢复出厂：清空覆盖层
    pub fn reset(&self) {
        *self.inner.write() = PersonaEvolution::empty();
        let _ = self.save_inner();
    }

    /// 渲染覆盖层为 prompt 文本
    pub fn render(&self, lang: &str) -> Option<String> {
        self.inner.read().render(lang)
    }

    fn save_inner(&self) -> VivianResult<()> {
        let ev = self.inner.read();
        let json = serde_json::to_string_pretty(&*ev)
            .map_err(|e| VivianError::Memory(format!("序列化进化覆盖层失败: {e}")))?;
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json)
            .map_err(|e| VivianError::Memory(format!("写入进化覆盖层失败: {e}")))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| VivianError::Memory(format!("替换进化覆盖层失败: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模拟"同一调整被多次独立反思提出"直到晋升，返回是否最终晋升。
    fn promote(ev: &mut PersonaEvolution, kind: &str, text: &str, reason: &str, now: f64) -> bool {
        let mut t = now;
        for _ in 0..(REQUIRED_SUPPORT + 2) {
            if ev.try_add(kind, text, reason, t) {
                return true;
            }
            t += 1.0;
        }
        false
    }

    #[test]
    fn test_first_proposal_stays_candidate() {
        let mut ev = PersonaEvolution::empty();
        // 第一次提出：未达门槛，仅作为候选
        assert!(!ev.try_add("tone", "更活泼一点", "最近回复有点机械", 1000.0));
        assert_eq!(ev.entries.len(), 0);
        assert_eq!(ev.candidate_count(), 1);
        assert!(ev.is_empty());
    }

    #[test]
    fn test_promote_after_support() {
        let mut ev = PersonaEvolution::empty();
        // 一次反射不改变人设
        assert!(!ev.try_add("tone", "更活泼一点", "原因", 1000.0));
        // 再次独立反思重复提出 → 晋升
        assert!(ev.try_add("tone", "更活泼一点", "原因", 1000.0 + EVOLUTION_MIN_INTERVAL_SECS + 1.0));
        assert_eq!(ev.entries.len(), 1);
        assert_eq!(ev.candidate_count(), 0);
        assert!(!ev.is_empty());
    }

    #[test]
    fn test_min_interval_blocks_second_promotion() {
        let mut ev = PersonaEvolution::empty();
        // 首次晋升不受间隔限制
        assert!(promote(&mut ev, "tone", "调整A", "原因", 1000.0));
        assert_eq!(ev.entries.len(), 1);
        // 第二次晋升：间隔不足，应被拦截
        assert!(!promote(&mut ev, "tone", "调整B", "原因", 1000.0 + 60.0));
        assert_eq!(ev.entries.len(), 1);
        // 间隔满足后晋升
        assert!(promote(&mut ev, "tone", "调整B", "原因", 1000.0 + EVOLUTION_MIN_INTERVAL_SECS + 1.0));
        assert_eq!(ev.entries.len(), 2);
    }

    #[test]
    fn test_empty_text_rejected() {
        let mut ev = PersonaEvolution::empty();
        assert!(!ev.try_add("tone", "   ", "原因", 1000.0));
        assert!(ev.is_empty());
        assert_eq!(ev.candidate_count(), 0);
    }

    #[test]
    fn test_duplicate_active_rejected() {
        let mut ev = PersonaEvolution::empty();
        assert!(promote(&mut ev, "tone", "更活泼一点", "原因", 1000.0));
        let later = 1000.0 + EVOLUTION_MIN_INTERVAL_SECS + 1.0;
        // 已是正式调整：不再重复
        assert!(!ev.try_add("tone", "更活泼一点", "原因", later));
        assert_eq!(ev.entries.len(), 1);
    }

    #[test]
    fn test_render_empty_returns_none() {
        let ev = PersonaEvolution::empty();
        assert!(ev.render("zh").is_none());
    }

    #[test]
    fn test_render_non_empty() {
        let mut ev = PersonaEvolution::empty();
        promote(&mut ev, "tone", "更活泼一点", "最近有点机械", 1000.0);
        let text = ev.render("zh").unwrap();
        assert!(text.contains("自我成长"));
        assert!(text.contains("更活泼一点"));
    }

    #[test]
    fn test_cap_total_entries() {
        let mut ev = PersonaEvolution::empty();
        let mut t = 1000.0;
        for i in 0..(MAX_TOTAL_ENTRIES + 10) {
            promote(&mut ev, "tone", &format!("调整{}", i), "原因", t);
            t += EVOLUTION_MIN_INTERVAL_SECS + 1.0;
        }
        assert!(ev.entries.len() <= MAX_TOTAL_ENTRIES);
    }

    #[test]
    fn test_cap_candidates() {
        let mut ev = PersonaEvolution::empty();
        let mut t = 1000.0;
        for i in 0..(MAX_CANDIDATES + 5) {
            // 每条只提出一次 → 全部停留在候选区
            ev.try_add("tone", &format!("候选{}", i), "原因", t);
            t += 1.0;
        }
        assert!(ev.candidate_count() <= MAX_CANDIDATES);
        assert!(ev.is_empty());
    }
}