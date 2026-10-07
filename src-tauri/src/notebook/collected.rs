//! 采集资料 → 笔记的反向同步。
//!
//! 采集链路（热梗采集 / 后台知识采集 / 分享链接抓取）经 `add_knowledge_document`
//! 写入知识库后，由本模块另存一份笔记，使采集内容在 NotebookPage 有正式归档位置，
//! 而不只是躺在记忆条目里。
//!
//! 与 `sync_notebook_to_knowledge`（笔记 → 知识库）方向相反，但共用同一条链路：
//! 知识条目是权威副本，笔记是它的可读归档。采集笔记因此是**只读**的——
//! 用户在 NotebookPage 编辑采集笔记不会回写知识条目，避免两个方向互相覆盖。
//!
//! 幂等：笔记 id 由知识条目 id 派生，重复同步原地覆盖，不会堆出重复笔记。
//!
//! **只对采集类来源生效。** `source = notebook` 是反方向同步的产物（笔记 → 知识库），
//! 若也归档成笔记就会自循环；`migration` 是历史数据搬迁，归档只会产生重复内容。
//! 两者由 [`should_archive`] 明确排除。

use super::{storage, Block, Cover, Layout, NoteBook, Palette};

/// 采集类笔记的 id 前缀，便于与用户手写笔记区分。
const COLLECTED_PREFIX: &str = "note_collected_";

/// 采集笔记的固定分类标签。
///
/// 采集笔记一律带这个标签，标记自动归档的资料；心智观察器的笔记页不展示这些归档。
/// 它是**筛选标记**而非展示标签——`renderer.rs` 会跳过采集笔记的页脚标签云，
/// 所以不会重新把内部枚举值摆到成品页面上。
pub const COLLECTED_TAG: &str = "知识采集";

/// 采集笔记的稳定 id：同一个知识条目始终映射到同一篇笔记。
///
/// 用知识条目 id 派生而非时间戳，是为了让「重新采集刷新」能原地更新同一篇笔记，
/// 而不是每刷新一次就多一篇。
pub fn collected_note_id(memory_id: &str) -> String {
    let sanitized: String = memory_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    format!("{COLLECTED_PREFIX}{sanitized}")
}

/// 该笔记是否为采集资料生成的笔记。
pub fn is_collected_note(note_id: &str) -> bool {
    note_id.starts_with(COLLECTED_PREFIX)
}

/// 该来源的知识条目是否需要归档为采集笔记。
///
/// - `notebook`：反方向同步（笔记 → 知识库）的产物，归档会自循环
/// - `migration`：历史数据搬迁，归档只会产生与原笔记重复的内容
///
/// 其余（`meme_acquisition` / `web` / `user_link` / `user_file` 等）都是真正的采集。
pub fn should_archive(source: &str) -> bool {
    !matches!(source.trim(), "notebook" | "migration")
}

/// 从知识条目的标题与正文生成一篇采集笔记。
///
/// 采集内容有高度规整的字段模式（实测：11 个条目，每条固定「来源/背景 / 用法 / 慎用」），
/// 因此走 [`compose_blocks`] 做结构化排版，而不是按空行切段塞纯文本。
///
/// 采集笔记只带一个[`COLLECTED_TAG`] 分类标签：来源与细分主题已在封面
/// （标题 + 中文副标题）里表达过一遍，页脚再挂一排 `meme_acquisition` 这类内部
/// 枚举值只是噪音。`COLLECTED_TAG` 只用于笔记页筛选，不进渲染。
pub fn build_collected_note(
    char_id: &str,
    memory_id: &str,
    title: &str,
    content: &str,
    source: &str,
) -> NoteBook {
    let now = chrono::Local::now().timestamp() as f64;
    let blocks = compose_blocks(content);

    // 采集内容几乎都带来源与日期，配封面把「这是什么、什么时候的」提到最前面。
    // Article 布局不渲染封面，所以这里用 CoverFlow。
    let cover = Cover {
        title: title.to_string(),
        subtitle: Some(cover_subtitle(source)),
        background: None,
    };

    NoteBook {
        id: collected_note_id(memory_id),
        title: title.to_string(),
        char_id: char_id.to_string(),
        created_at: now,
        updated_at: now,
        tags: vec![COLLECTED_TAG.to_string()],
        layout: Layout::CoverFlow,
        palette: Palette::Warm,
        // 采集笔记的排版由 compose_blocks 固定编排，不接受自定义 CSS
        custom_css: None,
        cover: Some(cover),
        blocks,
    }
}

