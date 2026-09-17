//! 角色长期记忆笔记（memory.md）
//!
//! 每个角色在应用数据目录下维护一份 markdown 长期记忆笔记：
//! `%APPDATA%\Vivian\characters\<char_id>\memory\memory.md`（按角色隔离）。
//!
//! 与结构化记忆库（`unified_memory.json` + 向量检索）互补：
//! - 结构化记忆库负责「按查询检索记忆条目」；
//! - 这里负责「每轮全量注入上下文的相处约定」，只收四类内容：
//!   相处约定 / 角色许下的承诺 / 相处中的教训 / 只有彼此懂的梗。
//!   用户的事实与偏好不在这里（由 user_facts / user_model 承载）。
//!
//! 读写链路（与注入预算协同，保证注入侧永不截断）：
//! - **注入**：每轮由 PromptBuildingStep 全量读取（`read_memory_md`）；
//! - **沉淀**：反思步（reflection）在每轮回复后产出 `memory_note` 字段，
//!   经 `append_memory_md` 按日期分节追加——对话中零工具调用，不打断沉浸感；
//! - **预算硬不变量**：追加后若超过 [`MEMORY_MD_MAX_CHARS`]，优先驱逐最旧的
//!   日期分节（未整理的原始沉淀），仍超限才动已整理的正文分节——使文件长度
//!   始终 ≤ 上限，注入侧因此永远读到全文，不存在截断损失；
//! - **整理**：睡眠巩固窗口按 [`tidy_need`] 判定触发，分 `Incremental` 与 `FullCompaction` 两条路径；
//!   `memory_md` 工具仅作用户明说查看/整理时的手动入口。
//!
//! # 两区模型（增量整理的基础）
//!
//! 文件在语义上是两个区，靠**分节标题的形态**区分，无需额外的水位文件：
//!
//! ```text
//! # 长期记忆笔记
//! > 说明头……
//!
//! ## 相处约定          ← 已整理区：主题分节（非日期标题），注入价值最高
//! - 主人不吃香菜
//!
//! ## 2026-09-16 19:53  ← 待整理区：日期分节（`YYYY-MM-DD HH:MM`），原始沉淀
//! - 今天一起看了《夏日大作战》
//! ```
//!
//! 用标题形态而非 sidecar 水位文件判断，理由：
//! 1. 不引入第二份状态，避免水位与文件不同步；
//! 2. [`enforce_char_budget`] 驱逐旧分节不改变标题形态，判定稳定；
//! 3. 不依赖时间戳比较，同一分钟内的多次追加也能正确归类。
//!
//! 整理因此可以**增量**做：LLM 只读待整理区（通常几百字符），产出「需新增的
//! 条目」，再由 [`merge_entries`] 机械并入已整理区；只有当合并后逼近预算上限
//! 时才回落到全量压缩（LLM 读全文、按主题重排），把平铺的 `## 近期补充`
//! 重新归入主题分节。常见路径的输入规模因此从「全文」降到「新增部分」。

/// 注入 prompt 的最大字符数（写侧驱逐保证文件恒 ≤ 此值，注入侧正常永不触发截断；
/// 保留读侧尾部截断仅作手工编辑文件等旁路场景的安全网）。
pub const MEMORY_MD_MAX_CHARS: usize = 2000;

/// 增量整理触发阈值：**待整理区**非空行数达到该值就跑一次增量整理。
///
/// 阈值远低于全量压缩阈值：增量路径的 LLM 输入仅为新增沉淀，高频触发成本低，
/// 可维持文件长期整洁。
pub const MEMORY_MD_TIDY_PENDING_LINES: usize = 12;

/// 全量压缩触发阈值：总字符数超过该值时改跑全量压缩。
///
/// 取值贴近 [`MEMORY_MD_MAX_CHARS`]：留出余量让压缩有机会在驱逐发生前
/// 把正文精简下来（压缩目标 1500 字符）。
pub const MEMORY_MD_COMPACT_CHARS: usize = 1700;

