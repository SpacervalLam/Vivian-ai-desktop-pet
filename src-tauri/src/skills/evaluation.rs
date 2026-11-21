//! 独立选择集、配对回放与严格改善接受闸门。
//!
//! 这里回答阶段 1 刻意没回答的问题：**新版本比现行版本更好吗**。判定过程：
//!
//! 1. 宿主维护 `skills/.eval/selection.json`，包含针对某个技能的正样本（应被召回）、
//!    负样本（不应被召回）与 `protected` 保留集样本。模型既不能写这个文件，
//!    拒绝反馈里也不会回传题目原文，因此无法自己出题或逐条过拟合。
//! 2. 对同一批样本，分别用现行版本与候选版本回放**运行时真实的选择函数**，
//!    逐样本比较「是否判对」，得到 `baseline_score` / `candidate_score` / `delta`。
//! 3. 只有 `delta >= delta_min`、确有样本由错变对、保留集零退化、无误召回净增，
//!    才判 `Accepted`；否则 `Rejected`。评估集缺失或样本不足是 `Unverifiable`
//!    —— 与「验证后判定为差」严格区分，绝不当作通过。
//! 4. 未通过的候选落到 `skills/.staging/`。点前缀目录不会被 `load_from_dir` /
//!    `dir_fingerprint` 扫描，因此未验证的候选永远不会生效，也不会触发热重载。
//!
//! ## 选择集不可被会话内改写
//!
//! 「点前缀 + create_skill 没有动作能写它」只是**目录隔离**，不是权限隔离：
//! 通用文件工具（`write_file` / `edit_file` / `run_command`）仍可能触达同一路径。
//! 因此宿主在进程启动时用 [`pin_selection_set`] 把选择集摘要锁进 `OnceLock`，
//! 每次评估走 `load_selection_set_locked` 复核摘要；会话内任何改动都让评估变成
//! `Unverifiable`。要变更选择集，必须由宿主放好文件并重启应用（模型无法重启宿主）。
//! 这不是密码学意义的防篡改：能直接控制宿主进程/磁盘的一方始终在信任边界内。
//!
//! 本模块刻意只依赖 `std` / `serde_json` / `sha2`，不引用 `super::`，可以单独
//! 编译成离线契约测试 crate（见 `tests/skill-publish.test.mjs`）。真实回放的
//! 桥接在 `selection.rs`，本模块只消费注入的判定结果，不负责打分。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// 评估集目录与文件名。点前缀确保不进入技能加载与热重载扫描。
pub const EVAL_DIR: &str = ".eval";
pub const EVAL_FILE: &str = "selection.json";
/// 未通过闸门的候选暂存目录，同样点前缀。
pub const STAGING_DIR: &str = ".staging";

