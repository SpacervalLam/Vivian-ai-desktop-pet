//! 跨 work 会话共享的当前事实清单。
//!
//! 会话只是界面：同一件工作的关键前提（在做哪个项目、用哪个目录、定过哪些
//! 约定）不该因为换了会话就重新问一遍。这里只存**一句话事实**，条目少、
//! 可编辑、可追溯版本；不存推理过程，也不存聊天原文。
//!
//! 写入是受控的：模型提交的条目先过一道判断（是否别的会话也需要知道），
//! 用户手改绕过这道判断直接生效——判断只用来挡噪音，不用来限制用户。
//!
//! 作用域仅限 work 侧。陪伴侧角色上下文里不注入本文件。

use std::collections::HashMap;
use std::path::PathBuf;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// 条目上限：清单要能被一眼读完，超过就该合并或删掉过时项。
pub const MAX_FACTS: usize = 40;
/// 单条事实字符上限：一句话说清，不写段落。
pub const MAX_FACT_CHARS: usize = 240;
/// 来源会话在卡片上只显示一小段，避免把长 id 写进提示词。
const MAX_SESSION_LABEL: usize = 64;

/// 写入者。用户编辑优先于模型判断，展示与追溯都靠它。
pub const WRITER_MODEL: &str = "model";
pub const WRITER_USER: &str = "user";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkFact {
    pub id: String,
    pub text: String,
    /// 首次写入该事实的会话；用户手写时为空。
    #[serde(default)]
    pub source_session: String,
    #[serde(default = "default_writer")]
    pub updated_by: String,
    /// 每次内容变更 +1，用于「提醒是否过时」与冲突判断。
    #[serde(default)]
    pub revision: u32,
    #[serde(default)]
    pub updated_at: i64,
}