/// 增量整理时新条目暂存的分节标题。
///
/// 增量产出不标主题（LLM 只看新增部分，无从判断全局主题分布），
/// 先平铺在这里；下次全量压缩时由 LLM 归并进主题分节后该标题自然消失。
pub const MEMORY_MD_RECENT_HEADING: &str = "## 近期补充";

/// memory.md 首次创建时写入的文件头（说明用途与维护方式）。
pub const MEMORY_MD_HEADER: &str = "# 长期记忆笔记\n\n\
    > 本文件随对话自动沉淀（只记相处约定、承诺、教训和梗），睡眠期自动整理合并。\n\
    > 每次对话全量注入上下文，是与主人相处的长期约定。\n";

/// 睡眠整理该走哪条路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TidyNeed {
    /// 不需要整理
    None,
    /// 增量整理：LLM 只读待整理区，产出新增条目后机械并入
    Incremental,
    /// 全量压缩：LLM 读全文按主题重排（文件逼近上限时的兜底）
    FullCompaction,
}

/// memory.md 的两区切分结果（文件头已剥离，两者都是可直接写回的正文片段）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MemoryMdRegions {
    /// 已整理区：主题分节（含 `## 近期补充`），不含文件头
    pub consolidated_body: String,
    /// 待整理区：日期分节（`YYYY-MM-DD HH:MM`），按追加顺序时间升序
    pub pending_body: String,
}

/// 角色长期记忆笔记文件路径：`<角色数据目录>/memory/memory.md`。
pub fn memory_md_path(char_id: &str) -> std::path::PathBuf {
    crate::utils::path::get_character_data_dir(char_id)
        .join("memory")
        .join("memory.md")
}