/// 一个技能至少要有这么多样本，评估才有统计意义。
pub const MIN_CASES: usize = 5;
pub const MIN_POSITIVE_CASES: usize = 2;
pub const MIN_NEGATIVE_CASES: usize = 1;
pub const MIN_PROTECTED_CASES: usize = 1;
pub const MAX_CASES: usize = 200;
/// 宿主可以调高门槛，但不能调到 0 来变相关闭闸门。
pub const MIN_DELTA_MIN: f64 = 1.0;
pub const DEFAULT_DELTA_MIN: f64 = 1.0;
const MAX_QUERY_CHARS: usize = 500;
const MAX_WEIGHT: f64 = 10.0;
const MAX_CANDIDATES_PER_SKILL: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct EvalCase {
    pub id: String,
    pub query: String,
    pub should_hit: bool,
    /// 保留集样本：任何退化都直接否决，不允许用别处的收益抵消。
    pub protected: bool,
    pub weight: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectionSet {
    pub skill: String,
    pub delta_min: f64,
    pub cases: Vec<EvalCase>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    Rejected,
    /// 评估集缺失、损坏或样本不足：无法判定，自动发布必须 fail-closed。
    Unverifiable,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Accepted => "accepted",
            Verdict::Rejected => "rejected",
            Verdict::Unverifiable => "unverifiable",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaseOutcome {
    pub id: String,
    pub protected: bool,
    pub baseline_hit: bool,
    pub candidate_hit: bool,
    pub baseline_correct: bool,
    pub candidate_correct: bool,
    pub weight: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    pub verdict: Verdict,
    pub baseline_score: f64,
    pub candidate_score: f64,
    pub delta: f64,
    pub delta_min: f64,
    pub improved: usize,
    pub regressed: usize,
    pub protected_regressions: Vec<String>,
    pub new_false_positives: Vec<String>,
    pub reasons: Vec<String>,
    pub case_count: usize,
    pub outcomes: Vec<CaseOutcome>,
}

impl Evaluation {
    /// 没有可用评估集时的结论。与 `Rejected` 分开，便于宿主给出不同的修复提示。
    pub fn unverifiable(reason: impl Into<String>) -> Self {
        Self {
            verdict: Verdict::Unverifiable,
            baseline_score: 0.0,
            candidate_score: 0.0,
            delta: 0.0,
            delta_min: DEFAULT_DELTA_MIN,
            improved: 0,
            regressed: 0,
            protected_regressions: Vec::new(),
            new_false_positives: Vec::new(),
            reasons: vec![reason.into()],
            case_count: 0,
            outcomes: Vec::new(),
        }
    }

    /// 可回传给模型的反馈。**刻意不含 query / should_hit / 命中方向**：
    /// 模型只能看到「哪些 case id 出了问题、整体差多少」，无法针对题目逐条硬编码。
    pub fn to_value(&self) -> Value {
        let outcomes: Vec<Value> = self
            .outcomes
            .iter()
            .map(|o| {
                json!({
                    "id": o.id,
                    "protected": o.protected,
                    "baseline_correct": o.baseline_correct,
                    "candidate_correct": o.candidate_correct,
                })
            })
            .collect();
        json!({
            "verdict": self.verdict.as_str(),
            "case_count": self.case_count,
            "baseline_score": round3(self.baseline_score),
            "candidate_score": round3(self.candidate_score),
            "delta": round3(self.delta),
            "delta_min": self.delta_min,
            "improved": self.improved,
            "regressed": self.regressed,
            "protected_regressions": self.protected_regressions,
            "new_false_positives": self.new_false_positives,
            "reasons": self.reasons,
            "cases": outcomes,
            "queries_disclosed": false,
        })
    }

    pub fn reason_text(&self) -> String {
        if self.reasons.is_empty() {
            "通过".to_string()
        } else {
            self.reasons.join("；")
        }
    }
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn sha_prefix(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))[..16].to_string()
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

/// 与 `publish::safe_component` 同规则；这里不引用别的模块以保持离线可编译。
fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.starts_with('.')
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains(':')
        && value.chars().count() <= 64
}

pub fn eval_file(skills_dir: &Path) -> PathBuf {
    skills_dir.join(EVAL_DIR).join(EVAL_FILE)
}

/// 进程启动时锁定的选择集摘要。`None` = 启动时没有可用选择集。
///
/// 只在 `SkillService::load_default_dir()` 首次装载时写入一次；热重载与模型侧的
/// 任何动作都不能重设它。用 `OnceLock` 而不是 RwLock，避免留下可被调用的解锁口。
static PINNED_SELECTION: OnceLock<Option<String>> = OnceLock::new();

/// 选择集的全文摘要。用完整 SHA-256，避免前缀碰撞给篡改留缝隙。
pub fn selection_digest(raw: &str) -> String {
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}

/// 由宿主在进程启动时调用一次。后续调用是 no-op（`OnceLock` 只接受第一次）。
///
/// 文件缺失或不是合法 JSON 时锁定 `None`：自动发布保持暂停，而不是退化成
/// 「没验证就放行」。放好选择集后必须重启应用才会生效。
pub fn pin_selection_set(skills_dir: &Path) {
    let digest = std::fs::read_to_string(eval_file(skills_dir))
        .ok()
        .filter(|raw| serde_json::from_str::<Value>(raw).is_ok())
        .map(|raw| selection_digest(&raw));
    let _ = PINNED_SELECTION.set(digest);
}

/// 校验「当前磁盘上的选择集」与启动快照是否一致。做成纯函数以便离线测。
pub fn verify_selection_set(raw: &str, pinned: Option<&str>) -> Result<(), String> {
    let Some(expected) = pinned else {
        return Err(
            "进程启动时没有可用的独立选择集：请由宿主放置 selection.json 后重启应用".into(),
        );
    };
    if selection_digest(raw) != expected {
        return Err(
            "独立选择集在进程启动后被修改，本次评估作废；须由宿主确认内容并重启应用重新锁定"
                .into(),
        );
    }
    Ok(())
}

/// 模型侧发布路径专用的读取入口：先做启动锁定校验，再做格式与配比校验。
///
/// 任何一步失败都返回 `Err`，调用方必须转成 `Unverifiable` 并 fail-closed。
pub fn load_selection_set_locked(
    skills_dir: &Path,
    skill: &str,
) -> Result<SelectionSet, String> {
    let pinned = PINNED_SELECTION
        .get()
        .cloned()
        .ok_or_else(|| "独立选择集未在进程启动时锁定（宿主未完成初始化）".to_string())?;
    let raw = std::fs::read_to_string(eval_file(skills_dir))
        .map_err(|e| format!("独立选择集无法读取：{e}"))?;
    verify_selection_set(&raw, pinned.as_deref())?;
    load_selection_set(skills_dir, skill)
}

/// 读取宿主维护的独立选择集（不做启动锁定校验；供离线测试与宿主自检使用）。
///
/// 预期结构：
/// ```json
/// { "version": 1, "sets": [
///   { "skill": "deploy-helper", "delta_min": 1.0, "cases": [
///     { "id": "p1", "query": "帮我上线", "should_hit": true, "protected": true, "weight": 1.0 },
///     { "id": "n1", "query": "讲个笑话", "should_hit": false, "protected": true }
///   ]}
/// ]}
/// ```
///
/// 任何格式/配比问题都返回 `Err`，由宿主转成 `Unverifiable`，绝不降级为「通过」。
pub fn load_selection_set(skills_dir: &Path, skill: &str) -> Result<SelectionSet, String> {
    let path = eval_file(skills_dir);
    let raw = std::fs::read_to_string(&path)
        .map_err(|_| format!("缺少独立选择集 {}，无法验证改善", path.display()))?;
    let doc: Value = serde_json::from_str(&raw)
        .map_err(|e| format!("独立选择集解析失败：{e}"))?;
    let sets = doc
        .get("sets")
        .and_then(Value::as_array)
        .ok_or_else(|| "独立选择集缺少 sets 数组".to_string())?;
    let entry = sets
        .iter()
        .find(|s| s.get("skill").and_then(Value::as_str) == Some(skill))
        .ok_or_else(|| format!("独立选择集中没有技能「{skill}」的样本"))?;

    let delta_min = entry
        .get("delta_min")
        .and_then(Value::as_f64)
        .unwrap_or(DEFAULT_DELTA_MIN);
    if !delta_min.is_finite() || delta_min < MIN_DELTA_MIN {
        return Err(format!(
            "delta_min={delta_min} 不合法：不得低于 {MIN_DELTA_MIN}，不能用低门槛变相关闭闸门"
        ));
    }

    let raw_cases = entry
        .get("cases")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("技能「{skill}」缺少 cases 数组"))?;
    if raw_cases.len() > MAX_CASES {
        return Err(format!("cases 有 {} 条，超过上限 {MAX_CASES}", raw_cases.len()));
    }

    let mut ids = std::collections::HashSet::new();
    let mut cases = Vec::with_capacity(raw_cases.len());
    for (index, raw_case) in raw_cases.iter().enumerate() {
        let id = raw_case
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if id.is_empty() || id.chars().count() > 64 || !ids.insert(id.clone()) {
            return Err(format!("第 {index} 条样本 id 为空或重复"));
        }
        let query = raw_case
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if query.is_empty() || query.chars().count() > MAX_QUERY_CHARS {
            return Err(format!("样本「{id}」的 query 为空或过长"));
        }
        let should_hit = raw_case
            .get("should_hit")
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("样本「{id}」缺少布尔 should_hit"))?;
        let protected = raw_case
            .get("protected")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let weight = raw_case
            .get("weight")
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        if !weight.is_finite() || weight <= 0.0 || weight > MAX_WEIGHT {
            return Err(format!("样本「{id}」的 weight 必须在 (0, {MAX_WEIGHT}]"));
        }
        cases.push(EvalCase {
            id,
            query,
            should_hit,
            protected,
            weight,
        });
    }

    let positives = cases.iter().filter(|c| c.should_hit).count();
    let negatives = cases.len() - positives;
    let protected = cases.iter().filter(|c| c.protected).count();
    if cases.len() < MIN_CASES
        || positives < MIN_POSITIVE_CASES
        || negatives < MIN_NEGATIVE_CASES
        || protected < MIN_PROTECTED_CASES
    {
        return Err(format!(
            "技能「{skill}」的选择集样本不足：共 {} 条（正 {} / 负 {} / 保留 {}），\
             至少需要 {MIN_CASES} 条、正 {MIN_POSITIVE_CASES} 条、负 {MIN_NEGATIVE_CASES} 条、保留 {MIN_PROTECTED_CASES} 条",
            cases.len(), positives, negatives, protected
        ));
    }

    Ok(SelectionSet {
        skill: skill.to_string(),
        delta_min,
        cases,
    })
}