fn default_writer() -> String {
    WRITER_MODEL.to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FactsSnapshot {
    #[serde(default)]
    revision: u32,
    #[serde(default)]
    next_id: u32,
    #[serde(default)]
    facts: Vec<WorkFact>,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// 归一化：折叠空白、去首尾、按字符截断。返回 None 表示整条无效。
fn normalize(text: &str) -> Option<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    Some(collapsed.chars().take(MAX_FACT_CHARS).collect())
}

fn same_text(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

/// 事实清单的持久化存储。`path` 为 None 时只在内存里（测试与隔离运行）。
pub struct WorkFactsStore {
    path: Option<PathBuf>,
    snapshot: RwLock<FactsSnapshot>,
}

impl WorkFactsStore {
    pub fn new() -> Self {
        Self {
            path: None,
            snapshot: RwLock::new(FactsSnapshot::default()),
        }
    }

    pub fn persistent(path: PathBuf) -> Self {
        let loaded: FactsSnapshot = crate::utils::fs::load_json_or_backup(&path).unwrap_or_default();
        Self {
            path: Some(path),
            snapshot: RwLock::new(loaded),
        }
    }

    fn save_locked(&self, snapshot: &FactsSnapshot) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(snapshot).map_err(|error| error.to_string())?;
        crate::utils::fs::atomic_write(path, &bytes).map_err(|error| error.to_string())
    }

    /// 当前版本号：任何写入都会 +1，提醒点与前端刷新都按它判断。
    pub fn revision(&self) -> u32 {
        self.snapshot.read().revision
    }

    pub fn list(&self) -> Vec<WorkFact> {
        self.snapshot.read().facts.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.snapshot.read().facts.is_empty()
    }

    /// 注入提示词的正文。空清单返回空串，调用方直接跳过注入。
    pub fn render(&self) -> String {
        let snapshot = self.snapshot.read();
        if snapshot.facts.is_empty() {
            return String::new();
        }
        let mut text = format!(
            "[跨会话工作事实 rev {}]\n以下条目由其他 work 会话或用户确认，可直接用来省略重复询问；\
             与你在本会话里直接观察到的证据冲突时，以本会话证据为准，并说明冲突。\n",
            snapshot.revision
        );
        for fact in &snapshot.facts {
            let tag = if fact.updated_by == WRITER_USER {
                "用户确认"
            } else {
                "会话确认"
            };
            text.push_str(&format!("- {}（{}）\n", fact.text, tag));
        }
        text
    }

    /// 新增一条事实。重复文本与超上限都返回错误，交调用方转述给模型。
    pub fn add(&self, text: &str, session_id: &str, writer: &str) -> Result<WorkFact, String> {
        let text = normalize(text).ok_or_else(|| "事实内容不能为空".to_string())?;
        let mut snapshot = self.snapshot.write();
        if let Some(existing) = snapshot.facts.iter().find(|fact| same_text(&fact.text, &text)) {
            return Ok(existing.clone());
        }
        if snapshot.facts.len() >= MAX_FACTS {
            return Err(format!(
                "跨会话事实已达上限（{MAX_FACTS} 条），请先合并或删除过时条目"
            ));
        }
        snapshot.next_id = snapshot.next_id.saturating_add(1);
        snapshot.revision = snapshot.revision.saturating_add(1);
        let fact = WorkFact {
            id: format!("fact_{}", snapshot.next_id),
            text,
            source_session: session_id.chars().take(MAX_SESSION_LABEL).collect(),
            updated_by: writer.to_string(),
            revision: 1,
            updated_at: unix_now(),
        };
        snapshot.facts.push(fact.clone());
        self.save_locked(&snapshot)?;
        Ok(fact)
    }

    /// 改写一条事实（按 id）。内容未变化时不写盘，也不推进版本。
    pub fn update(&self, id: &str, text: &str, writer: &str) -> Result<WorkFact, String> {
        let text = normalize(text).ok_or_else(|| "事实内容不能为空".to_string())?;
        let mut snapshot = self.snapshot.write();
        if let Some(duplicate) = snapshot
            .facts
            .iter()
            .find(|fact| fact.id != id && same_text(&fact.text, &text))
        {
            return Err(format!("已存在相同事实 {}，请直接改那一条", duplicate.id));
        }
        let Some(index) = snapshot.facts.iter().position(|fact| fact.id == id) else {
            return Err(format!("找不到事实 {id}"));
        };
        if same_text(&snapshot.facts[index].text, &text) {
            return Ok(snapshot.facts[index].clone());
        }
        snapshot.revision = snapshot.revision.saturating_add(1);
        let fact = &mut snapshot.facts[index];
        fact.text = text;
        fact.updated_by = writer.to_string();
        fact.revision = fact.revision.saturating_add(1);
        fact.updated_at = unix_now();
        let fact = fact.clone();
        self.save_locked(&snapshot)?;
        Ok(fact)
    }

    /// 删除一条事实。返回是否真的删掉了。
    pub fn remove(&self, id: &str) -> Result<bool, String> {
        let mut snapshot = self.snapshot.write();
        let before = snapshot.facts.len();
        snapshot.facts.retain(|fact| fact.id != id);
        if snapshot.facts.len() == before {
            return Ok(false);
        }
        snapshot.revision = snapshot.revision.saturating_add(1);
        self.save_locked(&snapshot)?;
        Ok(true)
    }

    /// 清空清单（用户操作）。空清单不清盘、不推进版本。
    pub fn clear(&self) -> Result<bool, String> {
        let mut snapshot = self.snapshot.write();
        if snapshot.facts.is_empty() {
            return Ok(false);
        }
        snapshot.facts.clear();
        snapshot.revision = snapshot.revision.saturating_add(1);
        self.save_locked(&snapshot)?;
        Ok(true)
    }
}

impl Default for WorkFactsStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(test))]
static WORK_FACTS: once_cell::sync::Lazy<std::sync::Arc<WorkFactsStore>> =
    once_cell::sync::Lazy::new(|| {
        std::sync::Arc::new(WorkFactsStore::persistent(
            crate::utils::path::get_user_data_dir().join("work_facts.json"),
        ))
    });