/// 读取 memory.md 全文（不做截断，重写/工具读写用）。空文件视为 None。
pub fn read_memory_md_raw(char_id: &str) -> Option<String> {
    if char_id.trim().is_empty() {
        return None;
    }
    let text = std::fs::read_to_string(memory_md_path(char_id)).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 读取 memory.md（注入用）。超长保留尾部——最新沉淀的条目。
///
/// 写侧驱逐保证正常运行时文件恒 ≤ [`MEMORY_MD_MAX_CHARS`]，此处的截断仅覆盖
/// 用户手工编辑文件等旁路场景，属安全网而非常规路径。
pub fn read_memory_md(char_id: &str) -> Option<String> {
    let full = read_memory_md_raw(char_id)?;
    let chars: Vec<char> = full.chars().collect();
    if chars.len() <= MEMORY_MD_MAX_CHARS {
        return Some(full);
    }
    let tail: String = chars[chars.len() - MEMORY_MD_MAX_CHARS..].iter().collect();
    Some(format!("（较早内容已截断）\n{tail}"))
}

/// 判断文件当前该走哪条整理路径（读文件后转 [`tidy_need_for_text`]）。
pub fn tidy_need(char_id: &str) -> TidyNeed {
    match read_memory_md_raw(char_id) {
        Some(text) => tidy_need_for_text(&text),
        None => TidyNeed::None,
    }
}

/// 判定逻辑的纯函数形式（便于单测，不碰文件系统）。
///
/// 优先级：逼近上限 → 全量压缩；否则待整理区够多 → 增量整理；都不满足 → 不动。
pub fn tidy_need_for_text(text: &str) -> TidyNeed {
    if text.chars().count() > MEMORY_MD_COMPACT_CHARS {
        return TidyNeed::FullCompaction;
    }
    let regions = split_regions(text);
    let pending_lines = regions
        .pending_body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    if pending_lines >= MEMORY_MD_TIDY_PENDING_LINES {
        TidyNeed::Incremental
    } else {
        TidyNeed::None
    }
}

/// 组装重写全文（文件头 + 正文）并校验预算。
///
/// 重写结果超过 [`MEMORY_MD_MAX_CHARS`] 时**拒绝**而非静默截断——
/// 全文重写的正文结构由调用方组织，截断会破坏其语义；调用方应精简后重试。
fn compose_memory_md(body: &str) -> Result<String, String> {
    let mut content = MEMORY_MD_HEADER.to_string();
    let body = body.trim();
    if !body.is_empty() {
        content.push('\n');
        content.push_str(body);
        content.push('\n');
    }
    let count = content.chars().count();
    if count > MEMORY_MD_MAX_CHARS {
        return Err(format!(
            "重写后的笔记为 {count} 字符，超过上限 {MEMORY_MD_MAX_CHARS}，请合并去重后精简重试"
        ));
    }
    Ok(content)
}

/// 全量重写 memory.md（文件头 + 正文）。
///
/// 这是**唯一**的落盘入口：增量整理产出的合并结果同样走这里写回，
/// 因此预算校验与文件头格式只有一处实现。
pub fn write_memory_md(char_id: &str, body: &str) -> Result<(), String> {
    if char_id.trim().is_empty() {
        return Err("缺少角色 ID".into());
    }
    let content = compose_memory_md(body)?;
    let path = memory_md_path(char_id);
    if let Some(parent) = path.parent() {
        crate::utils::path::ensure_dir(parent).map_err(|e| format!("创建记忆目录失败：{e}"))?;
    }
    crate::utils::fs::write_atomic(&path, &content)
        .map_err(|e| format!("写入 memory.md 失败：{e}"))
}

/// 追加一段内容到 memory.md（不存在则带说明头创建，按日期分节）。
///
/// 追加的分节标题是 `YYYY-MM-DD HH:MM`，天然落在**待整理区**，
/// 下次睡眠整理时被增量并入正文。
///
/// 追加后若超过 [`MEMORY_MD_MAX_CHARS`]，优先从**最旧的日期分节**起整节驱逐，
/// 仍超限才驱逐最旧的正文分节。先驱逐日期分节的依据是信息价值而非时序：
/// 已整理的正文是蒸馏后的长期知识，日期分节只是尚未蒸馏的原始素材，
/// 后者可丢、前者不可丢。
pub fn append_memory_md(char_id: &str, body: &str) -> Result<(), String> {
    if char_id.trim().is_empty() {
        return Err("缺少角色 ID".into());
    }
    let body = body.trim();
    if body.is_empty() {
        return Ok(());
    }
    let path = memory_md_path(char_id);
    if let Some(parent) = path.parent() {
        crate::utils::path::ensure_dir(parent).map_err(|e| format!("创建记忆目录失败：{e}"))?;
    }
    let mut content = std::fs::read_to_string(&path).unwrap_or_default();
    if content.trim().is_empty() {
        content = MEMORY_MD_HEADER.to_string();
    }
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&format!(
        "\n## {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M")
    ));
    content.push_str(body);
    content.push('\n');

    let (evicted, content) = enforce_char_budget(content);
    if evicted > 0 {
        tracing::info!(
            "[memory_md] 追加后超上限，已驱逐最旧的 {evicted} 个分节（优先驱逐未整理的日期分节）"
        );
    }
    crate::utils::fs::write_atomic(&path, &content)
        .map_err(|e| format!("写入 memory.md 失败：{e}"))
}

/// 把文件切成「已整理正文」与「待整理沉淀」两区（文件头剥离）。
///
/// 分节标题形如 `YYYY-MM-DD HH:MM`（或 ISO 的 `T` 分隔形式）归入待整理区，
/// 其余标题（相处约定 / 承诺 / 教训 / 梗 / 近期补充 …）归入已整理区。
/// 无任何分节标题时，整段正文视为已整理区（`write_memory_md` 的产物）。
pub fn split_regions(content: &str) -> MemoryMdRegions {
    const MARKER: &str = "\n## ";
    let body = strip_file_header(content);
    // 正文直接以 `## ` 开头时补一个换行，统一走 MARKER 切分
    let normalized = if body.starts_with("## ") {
        format!("\n{body}")
    } else {
        body.to_string()
    };

    let (head, rest) = match normalized.find(MARKER) {
        Some(idx) => normalized.split_at(idx),
        None => (normalized.as_str(), ""),
    };

    let mut consolidated = String::new();
    if !head.trim().is_empty() {
        consolidated.push_str(head.trim());
        consolidated.push('\n');
    }
    let mut pending = String::new();
    for chunk in rest.split(MARKER) {
        if chunk.trim().is_empty() {
            continue;
        }
        let header = chunk.lines().next().unwrap_or("");
        let section = format!("{MARKER}{chunk}");
        if is_date_header(header) {
            pending.push_str(&section);
        } else {
            consolidated.push_str(&section);
        }
    }

    MemoryMdRegions {
        // 两区都做**两端** trim：分节拼接时每节都带前导 `\n`，只 trim_end 会让
        // consolidated_body 以换行开头，破坏 `starts_with("## ...")` 这类判断
        consolidated_body: consolidated.trim().to_string(),
        pending_body: pending.trim().to_string(),
    }
}