/// 配对回放的核心判定。
///
/// `hit(case, is_candidate)` 由宿主注入，必须回放**运行时真实的选择函数**，
/// 不能用近似指标代替。同一条样本在两侧各判定一次，除此之外没有任何输入差异。
pub fn evaluate_paired<F>(set: &SelectionSet, mut hit: F) -> Evaluation
where
    F: FnMut(&EvalCase, bool) -> bool,
{
    let mut outcomes = Vec::with_capacity(set.cases.len());
    let mut baseline_score = 0.0;
    let mut candidate_score = 0.0;
    let mut improved = 0usize;
    let mut regressed = 0usize;
    let mut protected_regressions = Vec::new();
    let mut new_false_positives = Vec::new();

    for case in &set.cases {
        let baseline_hit = hit(case, false);
        let candidate_hit = hit(case, true);
        let baseline_correct = baseline_hit == case.should_hit;
        let candidate_correct = candidate_hit == case.should_hit;
        if baseline_correct {
            baseline_score += case.weight;
        }
        if candidate_correct {
            candidate_score += case.weight;
        }
        if !baseline_correct && candidate_correct {
            improved += 1;
        }
        if baseline_correct && !candidate_correct {
            regressed += 1;
            if case.protected {
                protected_regressions.push(case.id.clone());
            }
        }
        if !case.should_hit && candidate_hit && !baseline_hit {
            new_false_positives.push(case.id.clone());
        }
        outcomes.push(CaseOutcome {
            id: case.id.clone(),
            protected: case.protected,
            baseline_hit,
            candidate_hit,
            baseline_correct,
            candidate_correct,
            weight: case.weight,
        });
    }

    let delta = candidate_score - baseline_score;
    let mut reasons = Vec::new();
    if delta + 1e-9 < set.delta_min {
        reasons.push(format!(
            "独立选择集净收益 {:.3} 未达到门槛 {:.3}",
            delta, set.delta_min
        ));
    }
    if improved == 0 {
        reasons.push("没有任何样本由判错变为判对".to_string());
    }
    if !protected_regressions.is_empty() {
        reasons.push(format!(
            "保留集样本退化（硬不变量）：{}",
            protected_regressions.join(", ")
        ));
    }
    if !new_false_positives.is_empty() {
        reasons.push(format!(
            "新增误召回：{}",
            new_false_positives.join(", ")
        ));
    }

    Evaluation {
        verdict: if reasons.is_empty() {
            Verdict::Accepted
        } else {
            Verdict::Rejected
        },
        baseline_score,
        candidate_score,
        delta,
        delta_min: set.delta_min,
        improved,
        regressed,
        protected_regressions,
        new_false_positives,
        reasons,
        case_count: set.cases.len(),
        outcomes,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateMeta {
    pub id: String,
    pub name: String,
    pub created_at_ms: u64,
    pub verdict: String,
}

pub struct StoredCandidate {
    pub name: String,
    pub content: String,
    pub meta: CandidateMeta,
    /// 原始写入请求的布局。`promote` 必须沿用，不能偷偷改成 auto。
    pub layout: String,
    /// 原始写入请求携带的资源。重评通过后要一起发布，不能静默丢弃。
    pub resources: Vec<(String, String)>,
}

const MAX_STAGED_RESOURCE_CHARS: usize = 256 * 1024;

/// 暂存资源路径与发布侧 `is_safe_skill_rel_path` 同口径：只允许
/// `references/` / `scripts/` 下的一层普通文件名。
fn safe_resource_rel(path: &str) -> bool {
    let mut parts = path.split('/');
    let root = parts.next().unwrap_or("");
    if root != "references" && root != "scripts" {
        return false;
    }
    let Some(file) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    !file.is_empty()
        && file != "."
        && file != ".."
        && !file.starts_with('.')
        && !file.contains('\\')
        && !file.contains(':')
}

fn candidate_root(skills_dir: &Path, name: &str) -> Result<PathBuf, String> {
    if !safe_component(name) {
        return Err("技能名不能用作候选目录".into());
    }
    Ok(skills_dir.join(STAGING_DIR).join(name))
}

fn parse_candidate_id(id: &str) -> Result<(String, String), String> {
    let (name, dir_name) = id
        .trim()
        .split_once(':')
        .ok_or_else(|| "candidate_id 格式应为 <name>:<stamp>-<rev>".to_string())?;
    if !safe_component(name) {
        return Err("candidate_id 中的技能名不合法".into());
    }
    if dir_name.is_empty()
        || dir_name.starts_with('.')
        || !dir_name
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_hexdigit() || c == '-')
        || !dir_name.contains('-')
    {
        return Err("candidate_id 中的版本段不合法".into());
    }
    Ok((name.to_string(), dir_name.to_string()))
}

/// 把未通过闸门的候选完整存下来，返回可交给 `action=promote` 的 id。
///
/// 存的是**候选正文**而不是重新序列化的字段，宿主补齐选择集后可以字节一致地重评并发布。
/// 布局与资源也必须一起存：`promote` 是补齐选择集后唯一的重评入口，丢掉它们就等于
/// 让一次原本带资源的写入在重评通过后变成残缺版本。
pub fn save_candidate_with(
    skills_dir: &Path,
    name: &str,
    content: &str,
    evaluation: &Evaluation,
    layout: &str,
    resources: &[(String, String)],
) -> Result<String, String> {
    let root = candidate_root(skills_dir, name)?;
    std::fs::create_dir_all(&root).map_err(|e| format!("创建候选目录失败: {e}"))?;
    let stamp = now_millis();
    let rev = sha_prefix(content);
    // 同一毫秒内提交两份相同正文会产生同名目录并互相覆盖；加序数保证候选不丢。
    let mut dir_name = format!("{stamp}-{rev}");
    let mut dir = root.join(&dir_name);
    for attempt in 1..100u32 {
        if !dir.exists() {
            break;
        }
        dir_name = format!("{stamp}-{rev}-{attempt}");
        dir = root.join(&dir_name);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建候选版本目录失败: {e}"))?;
    std::fs::write(dir.join("SKILL.md"), content)
        .map_err(|e| format!("保存候选正文失败: {e}"))?;
    let stored_resources: Vec<Value> = resources
        .iter()
        .filter(|(path, text)| {
            safe_resource_rel(path) && text.len() <= MAX_STAGED_RESOURCE_CHARS
        })
        .map(|(path, text)| json!({ "path": path, "content": text }))
        .collect();
    let meta = json!({
        "id": format!("{name}:{dir_name}"),
        "name": name,
        "created_at_ms": stamp,
        "verdict": evaluation.verdict.as_str(),
        "revision": rev,
        "layout": if layout == "flat" { "flat" } else { "folder" },
        "resources": stored_resources,
        "feedback": evaluation.to_value(),
    });
    std::fs::write(
        dir.join("feedback.json"),
        serde_json::to_string_pretty(&meta).unwrap_or_default(),
    )
    .map_err(|e| format!("保存候选反馈失败: {e}"))?;
    prune_candidates(&root)?;
    Ok(format!("{name}:{dir_name}"))
}

/// 不带布局/资源信息的简化入口，供离线用例与不需要重评的宿主路径使用。
pub fn save_candidate(
    skills_dir: &Path,
    name: &str,
    content: &str,
    evaluation: &Evaluation,
) -> Result<String, String> {
    save_candidate_with(skills_dir, name, content, evaluation, "folder", &[])
}

fn prune_candidates(root: &Path) -> Result<(), String> {
    let mut dirs: Vec<(String, PathBuf)> = read_candidate_dirs(root)?
        .into_iter()
        .collect();
    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, dir) in dirs.iter().skip(MAX_CANDIDATES_PER_SKILL) {
        let _ = std::fs::remove_dir_all(dir);
    }
    Ok(())
}

fn read_candidate_dirs(root: &Path) -> Result<std::collections::HashMap<String, PathBuf>, String> {
    let mut out = std::collections::HashMap::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(out);
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        if dir_name.starts_with('.') || !dir_name.contains('-') {
            continue;
        }
        out.insert(dir_name, entry.path());
    }
    Ok(out)
}