#[cfg(test)]
static WORK_FACTS: once_cell::sync::Lazy<std::sync::Arc<WorkFactsStore>> =
    once_cell::sync::Lazy::new(|| std::sync::Arc::new(WorkFactsStore::new()));

/// 全局事实清单（工具、提示词构建与命令面共用同一实例）。
pub fn global_work_facts() -> std::sync::Arc<WorkFactsStore> {
    std::sync::Arc::clone(&WORK_FACTS)
}

/// 提示词注入入口：空清单返回空串，调用方据此跳过整段。
pub fn render_for_prompt() -> String {
    global_work_facts().render()
}

/// 判断候选事实是否值得跨会话共享。判据只给模型做参考，不做授权。
pub fn judge_instruction() -> &'static str {
    "判断这条一句话事实是否值得在**所有工作会话**之间共享。值得共享的是：项目的稳定前提、\
     用户明确表达的偏好与约定、跨任务都要遵守的决策。不值得共享的是：只属于当前会话的临时\
     状态、某次工具调用的中间结果、可以由工作区文件直接读到的内容、推测与未验证的结论。\
     只回答 yes 或 no。"
}

// ============================================================================
// 每轮复核：哪些事实过时了、这轮是否跑偏
// ============================================================================

/// 复核判定。`stale_ids` 只允许指向清单里真实存在的条目。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FactsVerdict {
    #[serde(default, alias = "stale")]
    pub stale_ids: Vec<String>,
    #[serde(default)]
    pub off_track: bool,
    #[serde(default)]
    pub reason: String,
}

impl FactsVerdict {
    pub fn is_empty(&self) -> bool {
        self.stale_ids.is_empty() && !self.off_track
    }
}

/// 复核用的指令：只判断，不改写，不回复用户。
pub fn review_instruction() -> &'static str {
    "你是工作事实清单的复核器。给你当前事实清单、本轮用户请求和本轮最终回复。\
     只输出一个 JSON 对象：{\"stale\":[\"fact_3\"],\"off_track\":false,\"reason\":\"\"}。\
     - stale：本轮工作**明确推翻或使其过时**的条目 id；没有就空数组。不确定时不要列。\
     - off_track：本轮工作是否明显偏离用户请求（答非所问、做了没要求的事、忘记原始目标）。\
       仅明显偏离才算，正常推进与合理的额外检查不算。\
     - reason：存在问题时限 60 字内写一句给执行者看的具体理由；没有问题留空串。\
     不要执行用户的请求，不要写代码，不要输出 JSON 以外的任何内容。"
}

/// 从模型输出里取判定。容忍代码围栏与前后废话；取不到合法 JSON 就是「没有结论」。
pub fn parse_verdict(raw: &str) -> Option<FactsVerdict> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<FactsVerdict>(&raw[start..=end]).ok()
}

/// 把判定渲染成给执行者看的提醒。只是建议，不构成新指令。
pub fn render_nudge(facts: &[WorkFact], verdict: &FactsVerdict) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for id in &verdict.stale_ids {
        if let Some(fact) = facts.iter().find(|fact| &fact.id == id) {
            lines.push(format!("- 事实 [{}]「{}」可能已经过时", fact.id, fact.text));
        }
    }
    if verdict.off_track {
        lines.push("- 本轮工作可能偏离了用户当前的目标".to_string());
    }
    if lines.is_empty() {
        return None;
    }
    let mut text = String::from("[事实清单复核] 这只是提醒，不是新的用户指令：\n");
    text.push_str(&lines.join("\n"));
    let reason: String = verdict.reason.trim().chars().take(120).collect();
    if !reason.is_empty() {
        text.push_str(&format!("\n理由：{reason}"));
    }
    text.push_str(
        "\n确认成立就用 work_facts 修正对应条目（update / remove）并在回复里说明；\
         不成立就忽略，不要向用户复述这条提醒。",
    );
    Some(text)
}