/// 增量并入：把 LLM 产出的条目合并进已整理正文。
///
/// 逐行处理 `entries`：
/// - `REPLACE: 原条目 => 新条目`：在正文里找规范化后等价的行就地改写；
///   找不到旧条目时**退化为新增**（宁可多一条，也不静默丢信息）；
/// - 其余行：剥掉列表符号后与正文（及本次已收条目）做规范化去重，重复则跳过。
///
/// 新条目落在 [`MEMORY_MD_RECENT_HEADING`] 分节末尾（不存在则新建），
/// 保持时间顺序；下次全量压缩时由 LLM 归入主题分节。
pub fn merge_entries(consolidated_body: &str, entries: &str) -> String {
    let body = consolidated_body.trim_end();
    let mut lines: Vec<String> = body.lines().map(|l| l.to_string()).collect();

    let mut known: std::collections::HashSet<String> = lines
        .iter()
        .map(|l| normalize_line(l))
        .filter(|s| !s.is_empty())
        .collect();

    let mut additions: Vec<String> = Vec::new();
    // 是否发生过就地改写。有改写时**不能**走 `additions.is_empty()` 的早退分支——
    // 那条分支返回的是原始 `body`，会丢弃已完成的 REPLACE 改写结果。
    let mut replaced_any = false;

    for raw in entries.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((old, new)) = parse_replace(line) {
            let target = normalize_line(&old);
            let mut replaced = false;
            if !target.is_empty() {
                for l in lines.iter_mut() {
                    if normalize_line(l) == target {
                        *l = format!("- {}", new.trim());
                        replaced = true;
                        break;
                    }
                }
            }
            if replaced {
                replaced_any = true;
                continue;
            }
            push_unique(&mut additions, &mut known, &new);
            continue;
        }
        push_unique(&mut additions, &mut known, line);
    }

    if additions.is_empty() {
        return if replaced_any {
            lines.join("\n").trim_end().to_string()
        } else {
            body.to_string()
        };
    }

    match lines.iter().position(|l| l.trim() == MEMORY_MD_RECENT_HEADING) {
        Some(idx) => {
            // 插到该分节末尾（下一个标题行之前），保持追加顺序
            let end = lines
                .iter()
                .enumerate()
                .skip(idx + 1)
                .find(|(_, l)| l.trim_start().starts_with('#'))
                .map(|(i, _)| i)
                .unwrap_or(lines.len());
            let prev_blank = end > 0 && lines.get(end - 1).is_some_and(|l| l.trim().is_empty());
            let mut block: Vec<String> = Vec::with_capacity(additions.len() + 2);
            if !prev_blank {
                block.push(String::new());
            }
            block.extend(additions.iter().map(|a| format!("- {a}")));
            // 结尾留空行，避免紧跟其后的分节标题被粘在列表项后面
            block.push(String::new());
            for (offset, l) in block.into_iter().enumerate() {
                lines.insert(end + offset, l);
            }
        }
        None => {
            lines.push(String::new());
            lines.push(MEMORY_MD_RECENT_HEADING.to_string());
            lines.push(String::new());
            lines.extend(additions.iter().map(|a| format!("- {a}")));
        }
    }

    lines.join("\n").trim_end().to_string()
}