/// 来源枚举值 → 用户可读的中文说明。成品笔记不该出现 `meme_acquisition` 这类
/// 内部枚举值（带下划线的技术词对用户无意义，且与 `meme` 这类真标签语义重复）。
/// 未登记的来源退化为「采集资料」，不把原始枚举值透传到页面上。
fn source_label(source: &str) -> &'static str {
    match source.trim() {
        "meme_acquisition" => "网络热梗采集",
        "web" => "后台知识采集",
        "user_link" => "分享链接",
        "user_file" => "分享文件",
        _ => "采集资料",
    }
}

/// 封面副标题：走来源说明（中文），不取导语。
///
/// 导语（数据来源与可靠性交代）现在是**不渲染的 Meta 块**——它是给检索链路看的，
/// 不是给读者看的。副标题是页面上读者唯一会看到的来源信息，所以用简短的中文来源
/// 标签（「网络热梗采集」），而不是截断后的导语长句。
fn cover_subtitle(source: &str) -> String {
    source_label(source).to_string()
}

/// 一个采集条目：标题 + 有序字段（用法/慎用等）+ 未归类的补充行。
struct CollectedItem {
    title: String,
    fields: Vec<(String, String)>,
    /// 不是已知字段标签的行（自由文本 / 未知标签的冒号行）。
    /// 单独收集而非塞进上一个字段，避免「他说：这样」被错误地并进「用法」的值里。
    extras: Vec<String>,
}

/// 识别采集内容里的编号条目。
///
/// 形态：每个条目以「数字 + . + 空白」开头，条目内部是「标签：值」的行，
/// 值可换行续写。编号之前的文字是导语，由 [`leading_paragraph`] 单独取。
fn parse_items(content: &str) -> Vec<CollectedItem> {
    let mut items: Vec<CollectedItem> = Vec::new();
    let mut pending: Vec<(String, String)> = Vec::new();
    let mut extras: Vec<String> = Vec::new();
    let mut current_title: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = split_ordinal(line) {
            // 遇到新编号：先把上一条收尾
            if let Some(title) = current_title.take() {
                items.push(CollectedItem {
                    title,
                    fields: std::mem::take(&mut pending),
                    extras: std::mem::take(&mut extras),
                });
            }
            current_title = Some(rest.to_string());
            continue;
        }
        match current_title {
            // 编号条目内部：已知标签拆字段，其余按「续写」或「补充行」归位
            Some(_) => {
                if let Some((label, value)) = split_field(line) {
                    pending.push((label, value));
                    continue;
                }
                // 含冒号但标签未知（「他说：这样」）→ 补充行，不能并进上一个字段。
                // 不含冒号 → 可能是字段值续写，也可能是独立补充行，见下。
                let has_colon = line.contains('：') || line.contains(':');
                if has_colon && !starts_with_known_label(line) {
                    extras.push(line.to_string());
                    continue;
                }
                // 已有字段时视为该字段的换行续写，否则是独立补充行。
                // 真实采集里字段值本身就可能跨行（「来源/背景：第一行\n第二行续写」）。
                match pending.last_mut() {
                    Some(last) => {
                        last.1.push('\n');
                        last.1.push_str(line);
                    }
                    None => extras.push(line.to_string()),
                }
            }
            // 编号条目之前是导语，交由 leading_paragraph 处理，这里不重复收集
            None => {}
        }
    }
    if let Some(title) = current_title {
        items.push(CollectedItem {
            title,
            fields: pending,
            extras,
        });
    }
    items
}

/// 拆「1. 班味」→「班味」。非编号行返回 None。
fn split_ordinal(line: &str) -> Option<&str> {
    let digits: String = line.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = line[digits.len()..].trim_start();
    let rest = rest.strip_prefix('.')?.trim_start();
    (!rest.is_empty() && digits.parse::<u32>().is_ok()).then_some(rest)
}