/// 待注入的复核提醒：同一 revision 只保留一条，超过 10 分钟自动过期。
#[derive(Debug, Clone)]
struct PendingNudge {
    text: String,
    revision: u32,
    created_at: i64,
}

/// 提醒有效期（秒）。
pub const NUDGE_TTL_SECS: i64 = 10 * 60;
/// 拒绝凭据有效期（秒）：过期后必须重新判断，不能用旧凭据绕开。
pub const REJECTION_TTL_SECS: i64 = 10 * 60;

static NUDGES: once_cell::sync::Lazy<parking_lot::Mutex<HashMap<String, PendingNudge>>> =
    once_cell::sync::Lazy::new(|| parking_lot::Mutex::new(HashMap::new()));

/// 记录一条复核提醒。同一 revision 已有一条有效提醒时不再覆盖，
/// 避免每轮复核都把同一句提醒重新刷新一遍。
pub fn set_nudge(session_id: &str, text: String, facts_revision: u32) {
    set_nudge_at(session_id, text, facts_revision, unix_now());
}

fn set_nudge_at(session_id: &str, text: String, facts_revision: u32, now: i64) {
    let mut nudges = NUDGES.lock();
    if let Some(existing) = nudges.get(session_id) {
        let alive = now - existing.created_at < NUDGE_TTL_SECS;
        if alive && existing.revision == facts_revision {
            return;
        }
    }
    nudges.insert(
        session_id.to_string(),
        PendingNudge { text, revision: facts_revision, created_at: now },
    );
}

/// 取本轮要注入的提醒（不消费：同一轮多次构建上下文时保持一致）。
pub fn prompt_nudge(session_id: &str) -> Option<String> {
    prompt_nudge_at(session_id, unix_now())
}

fn prompt_nudge_at(session_id: &str, now: i64) -> Option<String> {
    let mut nudges = NUDGES.lock();
    let nudge = nudges.get(session_id)?;
    if now - nudge.created_at >= NUDGE_TTL_SECS {
        nudges.remove(session_id);
        return None;
    }
    Some(nudge.text.clone())
}

/// 清理某个会话的提醒（会话删除时用）。
pub fn clear_nudge(session_id: &str) {
    NUDGES.lock().remove(session_id);
}

// ============================================================================
// 被拒更新的显式覆写
// ============================================================================

/// 一次被拒的写入：保留原文与时刻，供模型用拒绝编号显式再试。
#[derive(Debug, Clone)]
struct Rejection {
    session_id: String,
    text: String,
    created_at: i64,
}

static REJECTIONS: once_cell::sync::Lazy<parking_lot::Mutex<HashMap<String, Rejection>>> =
    once_cell::sync::Lazy::new(|| parking_lot::Mutex::new(HashMap::new()));
static NEXT_REJECTION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 记下一条被拒的事实，返回拒绝编号。同会话同文本复用同一个编号，
/// 避免模型反复失败时堆出一串凭据。
pub fn remember_rejection(session_id: &str, text: &str) -> String {
    remember_rejection_at(session_id, text, unix_now())
}