/// 字符预算执行器：超限时优先驱逐最旧的日期分节，仍超限再驱逐最旧的正文分节。
///
/// 返回 (驱逐分节数, 收敛后的内容)。极端兜底（单个分节仍超限，仅当单次追加
/// 本身巨长时出现）保留最新分节的尾部并加截断标记。
fn enforce_char_budget(content: String) -> (usize, String) {
    if content.chars().count() <= MEMORY_MD_MAX_CHARS {
        return (0, content);
    }
    const MARKER: &str = "\n## ";
    let head_end = content.find(MARKER).unwrap_or(content.len());
    let (head, rest) = content.split_at(head_end);
    let mut sections: Vec<(bool, String)> = rest
        .split(MARKER)
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            let is_date = is_date_header(s.lines().next().unwrap_or(""));
            (is_date, format!("{MARKER}{s}"))
        })
        .collect();

    let head_len = head.chars().count();
    let body_len = |secs: &[(bool, String)]| -> usize {
        secs.iter().map(|(_, s)| s.chars().count()).sum()
    };
    let mut evicted = 0;

    // 第一轮：驱逐最旧的日期分节（未整理的原始沉淀，可丢）
    loop {
        if head_len + body_len(&sections) <= MEMORY_MD_MAX_CHARS || sections.len() <= 1 {
            break;
        }
        match sections.iter().position(|(is_date, _)| *is_date) {
            Some(pos) => {
                sections.remove(pos);
                evicted += 1;
            }
            None => break,
        }
    }
    // 第二轮：仍超限则从最旧的正文分节起驱逐（已整理知识，最后才动）
    loop {
        if head_len + body_len(&sections) <= MEMORY_MD_MAX_CHARS || sections.len() <= 1 {
            break;
        }
        sections.remove(0);
        evicted += 1;
    }

    let mut result = format!(
        "{head}{}",
        sections.iter().map(|(_, s)| s.as_str()).collect::<String>()
    );
    if result.chars().count() > MEMORY_MD_MAX_CHARS {
        // 单节仍超限：保尾部（最新内容），标记截断
        let chars: Vec<char> = result.chars().collect();
        let keep = MEMORY_MD_MAX_CHARS.saturating_sub(20);
        let tail: String = chars[chars.len() - keep..].iter().collect();
        result = format!("{head}\n## （更早内容因超长被截断）\n{tail}");
        if evicted == 0 {
            evicted = 1;
        }
    }
    (evicted, result)
}

/// 剥掉文件头（一级标题 `# xxx` 行 + 紧随其后的 `>` 引用说明行）。
///
/// 只认**一级**标题：`## 相处约定` 这类分节标题必须原样保留，否则整个
/// 已整理区会被当成文件头吃掉。用户手工编辑后标题可能不同，因此按形态
/// 剥离而不是按 [`MEMORY_MD_HEADER`] 常量精确匹配。
fn strip_file_header(content: &str) -> &str {
    let t = content.trim_start();
    let first_line = t.lines().next().unwrap_or("");
    if !(first_line.starts_with("# ") || first_line.trim_end() == "#") {
        return t;
    }
    let Some(pos) = t.find('\n') else {
        return "";
    };
    let mut rest = &t[pos + 1..];
    loop {
        let trimmed = rest.trim_start_matches('\n');
        if trimmed.starts_with('>') {
            match trimmed.find('\n') {
                Some(p) => rest = &trimmed[p + 1..],
                None => return "",
            }
        } else {
            return trimmed;
        }
    }
}

/// 分节标题是否为自动追加的日期分节（`YYYY-MM-DD HH:MM`，也接受 ISO 的 `T`）。
///
/// 要求前 16 字节匹配日期时间形态**且其后无内容**——`2026-09-16 19:53 的对话`
/// 这类用户手写标题不算日期分节，避免把主题分节误判成原始沉淀。
fn is_date_header(header: &str) -> bool {
    let t = header.trim();
    let b = t.as_bytes();
    if b.len() < 16 {
        return false;
    }
    let d = |i: usize| b.get(i).map(|c| c.is_ascii_digit()).unwrap_or(false);
    let pattern = d(0)
        && d(1)
        && d(2)
        && d(3)
        && b[4] == b'-'
        && d(5)
        && d(6)
        && b[7] == b'-'
        && d(8)
        && d(9)
        && (b[10] == b' ' || b[10] == b'T')
        && d(11)
        && d(12)
        && b[13] == b':'
        && d(14)
        && d(15);
    // 前 16 字节若匹配则全是 ASCII，故字节 16 一定是字符边界，切片安全
    pattern && (b.len() == 16 || t[16..].trim().is_empty())
}