pub fn list_candidates(skills_dir: &Path, name: &str) -> Result<Vec<CandidateMeta>, String> {
    let root = candidate_root(skills_dir, name)?;
    let mut items = Vec::new();
    for (dir_name, dir) in read_candidate_dirs(&root)? {
        let raw = std::fs::read_to_string(dir.join("feedback.json")).unwrap_or_default();
        let value: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
        items.push(CandidateMeta {
            id: format!("{name}:{dir_name}"),
            name: name.to_string(),
            created_at_ms: value.get("created_at_ms").and_then(Value::as_u64).unwrap_or(0),
            verdict: value
                .get("verdict")
                .and_then(Value::as_str)
                .unwrap_or("unverifiable")
                .to_string(),
        });
    }
    items.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
    Ok(items)
}

pub fn load_candidate(skills_dir: &Path, id: &str) -> Result<StoredCandidate, String> {
    let (name, dir_name) = parse_candidate_id(id)?;
    let dir = candidate_root(skills_dir, &name)?.join(&dir_name);
    let content = std::fs::read_to_string(dir.join("SKILL.md"))
        .map_err(|_| format!("找不到候选 {id}"))?;
    let raw = std::fs::read_to_string(dir.join("feedback.json")).unwrap_or_default();
    let value: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
    let meta = CandidateMeta {
        id: id.to_string(),
        name: name.clone(),
        created_at_ms: value.get("created_at_ms").and_then(Value::as_u64).unwrap_or(0),
        verdict: value
            .get("verdict")
            .and_then(Value::as_str)
            .unwrap_or("unverifiable")
            .to_string(),
    };
    // 旧候选（阶段 2 初版）没有这两个字段，按目录布局、无资源兼容读回。
    let layout = if value.get("layout").and_then(Value::as_str) == Some("flat") {
        "flat".to_string()
    } else {
        "folder".to_string()
    };
    let resources: Vec<(String, String)> = value
        .get("resources")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let path = item.get("path").and_then(Value::as_str)?;
                    let text = item.get("content").and_then(Value::as_str)?;
                    if !safe_resource_rel(path) || text.len() > MAX_STAGED_RESOURCE_CHARS {
                        return None;
                    }
                    Some((path.to_string(), text.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(StoredCandidate {
        name,
        content,
        meta,
        layout,
        resources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(delta_min: f64, cases: Vec<EvalCase>) -> SelectionSet {
        SelectionSet {
            skill: "demo".into(),
            delta_min,
            cases,
        }
    }

    fn case(id: &str, should_hit: bool, protected: bool) -> EvalCase {
        EvalCase {
            id: id.into(),
            query: format!("秘密题目-{id}"),
            should_hit,
            protected,
            weight: 1.0,
        }
    }

    /// 6 条样本：4 正 2 负，2 条 protected。
    fn balanced() -> Vec<EvalCase> {
        vec![
            case("p1", true, true),
            case("p2", true, false),
            case("p3", true, false),
            case("p4", true, false),
            case("n1", false, true),
            case("n2", false, false),
        ]
    }

    fn set_json(cases: Value, delta_min: Value) -> Value {
        json!({
            "version": 1,
            "sets": [{ "skill": "demo", "delta_min": delta_min, "cases": cases }]
        })
    }

    fn write_set(dir: &Path, doc: &Value) {
        let folder = dir.join(EVAL_DIR);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(EVAL_FILE), doc.to_string()).unwrap();
    }

    fn raw_case(id: &str, should_hit: bool, protected: bool) -> Value {
        json!({ "id": id, "query": format!("题目-{id}"), "should_hit": should_hit, "protected": protected })
    }

    #[test]
    fn selection_set_requires_balanced_protected_cases_and_hidden_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path();
        assert!(load_selection_set(skills, "demo")
            .unwrap_err()
            .contains("缺少独立选择集"));

        let cases: Vec<Value> = (1..=4)
            .map(|i| raw_case(&format!("p{i}"), true, false))
            .chain(std::iter::once(raw_case("n1", false, false)))
            .collect();
        write_set(skills, &set_json(json!(cases), json!(1.0)));
        let err = load_selection_set(skills, "demo").unwrap_err();
        assert!(err.contains("保留"), "{err}");
        assert!(skills.join(EVAL_DIR).join(EVAL_FILE).is_file());

        let mut good: Vec<Value> = (1..=4)
            .map(|i| raw_case(&format!("p{i}"), true, false))
            .chain((1..=2).map(|i| raw_case(&format!("n{i}"), false, true)))
            .collect();
        // 上面只有 6 条但负样本 2 条、保留 2 条，应当通过。
        write_set(skills, &set_json(json!(good.clone()), json!(1.0)));
        let loaded = load_selection_set(skills, "demo").unwrap();
        assert_eq!(loaded.cases.len(), 6);
        assert_eq!(loaded.delta_min, 1.0);

        // 配比不足：全正样本。
        good = (1..=6).map(|i| raw_case(&format!("p{i}"), true, true)).collect();
        write_set(skills, &set_json(json!(good), json!(1.0)));
        assert!(load_selection_set(skills, "demo")
            .unwrap_err()
            .contains("负"));
    }

    #[test]
    fn selection_set_rejects_weak_gate_and_malformed_cases() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path();
        let cases: Vec<Value> = (1..=4)
            .map(|i| raw_case(&format!("p{i}"), true, false))
            .chain((1..=2).map(|i| raw_case(&format!("n{i}"), false, true)))
            .collect();

        write_set(skills, &set_json(json!(cases.clone()), json!(0.0)));
        assert!(load_selection_set(skills, "demo")
            .unwrap_err()
            .contains("delta_min"));

        let mut duplicated = cases.clone();
        duplicated[1] = raw_case("p1", true, false);
        write_set(skills, &set_json(json!(duplicated), json!(1.0)));
        assert!(load_selection_set(skills, "demo")
            .unwrap_err()
            .contains("重复"));

        let mut no_hit = cases.clone();
        no_hit[0] = json!({ "id": "p1", "query": "题目", "protected": true });
        write_set(skills, &set_json(json!(no_hit), json!(1.0)));
        assert!(load_selection_set(skills, "demo")
            .unwrap_err()
            .contains("should_hit"));

        write_set(skills, &json!({ "version": 1, "sets": [] }));
        assert!(load_selection_set(skills, "demo")
            .unwrap_err()
            .contains("没有技能"));
    }

    #[test]
    fn paired_evaluation_accepts_only_strict_improvement() {
        let cases = balanced();
        // 基线：p1/p2 命中失败，其余正确。候选：把 p2 修好。
        let baseline = |c: &EvalCase, _: bool| !matches!(c.id.as_str(), "p1" | "p2") && c.should_hit;
        let accepted = evaluate_paired(&set(1.0, cases.clone()), |c, is_candidate| {
            if is_candidate {
                !matches!(c.id.as_str(), "p1") && c.should_hit
            } else {
                baseline(c, false)
            }
        });
        assert_eq!(accepted.verdict, Verdict::Accepted, "{:?}", accepted.reasons);
        assert_eq!(accepted.delta, 1.0);
        assert_eq!(accepted.improved, 1);
        assert_eq!(accepted.regressed, 0);

        // 完全没有改善：delta=0，且 improved=0。
        let flat = evaluate_paired(&set(1.0, cases.clone()), |_, _| false);
        assert_eq!(flat.verdict, Verdict::Rejected);
        assert!(flat.reasons.iter().any(|r| r.contains("净收益")), "{:?}", flat.reasons);
        assert!(flat.reasons.iter().any(|r| r.contains("由判错变为判对")));

        // 收益达标但保留集退化：夹具让基线只对 p3/p4/n1/n2，候选修好 p1/p2
        // 却把 protected 的 n1 误召回。净收益 +1 达标，硬不变量仍须直接否决。
        let regressed = evaluate_paired(&set(1.0, cases.clone()), |c, is_candidate| {
            let hit = if is_candidate {
                c.should_hit || c.id == "n1"
            } else {
                c.id != "p1" && c.id != "p2" && c.should_hit
            };
            hit
        });
        assert_eq!(regressed.verdict, Verdict::Rejected, "{:?}", regressed.reasons);
        assert!(regressed.delta >= 1.0, "夹具必须让净收益达标，否则没测到硬不变量：{:?}", regressed.reasons);
        assert_eq!(regressed.protected_regressions, vec!["n1".to_string()]);
        assert_eq!(regressed.new_false_positives, vec!["n1".to_string()]);
        assert!(regressed.reasons.iter().any(|r| r.contains("保留集")));
    }

    #[test]
    fn unverifiable_is_distinct_from_rejected_and_feedback_is_desensitized() {
        let unverifiable = Evaluation::unverifiable("缺少选择集");
        assert_eq!(unverifiable.verdict, Verdict::Unverifiable);
        assert_eq!(unverifiable.case_count, 0);

        let cases = balanced();
        let rejected = evaluate_paired(&set(1.0, cases.clone()), |_, _| false);
        let value = rejected.to_value();
        let text = value.to_string();
        for secret in ["秘密题目", "should_hit", "query", "baseline_hit", "candidate_hit"] {
            assert!(!text.contains(secret), "反馈泄露了 {secret}: {text}");
        }
        assert_eq!(value["queries_disclosed"], json!(false));
        assert_eq!(value["verdict"], json!("rejected"));
        assert!(value["cases"].as_array().unwrap().len() == 6);
    }

    #[test]
    fn staging_roundtrips_hidden_candidates_and_blocks_path_escape() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path();
        let evaluation = Evaluation::unverifiable("缺少选择集");
        let content = "---\nname: demo\ndescription: d\n---\n\n候选正文\n";
        let id = save_candidate(skills, "demo", content, &evaluation).unwrap();
        assert!(skills.join(STAGING_DIR).join("demo").is_dir());
        assert!(id.starts_with("demo:"));

        let loaded = load_candidate(skills, &id).unwrap();
        assert_eq!(loaded.name, "demo");
        assert_eq!(loaded.content, content);
        assert_eq!(loaded.meta.verdict, "unverifiable");
        assert_eq!(list_candidates(skills, "demo").unwrap().len(), 1);

        for bad in [
            "../evil:1-a",
            "demo:../../x",
            "demo:.hidden",
            "demo:abc",
            "no-colon",
            "a/b:1-c",
        ] {
            assert!(load_candidate(skills, bad).is_err(), "{bad} 不该被接受");
        }
        assert!(save_candidate(skills, "../evil", content, &evaluation).is_err());
    }

    #[test]
    fn pinned_digest_blocks_session_local_tampering() {
        let raw = set_json(json!([]), json!(1.0)).to_string();
        let digest = selection_digest(&raw);
        assert!(verify_selection_set(&raw, Some(&digest)).is_ok());

        // 哪怕只追加一个空白字符，也必须让本次评估作废。
        let tampered = format!("{raw} ");
        let err = verify_selection_set(&tampered, Some(&digest)).unwrap_err();
        assert!(err.contains("被修改"), "{err}");

        // 启动时没有选择集：不能降级成「没验证就放行」。
        let err = verify_selection_set(&raw, None).unwrap_err();
        assert!(err.contains("重启应用"), "{err}");

        // 损坏文件不该被当成有效摘要。
        assert_ne!(selection_digest("not json"), digest);
    }

    #[test]
    fn staged_candidate_keeps_layout_and_resources_for_promote() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path();
        let evaluation = Evaluation::unverifiable("缺少选择集");
        let content = "---\nname: demo\ndescription: d\n---\n\n候选正文\n";
        let resources = vec![
            ("references/rules.md".to_string(), "细则".to_string()),
            ("../evil.md".to_string(), "越界资源必须被丢弃".to_string()),
        ];
        let id = save_candidate_with(skills, "demo", content, &evaluation, "flat", &resources)
            .unwrap();
        let loaded = load_candidate(skills, &id).unwrap();
        assert_eq!(loaded.layout, "flat", "promote 不能把布局偷偷改成 auto");
        assert_eq!(
            loaded.resources,
            vec![("references/rules.md".to_string(), "细则".to_string())]
        );

        // 目录布局同样要保留资源。
        let id2 = save_candidate_with(
            skills,
            "demo",
            content,
            &evaluation,
            "folder",
            &[("scripts/run.py".to_string(), "print(1)".to_string())],
        )
        .unwrap();
        let loaded2 = load_candidate(skills, &id2).unwrap();
        assert_eq!(loaded2.layout, "folder");
        assert_eq!(
            loaded2.resources,
            vec![("scripts/run.py".to_string(), "print(1)".to_string())]
        );

        // 旧候选（没有 layout/resources 字段）必须能兼容读回。
        let legacy = save_candidate(skills, "demo", content, &evaluation).unwrap();
        let loaded3 = load_candidate(skills, &legacy).unwrap();
        assert_eq!(loaded3.layout, "folder");
        assert!(loaded3.resources.is_empty());
    }
}