fn remember_rejection_at(session_id: &str, text: &str, now: i64) -> String {
    let mut rejections = REJECTIONS.lock();
    rejections.retain(|_, rejection| now - rejection.created_at < REJECTION_TTL_SECS);
    if let Some((id, _)) = rejections
        .iter()
        .find(|(_, rejection)| rejection.session_id == session_id && rejection.text == text)
    {
        return id.clone();
    }
    let id = format!(
        "reject_{}",
        NEXT_REJECTION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    rejections.insert(
        id.clone(),
        Rejection { session_id: session_id.to_string(), text: text.to_string(), created_at: now },
    );
    id
}

/// 用拒绝编号取出原文（一次性）。编号无效、过期或文本不匹配都返回 None。
pub fn take_rejection(session_id: &str, id: &str, text: &str) -> Option<String> {
    take_rejection_at(session_id, id, text, unix_now())
}

fn take_rejection_at(session_id: &str, id: &str, text: &str, now: i64) -> Option<String> {
    let mut rejections = REJECTIONS.lock();
    let rejection = rejections.get(id)?;
    if now - rejection.created_at >= REJECTION_TTL_SECS {
        rejections.remove(id);
        return None;
    }
    // 文本或会话不匹配时不消耗凭据：写错一次不该把这次机会烧掉。
    if rejection.session_id != session_id || rejection.text != text {
        return None;
    }
    let text = rejection.text.clone();
    rejections.remove(id);
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_are_normalized_and_deduplicated() {
        let store = WorkFactsStore::new();
        let added = store.add("  使用  rust  2021  ", "s1", WRITER_MODEL).unwrap();
        assert_eq!(added.text, "使用 rust 2021");
        assert_eq!(added.id, "fact_1");
        // 重复文本（大小写与空白差异）返回既有条目，不新增。
        let again = store.add("使用 rust 2021", "s2", WRITER_MODEL).unwrap();
        assert_eq!(again.id, "fact_1");
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn facts_cap_and_revision_track_writes() {
        let store = WorkFactsStore::new();
        for index in 0..MAX_FACTS {
            store.add(&format!("事实 {index}"), "s", WRITER_MODEL).unwrap();
        }
        assert!(store.add("超出上限", "s", WRITER_MODEL).is_err());
        let before = store.revision();
        store.remove("fact_1").unwrap();
        assert_eq!(store.revision(), before + 1);
        assert!(!store.remove("fact_1").unwrap());
        // 删除后腾出的位置可以再用。
        assert!(store.add("新的条目", "s", WRITER_MODEL).is_ok());
    }

    #[test]
    fn long_text_is_truncated_not_rejected() {
        let store = WorkFactsStore::new();
        let long = "长".repeat(MAX_FACT_CHARS + 50);
        let fact = store.add(&long, "s", WRITER_MODEL).unwrap();
        assert_eq!(fact.text.chars().count(), MAX_FACT_CHARS);
        assert!(store.add("   ", "s", WRITER_MODEL).is_err());
    }

    #[test]
    fn empty_list_renders_nothing_and_nonempty_lists_carry_revision() {
        let store = WorkFactsStore::new();
        assert!(store.render().is_empty());
        store.add("主工作区是 D:/work/api", "s", WRITER_MODEL).unwrap();
        store.add("提交信息使用中文", "s", WRITER_USER).unwrap();
        let text = store.render();
        assert!(text.contains("rev 2"));
        assert!(text.contains("主工作区是 D:/work/api"));
        assert!(text.contains("用户确认"));
    }

    #[test]
    fn update_rejects_duplicates_and_missing_ids() {
        let store = WorkFactsStore::new();
        let first = store.add("同一个事实", "s", WRITER_MODEL).unwrap();
        let second = store.add("另一个事实", "s", WRITER_MODEL).unwrap();
        assert!(store.update(&second.id, "同一个事实", WRITER_USER).is_err());
        assert!(store.update("fact_404", "随便", WRITER_USER).is_err());
        let updated = store.update(&first.id, "改过的事实", WRITER_USER).unwrap();
        assert_eq!(updated.revision, 2);
        assert_eq!(updated.updated_by, WRITER_USER);
        // 内容未变时不推进版本。
        let revision = store.revision();
        store.update(&first.id, "改过的事实", WRITER_USER).unwrap();
        assert_eq!(store.revision(), revision);
    }

    #[test]
    fn verdict_parsing_tolerates_fences_and_rejects_junk() {
        let verdict = parse_verdict("```json\n{\"stale\":[\"fact_1\"],\"off_track\":true,\"reason\":\"目标变了\"}\n```").unwrap();
        assert_eq!(verdict.stale_ids, ["fact_1"]);
        assert!(verdict.off_track);
        assert_eq!(verdict.reason, "目标变了");
        // 旧字段名 stale_ids 也能读。
        assert_eq!(
            parse_verdict("{\"stale_ids\":[\"fact_2\"],\"off_track\":false}").unwrap().stale_ids,
            ["fact_2"]
        );
        assert!(parse_verdict("没有 JSON").is_none());
        assert!(parse_verdict("{broken").is_none());
        assert!(FactsVerdict::default().is_empty());
    }

    #[test]
    fn nudge_only_mentions_known_facts_and_needs_a_reason_to_exist() {
        let store = WorkFactsStore::new();
        let fact = store.add("主工作区是 D:/work/api", "s", WRITER_MODEL).unwrap();
        let quiet = FactsVerdict { stale_ids: vec!["fact_404".into()], off_track: false, reason: "随便".into() };
        assert!(render_nudge(&store.list(), &quiet).is_none(), "未知 id 不制造提醒");

        let verdict = FactsVerdict { stale_ids: vec![fact.id.clone()], off_track: true, reason: "目录已迁移".into() };
        let text = render_nudge(&store.list(), &verdict).unwrap();
        assert!(text.contains("可能已经过时"));
        assert!(text.contains("目录已迁移"));
        assert!(text.contains("这只是提醒"));
    }

    #[test]
    fn nudges_dedupe_by_revision_and_expire() {
        let session = "nudge-test-session";
        set_nudge_at(session, "第一条".into(), 3, 1_000);
        assert_eq!(prompt_nudge_at(session, 1_010).as_deref(), Some("第一条"));
        // 同一 revision 的重复判定不刷新已有提醒。
        set_nudge_at(session, "重复提醒".into(), 3, 1_020);
        assert_eq!(prompt_nudge_at(session, 1_030).as_deref(), Some("第一条"));
        // revision 变了才替换。
        set_nudge_at(session, "第二条".into(), 4, 1_040);
        assert_eq!(prompt_nudge_at(session, 1_050).as_deref(), Some("第二条"));
        // 超过有效期即消失。
        assert_eq!(prompt_nudge_at(session, 1_040 + NUDGE_TTL_SECS), None);
        assert_eq!(prompt_nudge_at(session, 1_500).as_deref(), None);
        // 过期后同一 revision 可以重新写入。
        set_nudge_at(session, "重新提醒".into(), 4, 5_000);
        assert_eq!(prompt_nudge_at(session, 5_001).as_deref(), Some("重新提醒"));
        clear_nudge(session);
        assert!(prompt_nudge_at(session, 5_002).is_none());
    }

    #[test]
    fn rejections_are_single_use_scoped_and_expiring() {
        let session = "reject-session";
        let id = remember_rejection_at(session, "某条事实", 1_000);
        assert_eq!(
            remember_rejection_at(session, "某条事实", 1_010),
            id,
            "同会话同文本复用同一个编号"
        );
        // 文本不匹配不消耗凭据。
        assert!(take_rejection_at(session, &id, "别的文本", 1_020).is_none());
        assert_eq!(
            take_rejection_at(session, &id, "某条事实", 1_030).as_deref(),
            Some("某条事实")
        );
        assert!(
            take_rejection_at(session, &id, "某条事实", 1_040).is_none(),
            "凭据只能用一次"
        );

        let other = remember_rejection_at("another-session", "跨会话事实", 2_000);
        assert!(take_rejection_at(session, &other, "跨会话事实", 2_010).is_none());

        let expired = remember_rejection_at(session, "过期事实", 3_000);
        assert!(take_rejection_at(
            session,
            &expired,
            "过期事实",
            3_000 + REJECTION_TTL_SECS
        )
        .is_none());
    }
}