/// 解析 `REPLACE: 原条目 => 新条目`（也接受 `UPDATE:`）。
fn parse_replace(line: &str) -> Option<(String, String)> {
    let prefix = line.get(..8)?.to_ascii_uppercase();
    if !(prefix.starts_with("REPLACE:") || prefix.starts_with("UPDATE:")) {
        return None;
    }
    let rest = line.get(8..)?.trim();
    let (old, new) = rest.split_once("=>")?;
    let (old, new) = (old.trim(), new.trim());
    if old.is_empty() || new.is_empty() {
        return None;
    }
    Some((old.to_string(), new.to_string()))
}

/// 去重键：剥列表符号 + 折叠空白 + 去掉句末标点。
///
/// 刻意**只做**这三点而不做更激进的归一（如同义词/词序）：机械去重的误合并
/// 会丢信息，而漏合并只是让重复条目多活一轮，下次全量压缩会清掉。
fn normalize_line(line: &str) -> String {
    let stripped = strip_bullet(line);
    let mut collapsed = String::with_capacity(stripped.len());
    let mut last_space = false;
    for ch in stripped.chars() {
        if ch.is_whitespace() {
            if !last_space {
                collapsed.push(' ');
                last_space = true;
            }
        } else {
            collapsed.push(ch);
            last_space = false;
        }
    }
    collapsed
        .trim()
        .trim_end_matches(|c: char| matches!(c, '。' | '.' | '，' | ',' | '；' | ';' | '！' | '!'))
        .trim()
        .to_string()
}

/// 剥掉行首的 markdown 列表符号。
fn strip_bullet(line: &str) -> &str {
    let t = line.trim();
    for marker in ["- ", "* ", "+ ", "• ", "· "] {
        if let Some(rest) = t.strip_prefix(marker) {
            return rest.trim();
        }
    }
    match t {
        "-" | "*" | "+" => "",
        _ => t,
    }
}