/// 采集内容里已知的字段标签 → (展示顺序, 标记)。
///
/// 用**白名单**而不是「短且无句读就算字段」的字符集猜测：汉字的
/// `is_alphanumeric()` 为 true，「他说：这样」会被误判成字段。采集内容由固定
/// prompt 总结（实测「来源/背景 / 用法 / 慎用」各稳定出现 11 次），白名单既准确，
/// 又让排序与标记共用同一份定义。
/// 采集内容里已知的字段标签 → (展示顺序, 分组名)。
///
/// 用**白名单**而不是「短且无句读就算字段」的字符集猜测：汉字的
/// `is_alphanumeric()` 为 true，「他说：这样」会被误判成字段。采集内容由固定
/// prompt 总结（实测「来源/背景 / 用法 / 慎用」各稳定出现 11 次），白名单既准确，
/// 又让排序与分组共用同一份定义。
///
/// 分组名不用 emoji：笔记不使用 emoji 装饰，字段靠「标签：值」的中文标签本身
/// 表达语义，层次由Heading 与列表缩进承担。
const KNOWN_FIELDS: &[(&str, u8, &str)] = &[
    ("来源/背景", 0, "来源"),
    ("来源", 0, "来源"),
    ("背景", 0, "来源"),
    ("出处", 0, "来源"),
    ("用法", 1, "用法"),
    ("用法示例", 1, "用法"),
    ("示例", 1, "用法"),
    ("慎用", 2, "慎用"),
    ("注意", 2, "慎用"),
    ("避雷", 2, "慎用"),
    ("说明", 3, "说明"),
];

/// 字段名 → (展示顺序, 分组名)。未知标签返回 None，不参与结构化。
fn field_meta(label: &str) -> Option<(u8, &'static str)> {
    KNOWN_FIELDS
        .iter()
        .find(|(name, ..)| *name == label)
        .map(|(_, rank, group)| (*rank, *group))
}

/// 该行是否以「已知字段标签 + 冒号」开头。
///
/// 用于区分「字段值换行续写」与「一条新的补充行」：前者必然紧跟在某个已知字段之后。
fn starts_with_known_label(line: &str) -> bool {
    let Some((idx, _)) = line.char_indices().find(|(_, c)| *c == '：' || *c == ':') else {
        return false;
    };
    field_meta(line[..idx].trim()).is_some()
}

/// 拆「来源/背景：xxx」→ ("来源/背景", "xxx")。非已知字段行返回 None。
fn split_field(line: &str) -> Option<(String, String)> {
    // 全角「：」占 3 字节、半角「:」占 1 字节，必须用 char_indices 拿字符边界切，
    // 直接 find() 的字节偏移 +1 会切破全角冒号。
    let (idx, colon) = line.char_indices().find(|(_, c)| *c == '：' || *c == ':')?;
    let label = line[..idx].trim();
    let value = line[idx + colon.len_utf8()..].trim();
    if label.is_empty() || value.is_empty() {
        return None;
    }
    // 必须是已知字段标签，否则整行按正文续写处理
    field_meta(label)?;
    Some((label.to_string(), value.to_string()))
}