/// 收下一条新条目（规范化后与已知集合比对，重复则跳过）。
fn push_unique(
    additions: &mut Vec<String>,
    known: &mut std::collections::HashSet<String>,
    raw: &str,
) {
    let item = strip_bullet(raw.trim());
    if item.is_empty() {
        return;
    }
    let key = normalize_line(item);
    if key.is_empty() || !known.insert(key) {
        return;
    }
    additions.push(item.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_file() -> String {
        format!(
            "{}\n## 相处约定\n- 主人不吃香菜\n- 周五晚上一起看番\n\n\
             ## 2026-09-14 21:03\n- 今天聊到了大学的事\n\n\
             ## 2026-09-16 19:53\n- 答应了下周陪她看《夏日大作战》\n",
            MEMORY_MD_HEADER
        )
    }

    #[test]
    fn split_regions_separates_dated_sediment_from_theme_sections() {
        let r = split_regions(&sample_file());
        assert!(r.consolidated_body.starts_with("## 相处约定"));
        assert!(r.consolidated_body.contains("主人不吃香菜"));
        assert!(!r.consolidated_body.contains("2026-09-14"));
        assert!(r.pending_body.starts_with("## 2026-09-14 21:03"));
        assert!(r.pending_body.contains("夏日大作战"));
        assert!(!r.pending_body.contains("相处约定"));
        // 文件头不落在任何一区（写回时由 compose_memory_md 重新加）
        assert!(!r.consolidated_body.contains("# 长期记忆笔记"));
        assert!(!r.consolidated_body.contains("> 本文件随对话自动沉淀"));
    }

    #[test]
    fn split_regions_handles_file_without_sections() {
        let r = split_regions(&format!("{}\n## 承诺\n- 每天说晚安\n", MEMORY_MD_HEADER));
        assert_eq!(r.pending_body, "");
        assert!(r.consolidated_body.contains("每天说晚安"));

        let empty = split_regions(MEMORY_MD_HEADER);
        assert_eq!(empty.consolidated_body, "");
        assert_eq!(empty.pending_body, "");
    }

    #[test]
    fn tidy_need_picks_incremental_only_when_sediment_accumulates() {
        // 少量沉淀：不动
        assert_eq!(tidy_need_for_text(&sample_file()), TidyNeed::None);

        // 沉淀攒够行数：增量
        let mut text = sample_file();
        for i in 0..6 {
            text.push_str(&format!("\n## 2026-09-16 2{i}:00\n- 沉淀条目 {i}\n"));
        }
        assert_eq!(tidy_need_for_text(&text), TidyNeed::Incremental);

        // 逼近上限：全量压缩优先
        let mut big = sample_file();
        big.push_str(&"\n## 2026-09-16 23:00\n- ".to_string());
        big.push_str(&"很长的内容".repeat(400));
        assert_eq!(tidy_need_for_text(&big), TidyNeed::FullCompaction);
    }

    #[test]
    fn merge_entries_appends_new_items_under_recent_heading() {
        let body = "## 相处约定\n- 主人不吃香菜";
        let merged = merge_entries(body, "- 主人养了一只叫团子的猫\n- 主人不吃香菜\n");
        assert!(merged.contains("## 近期补充"));
        assert!(merged.contains("主人养了一只叫团子的猫"));
        // 与已有条目重复 → 不新增
        assert_eq!(merged.matches("主人不吃香菜").count(), 1);

        // 再跑一次：并入同一分节，不重复建标题
        let merged2 = merge_entries(&merged, "- 主人讨厌下雨天");
        assert_eq!(merged2.matches(MEMORY_MD_RECENT_HEADING).count(), 1);
        assert!(merged2.contains("主人讨厌下雨天"));
        assert!(merged2.contains("主人养了一只叫团子的猫"));
    }

    #[test]
    fn merge_entries_normalizes_bullets_whitespace_and_trailing_punctuation() {
        let body = "## 教训\n- 别在她累的时候追问工作";
        let merged = merge_entries(body, "* 别在她累的时候追问工作。\n");
        assert_eq!(merged, body, "等价条目应被机械去重：{merged}");
    }

    #[test]
    fn merge_entries_applies_replace_and_degrades_to_append() {
        let body = "## 相处约定\n- 主人不喝咖啡";
        let merged = merge_entries(body, "REPLACE: 主人不喝咖啡 => 主人现在改喝无咖啡因拿铁了");
        assert!(merged.contains("- 主人现在改喝无咖啡因拿铁了"));
        assert!(!merged.contains("主人不喝咖啡"));

        // 找不到旧条目 → 退化为新增而不是丢弃
        let degraded = merge_entries(body, "REPLACE: 主人从没提过的事 => 新条目");
        assert!(degraded.contains("主人不喝咖啡"));
        assert!(degraded.contains("新条目"));
    }

    #[test]
    fn merge_entries_keeps_sections_after_recent_heading_intact() {
        let body = "## 相处约定\n- 甲\n\n## 近期补充\n\n- 乙\n\n## 教训\n- 丙";
        let merged = merge_entries(body, "- 丁");
        assert!(merged.contains("- 丁"));
        // 丁 落在 `## 近期补充` 分节内，没有跑到 `## 教训` 之后
        let recent_pos = merged.find(MEMORY_MD_RECENT_HEADING).unwrap();
        let lesson_pos = merged.find("## 教训").unwrap();
        let new_pos = merged.find("- 丁").unwrap();
        assert!(recent_pos < new_pos && new_pos < lesson_pos, "{merged}");
        assert!(merged.contains("- 丙"));
    }

    #[test]
    fn merge_entries_is_noop_on_empty_entries() {
        let body = "## 相处约定\n- 甲";
        assert_eq!(merge_entries(body, ""), body);
        assert_eq!(merge_entries(body, "\n\n# 标题\n"), body);
    }

    #[test]
    fn merge_then_split_round_trips_into_consolidated_region() {
        // 增量整理后，新增内容必须落在「已整理区」，否则下一轮会被重复整理
        let file = sample_file();
        let regions = split_regions(&file);
        let merged = merge_entries(&regions.consolidated_body, "- 新条目");
        let reassembled = format!("{}\n{merged}\n", MEMORY_MD_HEADER);
        let again = split_regions(&reassembled);
        assert_eq!(again.pending_body, "", "增量整理后待整理区应清空");
        assert!(again.consolidated_body.contains("新条目"));
    }

    #[test]
    fn enforce_budget_evicts_dated_sections_before_consolidated_ones() {
        let mut content = String::from(MEMORY_MD_HEADER);
        content.push_str("\n## 相处约定\n- 重要的长期约定");
        content.push_str(&"很长的内容".repeat(60));
        for i in 0..6 {
            content.push_str(&format!("\n## 2026-09-0{i} 10:00\n- 原始沉淀 {i} "));
            content.push_str(&"很长的内容".repeat(60));
        }
        assert!(
            content.chars().count() > MEMORY_MD_MAX_CHARS,
            "夹具本身必须超限，否则这个用例什么也没测"
        );
        let (evicted, out) = enforce_char_budget(content);
        assert!(evicted > 0);
        assert!(out.chars().count() <= MEMORY_MD_MAX_CHARS);
        // 已整理的正文分节必须活着——它比原始沉淀值钱
        assert!(out.contains("重要的长期约定"), "正文分节被误驱逐：{out}");
        // 最旧的日期分节先走
        assert!(!out.contains("原始沉淀 0"));
        assert!(out.contains("原始沉淀 5"));
        assert!(out.starts_with("# 长期记忆笔记"));
    }

    #[test]
    fn enforce_budget_falls_back_to_evicting_consolidated_when_only_dates_are_gone() {
        let mut content = String::from(MEMORY_MD_HEADER);
        // 分节必须够大才越得过上限：6 节 × ~415 字符 ≈ 2490，加上文件头稳超 2000。
        // （早先写 60 次重复时总量只有 ~1963，压根没触发驱逐，断言 `evicted > 0` 就假失败了。）
        for i in 0..6 {
            content.push_str(&format!("\n## 主题{i}\n- 条目 {i} "));
            content.push_str(&"很长的内容".repeat(80));
        }
        assert!(
            content.chars().count() > MEMORY_MD_MAX_CHARS,
            "夹具本身必须超限，否则这个用例什么也没测"
        );
        let (evicted, out) = enforce_char_budget(content);
        assert!(evicted > 0);
        assert!(out.chars().count() <= MEMORY_MD_MAX_CHARS);
        assert!(!out.contains("条目 0"));
        assert!(out.contains("条目 5"));
    }

    #[test]
    fn enforce_budget_noop_within_limit() {
        let content = String::from("# 长期记忆笔记\n\n## 2026-09-01\n- 短条目\n");
        let (evicted, out) = enforce_char_budget(content.clone());
        assert_eq!(evicted, 0);
        assert_eq!(out, content);
    }

    #[test]
    fn write_compose_rejects_over_budget() {
        let body = "长".repeat(MEMORY_MD_MAX_CHARS);
        let err = compose_memory_md(&body).unwrap_err();
        assert!(err.contains("超过上限"));
        assert!(compose_memory_md("## 约定\n- 周五一起看番").is_ok());
    }

    #[test]
    fn date_header_detection_is_strict_enough() {
        assert!(is_date_header("2026-09-16 19:53"));
        assert!(is_date_header("  2026-09-16T19:53  "));
        assert!(!is_date_header("2026-09-16"));
        assert!(!is_date_header("相处约定"));
        assert!(!is_date_header("2026-09-16 19:53 的对话"));
        assert!(!is_date_header(""));
    }

    #[test]
    fn strip_file_header_tolerates_manual_edits() {
        assert_eq!(strip_file_header("# 标题\n> 说明\n\n## 甲\n- 乙"), "## 甲\n- 乙");
        assert_eq!(strip_file_header("## 甲\n- 乙"), "## 甲\n- 乙");
        assert_eq!(strip_file_header("# 只有标题"), "");
        assert_eq!(strip_file_header(""), "");
    }
}