/// 把采集正文编排成结构化块。
///
/// 有编号条目时：导语→ Callout，条目 → Card（标题 + 带序号的字段行）。
/// 无编号条目时（自由形态采集结果）退化为按空行分段，保持可读。
fn compose_blocks(content: &str) -> Vec<Block> {
    let items = parse_items(content);
    if items.is_empty() {
        return content
            .split("\n\n")
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|paragraph| Block::Paragraph {
                text: paragraph.to_string(),
                style: None,
            })
            .collect();
    }

    let mut blocks = Vec::new();
    // 导语（来源可靠性、筛选结论等）存为 **Meta 块**：写进 note.json、能被RAG 召回，
    // 但**不渲染到页面**。它是对检索链路的交代，不是给读者看的叙事内容——
    // 渲染出来会变成一大段来源声明，把笔记的日记口吻打断。
    if let Some(lead) = leading_paragraph(content) {
        blocks.push(Block::Meta {
            key: "provenance".to_string(),
            text: lead,
        });
    }

    for (idx, item) in items.iter().enumerate() {
        // 条目标题：带序号的 Heading，让 11 个条目在长页里可扫读
        blocks.push(Block::Heading {
            text: format!("{}. {}", idx + 1, item.title),
            level: 3,
            style: None,
        });

        // 字段排序：来源 → 用法 → 慎用（parse_items 已保证都是已知字段）
        let mut fields: Vec<&(String, String)> = item.fields.iter().collect();
        fields.sort_by_key(|(label, _)| field_meta(label).map(|(r, _)| r).unwrap_or(9));

        // 用 List 而非 Card 正文：渲染器对list-item 有独立缩进与标记样式，
        // 多字段纵向对齐比挤在一个 card-body 里更易读。
        // 不写 Markdown 粗体——渲染器不解析 `**`，会原样显示星号。
        // 字段前缀用中文分组名（「来源」「用法」「慎用」）而非 emoji：
        // 笔记不使用 emoji 装饰，同一字段名归入同一组时前缀也一致。
        let mut body: Vec<String> = fields
            .iter()
            .map(|(label, value)| {
                let group = field_meta(label).map(|(_, g)| g).unwrap_or("其他");
                if group == label {
                    // 分组名与字段名相同（如「用法」「慎用」）时不重复前缀
                    format!("{label}：{value}")
                } else {
                    format!("{group}· {label}：{value}")
                }
            })
            .collect();
        // 未归类的补充行跟在字段后，同样以列表项呈现，不丢内容
        body.extend(item.extras.iter().cloned());

        if body.is_empty() {
            continue;
        }
        blocks.push(Block::List {
            items: body,
            ordered: false,
            style: None,
        });
    }
    blocks
}

/// 取第一个编号条目之前的导语文本（若有）。
fn leading_paragraph(content: &str) -> Option<String> {
    let first_ordinal = content.lines().find_map(|l| {
        let t = l.trim();
        split_ordinal(t).map(|_| t)
    })?;
    let lead: String = content
        .split_once(first_ordinal)?
        .0
        .trim()
        .to_string();
    (!lead.is_empty()).then_some(lead)
}

/// 采集笔记的「首次采集时间」：已有旧笔记则沿用其`created_at`，否则用本次时间。
///
/// TTL 资料（如 3 天有效的热梗）每次刷新都会被重建，若`created_at` 跟着刷新，
/// 页脚日期就成了「最后一次采集」—— 对「这资料有多新」这个问题是反向误导。
fn first_collected_at(char_id: &str, note_id: &str, fallback: f64) -> f64 {
    storage::load(char_id, note_id)
        .map(|old| old.created_at)
        .unwrap_or(fallback)
}

/// 把知识条目同步为采集笔记。返回笔记 id。
///
/// 失败只记日志返回 `None`：采集链路的主产物是知识条目，笔记归档是次要的，
/// 不该因为归档失败让整个采集任务回滚。
pub fn sync_knowledge_to_notebook(
    char_id: &str,
    memory_id: &str,
    title: &str,
    content: &str,
    source: &str,
) -> Option<String> {
    if content.trim().is_empty() {
        return None;
    }
    let mut note = build_collected_note(char_id, memory_id, title, content, source);
    let note_id = note.id.clone();

    // 重新采集时保留「首次采集时间」：TTL 资料每次刷新都会重建笔记，
    // 若跟着用当前时间，页脚日期就变成「最后一次采集」，与「这资料多新」语义相反。
    // 先读旧笔记再删，否则首次时间随旧目录一起消失。
    note.created_at = first_collected_at(char_id, &note_id, note.created_at);

    // 原地覆盖：先删旧笔记（含上一版 .html 与 .memory_ref）
    remove_collected_note(char_id, &note_id);

    match storage::save(&note) {
        Ok(()) => {
            // 反向关联：知识条目 id 写进 .memory_ref，删笔记时据此清知识条目
            if let Err(e) =
                std::fs::write(storage::note_memory_ref_path(char_id, &note_id), memory_id)
            {
                tracing::warn!("[CollectedNote] 写入 .memory_ref 失败: {e}");
            }
            tracing::info!(
                "[CollectedNote] 采集资料已归档为笔记「{}」（note_id={}）",
                title,
                note_id
            );
            Some(note_id)
        }
        Err(e) => {
            tracing::warn!("[CollectedNote] 采集资料归档为笔记失败: {e}");
            None
        }
    }
}

/// 删除采集笔记（知识条目过期/删除时调用，避免留下空壳笔记）。
///
/// 只删采集笔记：用户手写笔记即便被手动改成 `note_collected_` 前缀也不动，
/// 因为删除是自动链路，不该吃掉用户数据。
pub fn remove_collected_note(char_id: &str, note_id: &str) {
    if !is_collected_note(note_id) {
        return;
    }
    if let Err(e) = storage::delete(char_id, note_id) {
        tracing::warn!("[CollectedNote] 删除采集笔记失败: {e}");
    }
}

/// 采集资料的统一入口：写知识库 + 归档笔记。
///
/// 采集链路有五六个调用点（热梗采集 / 后台知识采集 / 分享链接 / 文件 / 远程），
/// 逐个手动两步容易漏，所以收口在这里。返回知识条目 id。
///
/// `ttl_days` 语义与 `MemoryManager::add_knowledge_document` 一致：
/// `Some(n>0)` 过期、`Some(-1)` 永不过期、`None` 不设过期。
///
/// `tags` 只进知识条目（供向量检索与过滤），**不传给笔记**——采集笔记不挂标签云。
pub async fn ingest_collected(
    char_id: &str,
    memory: &crate::memory::MemoryManager,
    title: &str,
    content: &str,
    tags: Vec<String>,
    source: &str,
    ttl_days: Option<i64>,
) -> Option<String> {
    let item = memory
        .add_knowledge_document(title, content, tags, source, ttl_days)
        .await
        .ok()?;

    if should_archive(source) {
        sync_knowledge_to_notebook(char_id, &item.id, title, content, source);
    }
    Some(item.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_id_is_stable_and_sanitized() {
        // 同一知识条目重复同步必须得到同一 id，否则每次刷新都堆新笔记
        assert_eq!(
            collected_note_id("mem_abc-123"),
            collected_note_id("mem_abc-123")
        );
        // 异常 id 不允许逃出目录名
        let id = collected_note_id("../../etc/passwd");
        assert!(!id.contains("..") && !id.contains('/'));
    }

    #[test]
    fn collected_notes_are_distinguishable_from_user_notes() {
        assert!(is_collected_note(&collected_note_id("mem_x")));
        assert!(!is_collected_note("note_1730000000000"));
    }

    #[test]
    fn empty_content_produces_no_blocks_and_no_note() {
        let note = build_collected_note("vivian", "mem_1", "标题", "   \n\n ", "web");
        assert!(note.blocks.is_empty());
        // 空白正文在同步入口就该被拒掉
        assert!(sync_knowledge_to_notebook("vivian", "mem_1", "t", "  ", "web").is_none());
    }

    /// 真实采集样本的形态（取自 entries.db 里的 douyin热梗速览，字段已脱敏截短）。
    const MEME_SAMPLE: &str = "整理自 2026-10-05 的搜索结果。有效信息来自两份聚合盘点，可靠性中等。\n\n\
1. 班味\n来源/背景：打工人语境里的稳定高频词。\n用法：自嘲「今天班味有点重」。\n慎用：用来形容别人等于扎人。\n\n\
2. 低山臭水遇知音\n来源/背景：对「高山流水遇知音」的反向玩梗。\n用法：熟人之间互相接梗。\n慎用：容易变成阴阳暗讽。";

    #[test]
    fn structured_items_become_heading_plus_field_list() {
        let note = build_collected_note(
            "vivian",
            "mem_2",
            "douyin热梗速览",
            MEME_SAMPLE,
            "meme_acquisition",
        );
        // 导语 Meta + 2 个(Heading + List) = 5 块
        assert_eq!(note.blocks.len(), 5, "实际: {:?}", note.blocks);

        // 导语进 Meta 块（存但不渲染），不是 Callout 也不是裸段落
        assert!(
            matches!(&note.blocks[0], Block::Meta { key, text } if key == "provenance" && text.contains("整理自")),
            "实际: {:?}",
            note.blocks[0]
        );
        // 条目是带序号的 Heading，长页里可扫读
        assert!(
            matches!(&note.blocks[1], Block::Heading { text, level: 3, .. } if text == "1. 班味")
        );
        // 字段进 List，且按 来源 → 用法 → 慎用 排序
        let fields = match &note.blocks[2] {
            Block::List { items, ordered, .. } => {
                assert!(!ordered);
                items.clone()
            }
            other => panic!("期望 List，实际: {other:?}"),
        };
        assert_eq!(fields.len(), 3);
        assert!(fields[0].contains("来源/背景"));
        assert!(fields[1].starts_with("用法") && fields[1].contains("用法"));
        assert!(fields[2].starts_with("慎用") && fields[2].contains("慎用"));
    }

    /// 来源说明存进 note.json（供 RAG 召回）但**不渲染**到页面。
    ///
    /// 这是「存在但不渲染」的核心约定：它要能被后续问答召回（否则存了等于没存），
    /// 又不能出现在读者眼前（会打断日记口吻的叙事感）。
    #[test]
    fn provenance_is_stored_but_not_rendered() {
        let note = build_collected_note(
            "vivian",
            "mem_prov",
            "douyin热梗速览",
            MEME_SAMPLE,
            "meme_acquisition",
        );
        // 1) 存在于块序列里
        let meta_text = note
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Meta { key, text } if key == "provenance" => Some(text.clone()),
                _ => None,
            })
            .expect("来源说明应存为 Meta 块");
        assert!(meta_text.contains("可靠性中等"), "实际: {meta_text}");

        // 2) 不出现在渲染后的 HTML 里
        let html = super::super::renderer::render_html(&note);
        assert!(
            !html.contains("可靠性中等"),
            "来源说明不该渲染到页面"
        );
        assert!(!html.contains("class=\"callout\""), "不该有 Callout 残留");
        // 3) 但条目正文照常渲染
        assert!(html.contains("1. 班味"), "正文应照常渲染");
    }

    #[test]
    fn collected_note_text_contains_no_emoji() {
        // 笔记不使用 emoji：字段前缀用中文分组名，封面无装饰图标。
        // 用码位区间扫描而非枚举具体字符——枚举式断言曾漏掉 ✦ 与 📅 两处。
        let note = build_collected_note(
            "vivian",
            "mem_no_emoji",
            "douyin热梗速览",
            MEME_SAMPLE,
            "meme_acquisition",
        );
        let html = super::super::renderer::render_html(&note);

        let mut found: Vec<char> = Vec::new();
        for ch in html.chars() {
            let c = ch as u32;
            let is_emoji = (0x1F300..=0x1FAFF).contains(&c)
                || (0x1F000..=0x1F2FF).contains(&c)
                || (0x2600..=0x27BF).contains(&c)
                || c == 0xFE0F
                || c == 0x2705
                || c == 0x274C;
            if is_emoji && !found.contains(&ch) {
                found.push(ch);
            }
        }
        assert!(found.is_empty(), "采集笔记渲染不应含 emoji，发现: {found:?}");

        // 字段仍以中文分组名前缀呈现
        let list = note
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::List { items, .. } => Some(items.clone()),
                _ => None,
            })
            .expect("应产出字段列表");
        assert!(list.iter().any(|i| i.starts_with("用法")));
    }

    #[test]
    fn collected_note_carries_only_the_category_tag() {
        // 采集笔记只带 COLLECTED_TAG（供笔记页筛选），不带任何其他标签
        let note = build_collected_note(
            "vivian",
            "mem_tags",
            "douyin热梗速览",
            MEME_SAMPLE,
            "meme_acquisition",
        );
        assert_eq!(note.tags, vec![COLLECTED_TAG.to_string()]);
        // 内部来源枚举值不该混进标签
        assert!(!note.tags.iter().any(|t| t.contains('_')));
    }

    #[test]
    fn collected_category_tag_is_not_rendered_in_footer() {
        // COLLECTED_TAG 只用于筛选：页脚不渲染标签云，
        // 否则会把筛选标记摆到成品页面上
        let note = build_collected_note(
            "vivian",
            "mem_tag_render",
            "douyin热梗速览",
            MEME_SAMPLE,
            "meme_acquisition",
        );
        let html = super::super::renderer::render_html(&note);
        // 注意不能 assert !html.contains("footer-tags")——那会命中 <style> 里的
        // CSS 规则而非 DOM 元素。定位到页脚后再检查有无标签元素。
        let footer_at = html.find("class=\"footer\"").expect("应渲染页脚");
        let rest = &html[footer_at..];
        let end = rest.find("</div>").unwrap_or(0);
        let footer = &rest[..end];
        assert!(
            !footer.contains("class=\"tag\""),
            "采集笔记页脚不该渲染标签: {footer}"
        );
        assert!(!footer.contains("class=\"footer-tags\""), "不该有标签云容器");
    }

    #[test]
    fn free_form_content_falls_back_to_paragraphs() {
        // 无编号结构的采集结果（如自由形态总结）不该被硬套结构，退回分段
        let note = build_collected_note(
            "vivian",
            "mem_4",
            "标题",
            "第一段。\n\n第二段。\n\n第三段。",
            "web",
        );
        assert_eq!(note.blocks.len(), 3);
        assert!(note.blocks.iter().all(|b| matches!(b, Block::Paragraph { .. })));
    }

    #[test]
    fn note_gets_cover_and_structured_layout() {
        let note = build_collected_note(
            "vivian",
            "mem_5",
            "douyin热梗速览",
            MEME_SAMPLE,
            "meme_acquisition",
        );
        // Article 布局不渲染封面，必须是 CoverFlow
        assert!(matches!(note.layout, Layout::CoverFlow));
        let cover = note.cover.expect("采集笔记应带封面");
        assert_eq!(cover.title, "douyin热梗速览");
        // 副标题是中文来源说明（导语已改为不渲染的 Meta 块）
        assert_eq!(cover.subtitle.as_deref(), Some("网络热梗采集"));
    }

    #[test]
    fn cover_subtitle_is_source_label_even_without_lead() {
        // 无导语时同样是来源说明（实现已不依赖导语）
        let note = build_collected_note("vivian", "mem_6", "t", "1. 梗\n用法：自嘲", "web");
        let cover = note.cover.expect("应带封面");
        assert_eq!(
            cover.subtitle.as_deref(),
            Some("后台知识采集"),
            "实际: {:?}",
            cover.subtitle
        );
    }

    #[test]
    fn unknown_label_lines_are_kept_as_extras_not_merged_into_fields() {
        // 「他说：这样」不是已知字段，不能被并进「用法」的值里
        let content = "1. 梗\n用法：自嘲\n他说：这样也行";
        let blocks = compose_blocks(content);
        let items = blocks
            .iter()
            .find_map(|b| match b {
                Block::List { items, .. } => Some(items.clone()),
                _ => None,
            })
            .expect("应产出字段列表");
        assert_eq!(items.len(), 2, "实际: {items:?}");
        assert!(items[0].contains("用法：自嘲"));
        // 补充行原样保留，不带字段标记
        assert_eq!(items[1], "他说：这样也行");
    }

    #[test]
    fn cover_subtitle_uses_source_label_not_the_lead() {
        // 副标题走来源说明而非导语：导语已改为不渲染的 Meta 块，
        // 截断后的导语长句不该再出现在封面上
        let long = "整理自某来源，可靠性中等。".repeat(40);
        let content = format!("{long}\n\n1. 梗\n用法：自嘲");
        let note = build_collected_note("vivian", "mem_7", "t", &content, "web");
        let sub = note.cover.unwrap().subtitle.unwrap();
        assert_eq!(sub, "后台知识采集");
        assert!(!sub.contains('\u{2026}'), "不该以省略号收尾: {sub}");
        // 导语仍在 Meta 块里（供检索），只是不参与封面
        assert!(
            note.blocks.iter().any(|b| matches!(b, Block::Meta { text, .. } if text.contains("可靠性中等"))),
            "导语应保留为 Meta 块"
        );
    }

    #[test]
    fn field_values_continuing_on_next_line_stay_in_the_same_field() {
        let content = "1. 梗\n来源/背景：第一行\n第二行续写。\n用法：自嘲";
        let blocks = compose_blocks(content);
        let fields = blocks
            .iter()
            .find_map(|b| match b {
                Block::List { items, .. } => Some(items.clone()),
                _ => None,
            })
            .expect("应产出字段列表");
        assert_eq!(fields.len(), 2);
        assert!(fields[0].contains('\n') && fields[0].contains("第二行续写"));
    }

    #[test]
    fn sentence_with_colon_is_not_mistaken_for_a_field() {
        // 标签必须短且无句读，否则「他说：这样」会被误判成字段
        assert!(split_field("他说：这样").is_none());
        assert!(split_field("来源/背景：打工人语境").is_some());
    }

    #[test]
    fn full_width_colon_is_split_on_char_boundary() {
        // 全角「：」是 3 字节；按字节 +1 切会切破字符（曾panic 在 char boundary）
        let (label, value) = split_field("来源/背景：打工人语境里的高频词").expect("应能拆出字段");
        assert_eq!(label, "来源/背景");
        assert_eq!(value, "打工人语境里的高频词");
        // 半角冒号同样支持
        let (label, value) = split_field("用法:自嘲").expect("半角冒号也应支持");
        assert_eq!(label, "用法");
        assert_eq!(value, "自嘲");
        // 值里含第二个冒号不应被截断（只拆第一个）
        let (_, value) = split_field("用法：他说：这样也行").expect("应能拆出");
        assert_eq!(value, "他说：这样也行");
    }

    #[test]
    fn only_whitelisted_labels_are_treated_as_fields() {
        // 白名单内的标签能拆出字段（含真实数据里出现过的「说明」）
        assert_eq!(split_field("说明：注意这里").map(|(l, _)| l), Some("说明".to_string()));
        // 不在白名单的标签 → 整行按正文处理，不拆字段
        assert!(split_field("他说：这样").is_none());
        assert!(!starts_with_known_label("他说：这样"));
        assert!(starts_with_known_label("用法：自嘲"));
        assert!(!starts_with_known_label("没有任何冒号的正文"));
    }

    #[test]
    fn blank_source_still_produces_a_valid_note() {
        // 来源为空也要能生成：封面退回通用文案，分类标签照常带上
        let note = build_collected_note("vivian", "mem_3", "t", "正文", "  ");
        assert_eq!(note.tags, vec![COLLECTED_TAG.to_string()]);
        let cover = note.cover.expect("仍应带封面");
        assert_eq!(cover.subtitle.as_deref(), Some("采集资料"));
    }

    #[test]
    fn internal_source_enum_never_reaches_user_facing_text() {
        // 成品笔记里不该出现 meme_acquisition 这类内部枚举值
        for (src, expected) in [
            ("meme_acquisition", "网络热梗采集"),
            ("web", "后台知识采集"),
            ("user_link", "分享链接"),
            ("user_file", "分享文件"),
        ] {
            assert_eq!(source_label(src), expected);
            // 无导语时副标题才用来源；这里直接验证映射本身
            let note = build_collected_note("vivian", "mem_src", "t", "1. 梗", src);
            let sub = note.cover.unwrap().subtitle.unwrap();
            assert_eq!(sub, expected);
            assert!(!sub.contains('_'), "副标题不该含内部枚举值: {sub}");
        }
    }

    #[test]
    fn unknown_source_falls_back_instead_of_leaking_enum() {
        // 未登记来源不能把原始值透传到界面
        assert_eq!(source_label("some_new_internal_enum"), "采集资料");
        assert_eq!(source_label(""), "采集资料");
    }

    #[test]
    fn collected_note_footer_says_collected_at() {
        // created_at 是「首次采集时间」，文案要说清是采集而非撰写
        let note = build_collected_note("vivian", "mem_footer", "t", "正文", "web");
        let html = super::super::renderer::render_html(&note);
        let footer_at = html
            .find("class=\"footer\"")
            .map(|i| {
                let rest = &html[i..];
                let end = rest.find("</div>").unwrap_or(0);
                rest[..end].to_string()
            })
            .expect("应渲染页脚");
        assert!(
            footer_at.contains("采集于"),
            "采集笔记页脚应写「采集于」，实际: {footer_at}"
        );
    }

    #[test]
    fn reverse_direction_and_migration_are_not_archived() {
        // notebook 是反方向同步的产物，归档会自循环
        assert!(!should_archive("notebook"));
        // migration 是历史搬迁，归档只会产生重复内容
        assert!(!should_archive("migration"));
        // 真正的采集来源都要归档
        for src in ["meme_acquisition", "web", "user_link", "user_file", "manual"] {
            assert!(should_archive(src), "{src} 应当归档");
        }
        assert!(!should_archive("  notebook  "));
    }
}
