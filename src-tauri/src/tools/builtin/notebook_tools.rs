//! 笔记本工具 - 智能体创建/修改/分享卡片风格 HTML 笔记
//!
//! 四个工具：
//! - create_notebook：创建新笔记（LLM 输出结构化 JSON，后端渲染 HTML）
//! - get_notebook_detail：读取已有笔记的完整结构化内容（修改前查看，避免改错）
//! - update_notebook：修改已有笔记（增量更新字段）
//! - share_notebook：以链接卡片形式分享到微信 ChatWindow
//!
//! 笔记创建/更新时会同步到向量知识库（`MemoryType::Knowledge`），
//! 供智能体后续通过 `search_memories` 进行 RAG 检索。

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::messages::{MessageMeta, MessageSource};
use crate::notebook::{storage, Block, Cover, Layout, NoteBook, Palette};
use crate::state::AppState;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};
use crate::types::response::ChatMessage as DialogChatMessage;

/// 全局 AppHandle
static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// 从 JSON Value 解析 blocks 数组
pub(crate) fn parse_blocks(value: &Value) -> Result<Vec<Block>, String> {
    let arr = value
        .as_array()
        .ok_or("blocks 必须是数组")?;
    let mut blocks = Vec::new();
    for item in arr {
        let block: Block = serde_json::from_value(item.clone())
            .map_err(|e| format!("解析内容块失败: {}", e))?;
        blocks.push(block);
    }
    Ok(blocks)
}

/// 从 JSON Value 解析封面
pub(crate) fn parse_cover(value: &Value) -> Result<Option<Cover>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let cover: Cover = serde_json::from_value(value.clone())
        .map_err(|e| format!("解析封面失败: {}", e))?;
    Ok(Some(cover))
}

/// 从字符串解析布局
pub(crate) fn parse_layout(s: &str) -> Layout {
    match s {
        "article" => Layout::Article,
        "gallery" => Layout::Gallery,
        "simple" => Layout::Simple,
        _ => Layout::CoverFlow,
    }
}

/// 从字符串解析配色
pub(crate) fn parse_palette(s: &str) -> Palette {
    match s {
        "fresh" => Palette::Fresh,
        "elegant" => Palette::Elegant,
        "cute" => Palette::Cute,
        "cool" => Palette::Cool,
        "nature" => Palette::Nature,
        _ => Palette::Warm,
    }
}

// ============================================================================
// CreateNotebookTool
// ============================================================================

pub struct CreateNotebookTool;

impl CreateNotebookTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CreateNotebookTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for CreateNotebookTool {
    fn name(&self) -> &str {
        "create_notebook"
    }

    fn description(&self) -> &str {
        "Create a beautiful Xiaohongshu-style HTML note page from collected information. You provide structured content (title, layout, palette, cover, content blocks), and the system renders it into a visually appealing HTML page saved locally. The page can be viewed in the Notebook tab of the memory window and shared to WeChat chat later. Proactively reach for this whenever you are doing data-organizing work — researching a topic, summarizing a travel guide, writing a how-to, compiling recommendations, structuring a report — so the user gets a polished visual artifact instead of a plain chat reply. Don't wait for the user to explicitly ask; if you find yourself gathering and structuring multiple pieces of information worth presenting, this is the natural way to deliver it.\n\nFollow a writing workflow, not just block assembly:\n0. Assess evidence dependence first — decide how much of what this note conveys comes from your own memory vs. the outside world. Timely news, other people's data, and specialized knowledge need a web_search; personal experience and opinion can come straight from memory. When you do search, collect and verify: prefer authoritative sources, cross-check key data across 2+ independent sources, present conflicts honestly instead of picking one side arbitrarily, and flag missing data rather than inventing numbers. Treat search results as material, not as conclusions. Trust P0/P1 sources for firm statements and for chart/table data; soften wording on single-source or low-authority claims (e.g. \"some say...\") rather than asserting them as established fact.\n1. Frame the note — decide its purpose (inform / record / educate / entertain), its reader, and the emotional tone, and keep the tone consistent with who you are.\n2. Choose the right form — pick layout and palette to match the content: a guide or structured data fits cover_flow with tags plus a table/chart; a long reflective piece or travelogue fits article; a collection of recommendations fits gallery; a short memo fits simple.\n3. Shape the content — open with a hook, develop in sections, close with a takeaway. Use heading for sections, paragraph for prose, card/quote/callout for emphasis, list for enumerations, and tags for keywords. Use block types sparingly rather than padding for variety — structure serves the content, not the other way around.\n4. Visualize when it helps — use a chart (bar/line/pie) when there are 3+ comparable data points, a table for discrete comparisons, and a Mermaid diagram for a process or timeline. Grouped data brought back from search fits a table or chart especially well. Every chart must be referenced and explained by the adjacent prose, never left floating; weave visuals into the flow, not as an afterthought.\n5. Write with care — avoid checklist jargon and empty superlatives; keep paragraphs 3-8 sentences with a clear flow; make headings accurate and functional; state a conclusion once and let other sections add new evidence rather than restating it. When you cite facts you looked up, say so naturally instead of pretending you already knew them; separate \"facts retrieved from search\" from \"your own inference\" and \"the user's own words\". Keep the voice natural and consistent with your personality."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "根据搜集到的信息制作漂亮的卡片风格 HTML 页面。你只需提供结构化内容（标题、布局、配色、封面、内容块），系统会自动渲染成视觉精美的 HTML 页面并保存在本地。页面可在笔记本窗口的'笔记'tab 中查阅，之后也可分享到微信聊天。当你在做资料整理类工作时——研究某个话题、总结旅行攻略、撰写步骤教程、整理好物推荐、结构化报告等——应主动用此工具，让用户得到一份精美的可视化产物而不是普通聊天回复。不必等用户明确要求；一旦你发现自己在搜集并组织多条值得呈现的信息，就该自然想到用这个方式交付。\n\n请遵循一套撰写工作流，而不是机械地堆砌内容块：\n0. 先判断信息依赖——这篇笔记要传达的事实，多少来自你的既有记忆、多少依赖外部世界？时效新闻、他人数据、专有知识需要检索；个人经历与观点从记忆取材即可。要检索时用 web_search 采集并核验：优先权威来源，关键数据交叉验证（≥2 个独立来源），来源冲突时如实呈现而非任意取舍，数据不足就明确标注缺漏、绝不编造数字。把检索结果当作素材而非结论。P0/P1 来源的数据可放心用于图表和肯定性表述；单来源或低权威的说法要降调措辞（如「有人提到…」），不要写成既定事实。\n1. 构思定位——先想清楚这篇笔记的目的（告知/记录/教学/娱乐）、读者和情绪基调，并与你自己的语气保持一致。\n2. 选对形式——根据内容选择布局与配色：攻略或结构化资料适合 cover_flow（封面+卡片流）并配标签和表格/图表；长文反思或游记适合 article（无封面）；好物推荐适合 gallery；简短备忘适合 simple。\n3. 组织内容——开头抓人、主体分节、结尾有收束。用 heading 分节、paragraph 写正文、card/quote/callout 做强调、list 做罗列、tags 做关键词。块类型按需选用，不为凑数堆砌——结构服务内容，不是为类型齐全而填满。\n4. 善用可视化——3 组以上可比数据用图表（柱状/折线/饼图）；离散数据对比优先用表格；流程或时序用 mermaid 流程图。检索带回的成组数据尤其适合用表格和图表呈现。每个图表都要被相邻正文引用并解释，不孤立放图；把可视化融入行文，而不是事后补图。\n5. 用心写作——避免清单式套话和空泛形容词；段落保持 3-8 句、逻辑连贯；标题准确功能化；一个结论只完整出现一次，其余部分补充新证据而非复述。检索来的事实要自然说明是查到的，不假装本来就知道；区分「搜索到的事实」「你的推断」和「用户原话」。保持语气自然，符合你的性格。",
            "ja" => "集めた情報から美しいカード風HTMLノートページを作成する。構造化コンテンツ（タイトル、レイアウト、配色、カバー、コンテンツブロック）を提供するだけで、システムが視覚的に美しいHTMLページにレンダリングしてローカルに保存する。ページはノートウィンドウの「ノート」タブで閲覧でき、後でWeChatチャットに共有することもできる。資料整理系の作業——テーマのリサーチ、旅行ガイドのまとめ、ハウツーの執筆、おすすめの整理、レポートの構造化など——を行う時は、普通のチャット返信の代わりに美しいビジュアル成果物をユーザーに届けるため、自らこのツールを使うこと。ユーザーに明示的に頼まれるのを待つ必要はない；複数の提示価値のある情報を集めて構造化している自分に気付いたら、自然にこの手段を選ぶ。\n\nブロックを機械的に並べるのではなく、次の書き方ワークフローに従ってください：\n0. まず情報依存を判断する——このノートが伝える事実のうち、どれだけが自分の記憶から来て、どれだけが外部世界に依存するか。時事ニュース、他人のデータ、専門知識は検索が必要；個人的な経験や意見は記憶から直接取材できる。検索する場合は web_search で収集・検証する：権威あるソースを優先し、重要データは 2 つ以上の独立ソースで相互検証し、矛盾があれば好き勝手に選ばず正直に提示し、データ不足なら欠けていることを明記し、数字を捏造しない。検索結果は結論ではなく素材として扱う。P0/P1 ソースのデータはチャートや断定表現に安心して使える；単一ソースや低権威の主張は「〜と言う人もいる」のようにトーンを下げ、確立した事実のように書かない。\n1. 構想を固める——このノートの目的（伝える/記録する/教える/楽しませる）、読者、感情のトーンを決め、あなた自身の口調と一致させる。\n2. 形式を選ぶ——内容に合わせてレイアウトと配色を選ぶ：攻略や構造化データには cover_flow（カバー+カード流）＋タグ＋表/チャート、長文の回想や紀行には article（カバーなし）、おすすめ集には gallery、短いメモには simple。\n3. 内容を組み立てる——冒頭で引き込む、本文で展開、結びで締める。節には heading、散文には paragraph、強調には card/quote/callout、羅列には list、キーワードには tags を使う。ブロックタイプは数合わせではなく必要に応じて選ぶ——構造は内容に仕えるもので、内容が構造に仕えるのではない。\n4. 可視化を活用——3 組以上の比較可能なデータはチャート（棒/折れ線/円）、離散データの比較は表、プロセスや時系列は mermaid フロー図。検索で得たまとまったデータは特に表やチャートに向く。各チャートは隣接する本文で参照・説明され、孤立して置かないこと。後付けではなく、本文の流れに自然に組み込む。\n5. 丁寧に書く——チェックリスト風の決まり文句や飾り形容詞を避け、段落は 3〜8 文で論理的に、見出しは正確かつ機能的に。結論は一度だけ完全に述べ、他の節は言い換えではなく新しい証拠を加える。調べた事実は自然に「調べた」と述べ、元から知っていたように振る舞わない；「検索で得た事実」「自分の推測」「ユーザーの言葉」を区別する。口調は自然に、あなたの性格に合わせる。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "笔记标题（简洁吸引人，建议15字以内）" },
                "layout": {
                    "type": "string",
                    "enum": ["cover_flow", "article", "gallery", "simple"],
                    "description": "布局模板：cover_flow=封面+卡片流（经典卡片风格），article=纯文章流（无封面，适合长文），gallery=图文混排，simple=简洁卡片",
                    "default": "cover_flow"
                },
                "palette": {
                    "type": "string",
                    "enum": ["warm", "fresh", "elegant", "cute", "cool", "nature"],
                    "description": "配色方案：warm=暖色珊瑚粉，fresh=清新薄荷绿，elegant=优雅紫罗兰，cute=可爱粉橙，cool=冷色天蓝，nature=自然橄榄绿",
                    "default": "warm"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "笔记标签（3-5个关键词，如['美食','早餐','简单']）"
                },
                "cover": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "封面大标题" },
                        "subtitle": { "type": "string", "description": "副标题（可选）" },
                        "emoji": { "type": "string", "description": "封面装饰emoji（如🍳✈️📚）" },
                        "background": { "type": "string", "description": "自定义背景CSS（如'#FF6B6B'或'linear-gradient(...)'，留空用配色默认渐变）" }
                    },
                    "required": ["title"],
                    "description": "封面配置（cover_flow/gallery布局需要，article/simple布局忽略）"
                },
                "blocks": {
                    "type": "array",
                    "description": "内容块列表，按顺序排列。每个块有type字段和对应内容",
                    "items": {
                        "type": "object",
                        "properties": {
                            "type": {
                                "type": "string",
                                "enum": ["heading", "paragraph", "card", "quote", "list", "tags", "image", "divider", "callout", "table", "chart", "mermaid", "custom"],
                                "description": "块类型"
                            }
                        }
                    }
                }
            },
            "required": ["title", "blocks"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "笔记标题（简洁吸引人，建议15字以内）" },
                    "layout": {
                        "type": "string",
                        "enum": ["cover_flow", "article", "gallery", "simple"],
                        "description": "布局模板：cover_flow=封面+卡片流（经典卡片风格），article=纯文章流（无封面，适合长文），gallery=图文混排，simple=简洁卡片",
                        "default": "cover_flow"
                    },
                    "palette": {
                        "type": "string",
                        "enum": ["warm", "fresh", "elegant", "cute", "cool", "nature"],
                        "description": "配色方案：warm=暖色珊瑚粉，fresh=清新薄荷绿，elegant=优雅紫罗兰，cute=可爱粉橙，cool=冷色天蓝，nature=自然橄榄绿",
                        "default": "warm"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "笔记标签（3-5个关键词，如['美食','早餐','简单']）"
                    },
                    "cover": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string", "description": "封面大标题" },
                            "subtitle": { "type": "string", "description": "副标题（可选）" },
                            "emoji": { "type": "string", "description": "封面装饰emoji（如🍳✈️📚）" },
                            "background": { "type": "string", "description": "自定义背景CSS（如'#FF6B6B'或'linear-gradient(...)'，留空用配色默认渐变）" }
                        },
                        "required": ["title"],
                        "description": "封面配置（cover_flow/gallery布局需要，article/simple布局忽略）"
                    },
                    "blocks": {
                        "type": "array",
                        "description": "内容块列表，按顺序排列，构成一篇有起承转合的笔记。建议按“开头点题—主体分节—结尾收束”组织：heading 分节、paragraph 写正文、card/quote/callout 做强调、list 罗列要点。3 组以上可比数据用 chart 可视化，离散数据对比优先 table，流程/时序用 mermaid。每个块用type字段指定类型，其余字段为内容。\n可用块类型：\n- heading: {type,text,level(1-3)} 标题\n- paragraph: {type,text} 段落\n- card: {type,title?,body,emoji?} 卡片\n- quote: {type,text,author?} 引用\n- list: {type,items[],ordered?} 列表\n- tags: {type,items[]} 标签云\n- image: {type,url,caption?} 图片\n- divider: {type,emoji?} 分割线\n- callout: {type,text,emoji?} 提示框\n- table: {type,headers[],rows[][],caption?} 数据表格（headers为列头，rows为二维数据）\n- chart: {type,chart_type('bar'柱状/'line'折线/'pie'饼图),categories[],series[{name,data[]}]} 图表\n- mermaid: {type,code,caption?} 流程图（code为Mermaid源码，如```graph TD\\n A-->B```）\n- custom: {type,html} 自定义HTML片段",
                        "items": { "type": "object" }
                    }
                },
                "required": ["title", "blocks"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "ノートタイトル（簡潔で魅力的、15文字以内推奨）" },
                    "layout": {
                        "type": "string",
                        "enum": ["cover_flow", "article", "gallery", "simple"],
                        "description": "レイアウト：cover_flow=カバー+カード流、article=記事流（カバーなし）、gallery=画像テキスト混在、simple=シンプルカード",
                        "default": "cover_flow"
                    },
                    "palette": {
                        "type": "string",
                        "enum": ["warm", "fresh", "elegant", "cute", "cool", "nature"],
                        "description": "配色：warm=暖色コーラルピンク、fresh=ミントグリーン、elegant=エレガント紫、cute=キュートピンクオレンジ、cool=クールブルー、nature=ナチュラルオリーブ",
                        "default": "warm"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "ノートタグ（3-5個のキーワード）"
                    },
                    "cover": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string", "description": "カバー大タイトル" },
                            "subtitle": { "type": "string", "description": "サブタイトル（任意）" },
                            "emoji": { "type": "string", "description": "カバー装飾emoji" },
                            "background": { "type": "string", "description": "カスタム背景CSS" }
                        },
                        "required": ["title"],
                        "description": "カバー設定（cover_flow/galleryレイアウトで必要）"
                    },
                    "blocks": {
                        "type": "array",
                        "description": "コンテンツブロックリスト。各ブロックはtypeフィールドで種類を指定。\n利用可能ブロック：heading/paragraph/card/quote/list/tags/image/divider/callout/custom",
                        "items": { "type": "object" }
                    }
                },
                "required": ["title", "blocks"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let title = input.get("title").and_then(|v| v.as_str()).unwrap_or("").trim();
        if title.is_empty() {
            return ValidationResult::failure("title 不能为空", 2);
        }
        let blocks = input.get("blocks").and_then(|v| v.as_array());
        if blocks.map(|a| a.is_empty()).unwrap_or(true) {
            return ValidationResult::failure("blocks 不能为空（至少需要一个内容块）", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let char_id = if ctx.char_id.is_empty() {
            "vivian".to_string()
        } else {
            ctx.char_id.clone()
        };

        let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let layout = args.get("layout").and_then(|v| v.as_str()).map(parse_layout).unwrap_or_default();
        let palette = args.get("palette").and_then(|v| v.as_str()).map(parse_palette).unwrap_or_default();
        let tags: Vec<String> = args.get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let cover = match args.get("cover") {
            Some(c) => match parse_cover(c) {
                Ok(c) => c,
                Err(e) => return ToolResult::standard_error(&e, None, None),
            },
            None => None,
        };
        let blocks = match parse_blocks(args.get("blocks").unwrap_or(&Value::Null)) {
            Ok(b) => b,
            Err(e) => return ToolResult::standard_error(&e, None, None),
        };

        let now = chrono::Local::now().timestamp() as f64;
        let note = NoteBook {
            id: NoteBook::generate_id(),
            title: title.clone(),
            char_id: char_id.clone(),
            created_at: now,
            updated_at: now,
            tags,
            layout,
            palette,
            cover,
            blocks,
        };

        if let Err(e) = storage::save(&note) {
            return ToolResult::standard_error(&format!("保存笔记失败: {}", e), None, None);
        }

        // emit 事件通知前端刷新笔记列表
        let handle_opt = APP_HANDLE.read().clone();
        if let Some(handle) = handle_opt {
            let _ = handle.emit("notebook:created", json!({
                "note_id": &note.id,
                "char_id": &char_id,
                "title": &title,
            }));

            // 同步到向量知识库，供后续 RAG 检索
            sync_notebook_to_knowledge(&handle, &note).await;
        }

        ToolResult::standard_success(
            &format!("已创建笔记「{}」，可在记忆窗口的笔记 tab 中查阅", title),
            Some(json!({
                "note_id": &note.id,
                "title": &title,
                "char_id": &char_id,
                "block_count": note.blocks.len(),
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn always_load(&self) -> bool {
        false
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "create notebook note page html xiaohongshu red style beautiful card"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Creating notes for trivial content that doesn't need visual presentation",
            "Using it when the user just wants a quick fact or short answer and you are not actually organizing multiple pieces of information",
            "Recreating a note that already exists — if the user asks you to share or edit a note you already made, use list_notebooks to find its note_id and share_notebook / update_notebook instead of calling create_notebook again",
        ]
    }
}

// ============================================================================
// GetNotebookDetailTool
// ============================================================================

pub struct GetNotebookDetailTool;

impl GetNotebookDetailTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GetNotebookDetailTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GetNotebookDetailTool {
    fn name(&self) -> &str {
        "get_notebook_detail"
    }

    fn description(&self) -> &str {
        "Read the full structured content of an existing notebook note. Returns the note's title, layout, palette, tags, cover, and all content blocks as JSON. Always call this BEFORE update_notebook to see the current content and avoid accidentally overwriting or losing existing blocks."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "读取已有笔记的完整结构化内容（标题、布局、配色、标签、封面、全部内容块）。修改笔记前务必先调用此工具查看原内容，避免覆盖或丢失已有内容块。",
            "ja" => "既存ノートの完全な構造化内容（タイトル、レイアウト、配色、タグ、カバー、全コンテンツブロック）を読み取る。ノートを更新する前に必ずこのツールを呼び出して元の内容を確認し、既存ブロックの上書きや消失を防ぐこと。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note_id": { "type": "string", "description": "要查看的笔记 ID" }
            },
            "required": ["note_id"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "note_id": { "type": "string", "description": "要查看的笔记 ID" }
                },
                "required": ["note_id"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "note_id": { "type": "string", "description": "確認するノート ID" }
                },
                "required": ["note_id"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let note_id = input.get("note_id").and_then(|v| v.as_str()).unwrap_or("").trim();
        if note_id.is_empty() {
            return ValidationResult::failure("note_id 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let char_id = if ctx.char_id.is_empty() {
            "vivian".to_string()
        } else {
            ctx.char_id.clone()
        };
        let note_id = args.get("note_id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();

        let note = match storage::load(&char_id, &note_id) {
            Ok(n) => n,
            Err(e) => return ToolResult::standard_error(&format!("读取笔记失败: {}", e), None, None),
        };

        let detail = serde_json::to_value(&note)
            .map_err(|e| format!("序列化失败: {}", e))
            .unwrap_or_else(|_| json!({}));

        ToolResult::standard_success(
            &format!("已读取笔记「{}」的完整内容（{} 个内容块）", note.title, note.blocks.len()),
            Some(detail),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn always_load(&self) -> bool {
        false
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "read notebook note detail content blocks view"
    }
}

// ============================================================================
// UpdateNotebookTool
// ============================================================================

pub struct UpdateNotebookTool;

impl UpdateNotebookTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UpdateNotebookTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for UpdateNotebookTool {
    fn name(&self) -> &str {
        "update_notebook"
    }

    fn description(&self) -> &str {
        "Update an existing notebook note. You can modify the title, layout, palette, tags, cover, or replace content blocks. Only provided fields are updated; omitted fields keep their original values. IMPORTANT: blocks field replaces ALL content blocks — always call get_notebook_detail first to see the current content, then provide the complete updated blocks list (original blocks + your changes) to avoid losing existing content."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "修改已有笔记。可以更新标题、布局、配色、标签、封面或替换内容块，只更新提供的字段，未提供的字段保持原值。重要：blocks 字段会替换全部内容块——修改前务必先调用 get_notebook_detail 查看原内容，然后提供完整的更新后 blocks 列表（原有内容块 + 你的修改），避免丢失已有内容。",
            "ja" => "既存のノートを更新する。タイトル、レイアウト、配色、タグ、カバー、コンテンツブロックを変更できる。提供されたフィールドのみ更新される。重要：blocksフィールドは全コンテンツブロックを置換する——更新前に必ず get_notebook_detail で元の内容を確認し、更新後の完全なblocksリスト（元のブロック + 変更内容）を提供して、既存内容の消失を防ぐこと。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note_id": { "type": "string", "description": "要修改的笔记 ID（create_notebook 返回的 note_id）" },
                "title": { "type": "string", "description": "新标题（可选）" },
                "layout": { "type": "string", "enum": ["cover_flow", "article", "gallery", "simple"], "description": "新布局（可选）" },
                "palette": { "type": "string", "enum": ["warm", "fresh", "elegant", "cute", "cool", "nature"], "description": "新配色（可选）" },
                "tags": { "type": "array", "items": { "type": "string" }, "description": "新标签列表（可选，替换原标签）" },
                "cover": { "type": "object", "description": "新封面配置（可选，替换原封面）" },
                "blocks": { "type": "array", "description": "新内容块列表（可选，替换原全部内容块）" }
            },
            "required": ["note_id"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "note_id": { "type": "string", "description": "要修改的笔记 ID（create_notebook 返回的 note_id）" },
                    "title": { "type": "string", "description": "新标题（可选）" },
                    "layout": { "type": "string", "enum": ["cover_flow", "article", "gallery", "simple"], "description": "新布局（可选）" },
                    "palette": { "type": "string", "enum": ["warm", "fresh", "elegant", "cute", "cool", "nature"], "description": "新配色（可选）" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "新标签列表（可选，替换原标签）" },
                    "cover": { "type": "object", "description": "新封面配置（可选，替换原封面）" },
                    "blocks": { "type": "array", "description": "新内容块列表（可选，替换原全部内容块）" }
                },
                "required": ["note_id"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let note_id = input.get("note_id").and_then(|v| v.as_str()).unwrap_or("").trim();
        if note_id.is_empty() {
            return ValidationResult::failure("note_id 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let char_id = if ctx.char_id.is_empty() {
            "vivian".to_string()
        } else {
            ctx.char_id.clone()
        };
        let note_id = args.get("note_id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();

        let mut note = match storage::load(&char_id, &note_id) {
            Ok(n) => n,
            Err(e) => return ToolResult::standard_error(&format!("读取笔记失败: {}", e), None, None),
        };

        if let Some(title) = args.get("title").and_then(|v| v.as_str()) {
            note.title = title.trim().to_string();
        }
        if let Some(layout) = args.get("layout").and_then(|v| v.as_str()) {
            note.layout = parse_layout(layout);
        }
        if let Some(palette) = args.get("palette").and_then(|v| v.as_str()) {
            note.palette = parse_palette(palette);
        }
        if let Some(tags) = args.get("tags").and_then(|v| v.as_array()) {
            note.tags = tags.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
        }
        if let Some(cover_val) = args.get("cover") {
            match parse_cover(cover_val) {
                Ok(c) => note.cover = c,
                Err(e) => return ToolResult::standard_error(&e, None, None),
            }
        }
        if let Some(blocks_val) = args.get("blocks") {
            match parse_blocks(blocks_val) {
                Ok(b) => note.blocks = b,
                Err(e) => return ToolResult::standard_error(&e, None, None),
            }
        }

        note.updated_at = chrono::Local::now().timestamp() as f64;

        if let Err(e) = storage::save(&note) {
            return ToolResult::standard_error(&format!("保存笔记失败: {}", e), None, None);
        }

        let handle_opt = APP_HANDLE.read().clone();
        if let Some(handle) = handle_opt {
            let _ = handle.emit("notebook:updated", json!({
                "note_id": &note.id,
                "char_id": &char_id,
                "title": &note.title,
            }));

            // 重新同步到向量知识库（内部会先删旧条目再入库新内容）
            sync_notebook_to_knowledge(&handle, &note).await;
        }

        ToolResult::standard_success(
            &format!("已更新笔记「{}」", note.title),
            Some(json!({
                "note_id": &note.id,
                "title": &note.title,
                "block_count": note.blocks.len(),
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn always_load(&self) -> bool {
        false
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "update notebook note edit modify revise restyle"
    }
}

// ============================================================================
// ListNotebooksTool
// ============================================================================

pub struct ListNotebooksTool;

impl ListNotebooksTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ListNotebooksTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ListNotebooksTool {
    fn name(&self) -> &str {
        "list_notebooks"
    }

    fn description(&self) -> &str {
        "List all notebook notes you have already created, with each note's id, title, tags, layout and last-updated time. Use this to find the note_id of an EXISTING note before sharing or editing it. When the user asks you to share 'the note you wrote' / 'a previous note' / 'that note about <topic>', you MUST look up the existing note via list_notebooks (or reuse a note_id already known from context) and then call share_notebook with that id — do NOT create a new note just to share it."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "列出你已创建的所有笔记，包含每条笔记的 id、标题、标签、布局和最后修改时间。用于在分享或修改已有笔记前，先找到对应笔记的 note_id。当用户要求分享「你写的那篇笔记」「以前那篇笔记」「关于某主题的那篇笔记」时，你必须先通过 list_notebooks 查到已有笔记的 note_id（或直接复用上下文中已知的 note_id），再调用 share_notebook 用该 id 分享——不要为了分享而重新创建一篇新笔记。",
            "ja" => "すでに作成したすべてのノートを、各ノートの id・タイトル・タグ・レイアウト・最終更新時刻とともに一覧表示する。既存ノートを共有・編集する前に note_id を見つけるために使用する。ユーザーが「あなたが書いたノート」「以前のノート」「あるテーマのノート」を共有してほしいと言った場合は、必ず list_notebooks で既存ノートの note_id を調べ（またはコンテキストで既知の note_id を再利用し）、その id で share_notebook を呼び出すこと。共有のために新しいノートを作成してはならない。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "keyword": { "type": "string", "description": "可选：按标题/标签模糊过滤，只返回匹配的笔记（留空返回全部）" }
            },
            "required": []
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "keyword": { "type": "string", "description": "可选：按标题/标签模糊过滤，只返回匹配的笔记（留空返回全部）" }
                },
                "required": []
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "keyword": { "type": "string", "description": "任意：タイトル/タグで曖昧絞り込み（空なら全件）" }
                },
                "required": []
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let char_id = if ctx.char_id.is_empty() {
            "vivian".to_string()
        } else {
            ctx.char_id.clone()
        };
        let keyword = args
            .get("keyword")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_lowercase());

        let notes = match storage::list(&char_id) {
            Ok(n) => n,
            Err(e) => return ToolResult::standard_error(&format!("读取笔记列表失败: {}", e), None, None),
        };

        let filtered: Vec<Value> = notes
            .into_iter()
            .filter(|n| match &keyword {
                Some(k) => {
                    n.title.to_lowercase().contains(k)
                        || n.tags.iter().any(|t| t.to_lowercase().contains(k))
                }
                None => true,
            })
            .map(|n| {
                let updated = chrono::DateTime::from_timestamp(n.updated_at as i64, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| n.updated_at.to_string());
                json!({
                    "note_id": n.id,
                    "title": n.title,
                    "tags": n.tags,
                    "layout": n.layout,
                    "last_updated": updated,
                })
            })
            .collect();

        if filtered.is_empty() {
            return ToolResult::standard_success("没有找到匹配的笔记", Some(json!({ "notes": [] })));
        }

        ToolResult::standard_success(
            &format!("共 {} 篇笔记", filtered.len()),
            Some(json!({ "notes": filtered })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn always_load(&self) -> bool {
        false
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "list notebooks note find existing id share"
    }
}

// ============================================================================
// ShareNotebookTool
// ============================================================================

pub struct ShareNotebookTool;

impl ShareNotebookTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ShareNotebookTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ShareNotebookTool {
    fn name(&self) -> &str {
        "share_notebook"
    }

    fn description(&self) -> &str {
        "Share an EXISTING notebook note to the WeChat chat window as a link card. The card shows the note title and a preview, and clicking it opens the note in the memory window's Notebook tab. Use this when the user asks you to share a note you already made. IMPORTANT: when the user says 'share the note you wrote' / 'a previous note' / 'that note about <topic>', find the existing note_id via list_notebooks (or reuse a note_id already known from context) and share THAT note — do NOT call create_notebook to make a new note just for sharing. Include a brief follow-up comment."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "将一篇已有的笔记以链接卡片形式分享到微信聊天窗口。卡片显示笔记标题和预览，点击后在记忆窗口的笔记 tab 中打开完整内容。当用户要求你把做好的笔记分享给他时使用。重要：当用户说「分享你写的那篇笔记」「以前那篇」「关于某主题那篇」时，请先通过 list_notebooks 找到已有笔记的 note_id（或复用上下文中已知的 note_id）并分享那篇——不要调用 create_notebook 为了分享而重新生成新笔记。需附带一句简短的跟进评论。",
            "ja" => "既存のノートをリンクカードとしてWeChatチャットウィンドウに共有する。カードにはノートタイトルとプレビューが表示され、クリックするとメモリウィンドウのノートタブで完全な内容が開く。ユーザーが作成済みノートの共有を求めた時に使用する。重要：ユーザーが「あなたが書いたノート」「以前のノート」「あるテーマのノート」を共有してほしいと言った場合、list_notebooks で既存ノートの note_id を見つけ（またはコンテキストで既知の note_id を再利用）、そのノートを共有すること。共有のために create_notebook で新しいノートを作成してはならない。短いフォローアップコメントを添えること。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note_id": { "type": "string", "description": "要分享的笔记 ID" },
                "follow_up": { "type": "string", "description": "分享后的简短跟进评论（如'给你整理了一份早餐食谱～'），语气自然随意，1-2句" }
            },
            "required": ["note_id", "follow_up"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "note_id": { "type": "string", "description": "要分享的笔记 ID" },
                    "follow_up": { "type": "string", "description": "分享后的简短跟进评论（如'给你整理了一份早餐食谱～'），语气自然随意，1-2句" }
                },
                "required": ["note_id", "follow_up"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let note_id = input.get("note_id").and_then(|v| v.as_str()).unwrap_or("").trim();
        if note_id.is_empty() {
            return ValidationResult::failure("note_id 不能为空", 2);
        }
        let follow_up = input.get("follow_up").and_then(|v| v.as_str()).unwrap_or("").trim();
        if follow_up.is_empty() {
            return ValidationResult::failure("follow_up 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let char_id = if ctx.char_id.is_empty() {
            "vivian".to_string()
        } else {
            ctx.char_id.clone()
        };
        let note_id = args.get("note_id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let follow_up = args.get("follow_up").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();

        let note = match storage::load(&char_id, &note_id) {
            Ok(n) => n,
            Err(e) => return ToolResult::standard_error(&format!("读取笔记失败: {}", e), None, None),
        };

        let app_handle = match APP_HANDLE.read().clone() {
            Some(h) => h,
            None => return ToolResult::standard_error("AppHandle 未初始化", None, None),
        };

        share_notebook_to_wechat(&app_handle, &char_id, &note, &follow_up).await;

        ToolResult::standard_success(
            &format!("已分享笔记「{}」到微信聊天", note.title),
            Some(json!({
                "shared": true,
                "note_id": &note.id,
                "title": &note.title,
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn always_load(&self) -> bool {
        false
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "share notebook note wechat chat send card"
    }
}

/// 分享笔记到微信聊天面板（复用 share_link 的链接卡片样式）
async fn share_notebook_to_wechat(
    app_handle: &AppHandle,
    char_id: &str,
    note: &NoteBook,
    follow_up: &str,
) {
    let now = chrono::Local::now().to_rfc3339();
    let now_ts = chrono::Local::now().timestamp() as f64;

    // 笔记链接用 vivian://notebook/<char_id>/<note_id> 协议，前端识别后跳转
    let note_url = format!("vivian://notebook/{}/{}", char_id, note.id);
    let preview: String = note.blocks.iter()
        .filter_map(|b| match b {
            Block::Paragraph { text, .. } | Block::Card { body: text, .. } => Some(text.as_str()),
            _ => None,
        })
        .next()
        .map(|s| s.chars().take(80).collect())
        .unwrap_or_else(|| format!("{}个内容块", note.blocks.len()));

    // 写入对话历史
    if let Some(state) = app_handle.try_state::<Arc<AppState>>() {
        let characters = state.characters.read();
        if let Some(instance) = characters.get(char_id) {
            let card_content = format!("{}\n{}", note.title, note_url);
            let card_msg = DialogChatMessage {
                role: "assistant".to_string(),
                content: card_content,
                timestamp: Some(chrono::Local::now()),
                tool_calls: None,
                tool_call_id: None,
                reasoning: None,
                images: None,
                meta: Some(MessageMeta {
                    source: MessageSource::Assistant,
                    is_memory_disabled: false,
                    mirror_kind: None,
                    channel: Some("wechat".to_string()),
                    kind: None,
                }),
            };
            let _ = instance.brain.dialogue.add_message_with_metadata(card_msg, json!({
                "kind": "notebook_link",
                "link_card": {
                    "url": note_url,
                    "title": note.title,
                    "description": preview,
                    "source": "notebook",
                    "note_id": note.id,
                    "char_id": char_id,
                    "palette": format!("{:?}", note.palette).to_lowercase(),
                }
            }));

            if !follow_up.is_empty() {
                let follow_msg = DialogChatMessage {
                    role: "assistant".to_string(),
                    content: follow_up.to_string(),
                    timestamp: Some(chrono::Local::now()),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning: None,
                    images: None,
                    meta: Some(MessageMeta {
                        source: MessageSource::Assistant,
                        is_memory_disabled: false,
                        mirror_kind: None,
                        channel: Some("wechat".to_string()),
                        kind: None,
                    }),
                };
                let _ = instance.brain.dialogue.add_message_with_metadata(follow_msg, json!({}));
            }
        }
    }

    // emit 链接卡片事件
    let _ = app_handle.emit(
        "chat:link_card",
        json!({
            "url": note_url,
            "title": note.title,
            "description": preview,
            "source": "notebook",
            "note_id": note.id,
            "char_id": char_id,
            "palette": format!("{:?}", note.palette).to_lowercase(),
            "timestamp": now,
            "channel": "wechat",
            "is_notebook": true,
        }),
    );

    // 横幅提示
    let need_banner = match app_handle.get_webview_window("chat") {
        Some(win) => !win.is_visible().ok().unwrap_or(false),
        None => true,
    };
    if need_banner {
        let banner_preview = if !follow_up.is_empty() {
            format!("{}: {}", note.title, follow_up)
        } else {
            note.title.clone()
        };
        let _ = app_handle.emit(
            "wechat:message_banner",
            json!({
                "character_id": char_id,
                "preview": banner_preview,
                "kind": "notebook_link",
                "timestamp": now_ts,
            }),
        );
    }

    // emit 跟进评论
    if !follow_up.is_empty() {
        let _ = app_handle.emit(
            "chat:assistant_message",
            json!({
                "content": follow_up,
                "timestamp": now,
                "character_id": char_id,
                "channel": "wechat",
            }),
        );
    }
}

// ============================================================================
// 知识库同步 - 笔记内容写入向量知识库供 RAG 检索
// ============================================================================

/// 把笔记内容块拼接成可被检索的纯文本
fn note_to_searchable_text(note: &NoteBook) -> String {
    let mut parts = Vec::new();
    parts.push(note.title.clone());
    if let Some(cover) = &note.cover {
        if let Some(sub) = &cover.subtitle {
            parts.push(sub.clone());
        }
    }
    for block in &note.blocks {
        match block {
            Block::Heading { text, .. } | Block::Paragraph { text, .. } | Block::Callout { text, .. } => {
                parts.push(text.clone());
            }
            Block::Card { title, body, .. } => {
                if let Some(t) = title {
                    parts.push(t.clone());
                }
                parts.push(body.clone());
            }
            Block::Quote { text, author, .. } => {
                parts.push(text.clone());
                if let Some(a) = author {
                    parts.push(a.clone());
                }
            }
            Block::List { items, .. } | Block::Tags { items } => {
                parts.extend(items.iter().cloned());
            }
            Block::Image { caption, .. } => {
                if let Some(c) = caption {
                    parts.push(c.clone());
                }
            }
            Block::Divider { .. } => {}
            Block::Table { headers, rows, caption } => {
                parts.extend(headers.iter().cloned());
                if let Some(c) = caption {
                    parts.push(c.clone());
                }
                for row in rows {
                    parts.extend(row.iter().cloned());
                }
            }
            Block::Chart { chart_type, title, categories, series } => {
                parts.push(chart_type.clone());
                if let Some(t) = title {
                    parts.push(t.clone());
                }
                parts.extend(categories.iter().cloned());
                for s in series {
                    parts.push(s.name.clone());
                    for v in &s.data {
                        parts.push(v.to_string());
                    }
                }
            }
            Block::Mermaid { code, caption } => {
                if let Some(c) = caption {
                    parts.push(c.clone());
                }
                // 提取 Mermaid 定义中的节点文案（去掉语法符号）
                let text: String = code
                    .lines()
                    .filter_map(|line| {
                        // 去掉 --> 箭头、[|(]{} 等语法，保留可读文本
                        let cleaned = line
                            .replace("-->", " ")
                            .replace("---", " ")
                            .replace(['[', ']', '(', ')', '{', '}', '|', '>', '<'], " ");
                        let t = cleaned.trim();
                        if t.is_empty() { None } else { Some(t.to_string()) }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                if !text.is_empty() {
                    parts.push(text);
                }
            }
            Block::Custom { html } => {
                // 粗略剥离 HTML 标签，保留文本
                let text: String = html
                    .chars()
                    .scan(false, |in_tag, c| {
                        match c {
                            '<' => {
                                *in_tag = true;
                                Some(None)
                            }
                            '>' => {
                                *in_tag = false;
                                Some(None)
                            }
                            _ if !*in_tag => Some(Some(c)),
                            _ => Some(None),
                        }
                    })
                    .flatten()
                    .collect();
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
            }
        }
    }
    parts.join("\n")
}

/// 同步笔记到向量知识库（创建/更新时调用）
///
/// 以 `MemoryType::Knowledge` 入库，metadata 中存 note_id 关联，
/// source="notebook"，ttl_days=Some(-1) 永不过期。
/// 嵌入失败只记日志不阻断主流程（笔记文件已保存）。
pub(crate) async fn sync_notebook_to_knowledge(app_handle: &AppHandle, note: &NoteBook) {
    let state = match app_handle.try_state::<Arc<AppState>>() {
        Some(s) => s,
        None => {
            tracing::warn!("[Notebook] AppState 未注入，跳过知识库同步");
            return;
        }
    };
    let memory = match state.get_character(Some(&note.char_id)) {
        Ok(inst) => inst.brain.memory.clone(),
        Err(e) => {
            tracing::warn!("[Notebook] 获取角色 {} 的 MemoryManager 失败: {}", note.char_id, e);
            return;
        }
    };

    // 先删除该 note_id 的旧知识条目，避免重复
    remove_notebook_from_knowledge(&memory, &note.id).await;

    let content = note_to_searchable_text(note);
    if content.trim().is_empty() {
        tracing::warn!("[Notebook] 笔记 {} 内容为空，跳过知识库同步", note.id);
        return;
    }

    let mut tags = note.tags.clone();
    tags.push("notebook".to_string());
    tags.sort();
    tags.dedup();

    match memory
        .add_knowledge_document(&note.title, &content, tags, "notebook", Some(-1))
        .await
    {
        Ok(item) => {
            // 回写 memory_id 到笔记的 note_id 关联（通过 metadata）
            // 这里我们把 memory_id 记到笔记的 index 里方便后续更新时查找
            tracing::info!(
                "[Notebook] 笔记「{}」已同步到知识库，memory_id={}",
                note.title,
                item.id
            );
            // 把关联关系写入笔记目录的 .memory_ref 文件
            let ref_path = storage::note_memory_ref_path(&note.char_id, &note.id);
            let _ = std::fs::write(&ref_path, &item.id);
        }
        Err(e) => {
            tracing::warn!("[Notebook] 笔记同步到知识库失败: {}", e);
        }
    }
}

/// 从知识库删除指定笔记的旧条目（更新前调用）
async fn remove_notebook_from_knowledge(memory: &Arc<crate::memory::MemoryManager>, note_id: &str) {
    let ref_path = storage::note_memory_ref_path(memory.char_id(), note_id);
    if let Ok(memory_id) = std::fs::read_to_string(&ref_path) {
        let memory_id = memory_id.trim();
        if !memory_id.is_empty() {
            if let Err(e) = memory.delete_knowledge_document(memory_id).await {
                tracing::warn!("[Notebook] 删除旧知识条目 {} 失败: {}", memory_id, e);
            }
        }
        let _ = std::fs::remove_file(&ref_path);
    }
}

// ============================================================================
// CreateHtmlNoteTool
// ============================================================================

/// 让 LLM 直接撰写完整自包含 HTML 笔记（不经过结构化内容块渲染引擎）
///
/// 与 create_notebook（结构化 blocks → 预设主题渲染）互补：当内容需要完全自由的
/// 版式、配色与可视化控制时（数据大屏、精美报告、落地页式笔记、复杂图文混排等），
/// 由 LLM 直接产出完整 HTML 文档，系统原样保存。渲染链路由前端 Shadow DOM 承接，
/// 样式经 :host 适配，图表/流程图由前端懒加载初始化。
pub struct CreateHtmlNoteTool;

impl CreateHtmlNoteTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CreateHtmlNoteTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for CreateHtmlNoteTool {
    fn name(&self) -> &str {
        "create_html_note"
    }

    fn description(&self) -> &str {
        "Create a fully self-contained HTML note where YOU write the complete HTML/CSS directly — not the block-based renderer. Ideal when the content needs full layout and visual freedom: data dashboards, polished reports, landing-page style notes, complex mixed media. Provide the complete HTML document (with <style>), and the system saves it as-is and renders it in the Notebook tab. Reuse the same authoring discipline as a polished HTML report: design a coherent visual system via CSS variables on :root (colors, fonts, spacing); use font-size/weight/whitespace for hierarchy, not font families; keep layouts responsive; ensure ink-on-bg contrast ≥ 4.5:1; structure with proper headings; use a BODY-level or :host-compatible background rather than relying on a <body> element (the document is rendered inside a shadow root, so :root and body selectors are rewritten to :host automatically). For data, use ECharts via <div class=\"nb-chart\" data-option='{...}'> (bar/line/pie supported, lazy-loaded by the frontend) or Mermaid via <pre class=\"mermaid\">...</pre> — every chart must have a visible title and be referenced by adjacent prose. Use tables wrapped in a scrollable container for discrete comparisons. Cite sources when facts come from search. Keep the tone natural and consistent with your personality. Prefer this tool over create_notebook when you need fine-grained visual control; use create_notebook for simpler card-style notes."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "制作一篇由你直接撰写完整 HTML/CSS 的自包含笔记（不经过结构化内容块渲染）。适合需要完全自由版式与视觉控制的场景：数据大屏、精美报告、落地页式笔记、复杂图文混排等。你提供完整的 HTML 文档（含 <style>），系统原样保存并在笔记 tab 中原样渲染。请遵循与精美 HTML 报告一致的撰写纪律：用 :root 上的 CSS 变量（颜色/字体/间距）建立统一视觉系统；用字号粗细与留白建立层级，而非换字体家族；保持响应式布局；正文与背景对比度 ≥ 4.5:1；用规范标题组织结构；背景建议写在 body/{:host 兼容} 上——文档在 Shadow DOM 内渲染，:root 与 body 选择器会被自动改写为 :host。数据可视化用 ECharts（<div class=\"nb-chart\" data-option='{...}'>，支持柱状/折线/饼图，前端懒加载）或 Mermaid（<pre class=\"mermaid\">…</pre>），每个图表必须有可见标题并被相邻正文引用解释。离散数据对比优先用可横向滚动的表格容器。检索得来的事实要自然说明是查到的，不假装本来就知道，并区分「搜索事实」「你的推断」「用户原话」。保持语气自然，符合你的性格。需要精细视觉控制时优先用本工具；简单卡片风格笔记用 create_notebook。",
            "ja" => "完全自己でHTML/CSSを書く自己完結型HTMLノートを作成する（構造化コンテンツブロックのレンダラーは使わない）。自由なレイアウトと視覚制御が必要な場面に最適：データダッシュボード、洗練されたレポート、ランディングページ風ノート、複雑なメディア混在など。完全なHTML文書（<style>含む）を提供すると、システムがそのまま保存し、ノートタブでそのままレンダリングする。洗練されたHTMLレポートと同じ執筆規律に従うこと：:root上のCSS変数（色/フォント/間隔）で一貫したビジュアルシステムを確立；フォントファミリーではなくフォントサイズ/太さ/空白で階層を作る；レスポンシブレイアウトを維持；本文と背景のコントラスト比は4.5:1以上；見出しで構造化；背景はbody/{:host}互換で書く——文書はShadow DOM内でレンダリングされ、:rootとbodyセレクタは自動的に:hostへ書き換えられる。データ可視化はECharts（<div class=\"nb-chart\" data-option='{...}'>、棒/折れ線/円対応、フロントエンドで遅延ロード）またはMermaid（<pre class=\"mermaid\">…</pre>）を使い、各チャートには可視タイトルを付け、隣接する本文で参照・説明すること。離散データ比較は横スクロール可能なテーブルコンテナを優先。検索で得た事実は自然に「調べた」と述べ、元から知っていたように振る舞わない。「検索で得た事実」「自分の推測」「ユーザーの言葉」を区別する。口調は自然に、性格に合わせる。細かい視覚制御が必要な場合は本ツールを、簡単なカード風ノートはcreate_notebookを使うこと。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "笔记标题（简洁吸引人，建议15字以内）" },
                "html": { "type": "string", "description": "完整的自包含 HTML 文档（含 <style>，可含 <div class=\"nb-chart\" data-option='...'> 图表与 <pre class=\"mermaid\"> 流程图）。系统原样保存。不要使用 <script> 标签——脚本不会执行，可视化请用 nb-chart / mermaid 约定。" },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "笔记标签（3-5个关键词）"
                }
            },
            "required": ["title", "html"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "笔记标题（简洁吸引人，建议15字以内）" },
                    "html": { "type": "string", "description": "完整自包含 HTML 文档（含 <style>）。可含 <div class=\"nb-chart\" data-option='...> 图表（柱状/折线/饼图）与 <pre class=\"mermaid\"> 流程图，前端会懒加载渲染。不要写 <script> 标签——脚本不会执行。样式用 :root 上的 CSS 变量统一，背景写在 body（会被自动改写为 :host）。" },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "笔记标签（3-5个关键词）"
                    }
                },
                "required": ["title", "html"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "ノートタイトル（簡潔で魅力的、15文字以内推奨）" },
                    "html": { "type": "string", "description": "完全自己完結のHTML文書（<style>含む）。<div class=\"nb-chart\" data-option='...> チャート（棒/折れ線/円）と <pre class=\"mermaid\"> フローチャートを含められ、フロントエンドで遅延ロードされる。<script>タグは書かないこと——実行されない。スタイルは:root上のCSS変数で統一し、背景はbodyに書く（自動的に:hostへ書き換えられる）。" },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "ノートタグ（3-5個のキーワード）"
                    }
                },
                "required": ["title", "html"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let title = input.get("title").and_then(|v| v.as_str()).unwrap_or("").trim();
        if title.is_empty() {
            return ValidationResult::failure("title 不能为空", 2);
        }
        let html = input.get("html").and_then(|v| v.as_str()).unwrap_or("").trim();
        if html.is_empty() {
            return ValidationResult::failure("html 不能为空（需要提供完整的 HTML 文档）", 2);
        }
        if html.contains("<script") {
            return ValidationResult::failure("html 中不允许包含 <script> 标签（脚本不会执行且可能被拦截），请改用 nb-chart / mermaid 约定实现可视化", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let char_id = if ctx.char_id.is_empty() {
            "vivian".to_string()
        } else {
            ctx.char_id.clone()
        };

        let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let html = args.get("html").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let tags: Vec<String> = args.get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();

        let note_id = NoteBook::generate_id();
        if let Err(e) = storage::save_raw_html(&char_id, &note_id, &title, &tags, &html) {
            return ToolResult::standard_error(&format!("保存笔记失败: {}", e), None, None);
        }

        // emit 事件通知前端刷新笔记列表并定位
        let handle_opt = APP_HANDLE.read().clone();
        if let Some(handle) = handle_opt {
            let _ = handle.emit("notebook:created", json!({
                "note_id": &note_id,
                "char_id": &char_id,
                "title": &title,
            }));
            // 同步到向量知识库供后续 RAG 检索（raw_html 笔记从中提取纯文本）
            sync_raw_html_to_knowledge(&handle, &char_id, &note_id, &title, &tags, &html).await;
        }

        ToolResult::standard_success(
            &format!("已创建 HTML 笔记「{}」，可在记忆窗口的笔记 tab 中查阅", title),
            Some(json!({
                "note_id": &note_id,
                "title": &title,
                "char_id": &char_id,
                "render_type": "raw_html",
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn always_load(&self) -> bool {
        false
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "create html note page self-contained dashboard report custom html css"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Using it for simple card-style notes that create_notebook can handle — prefer that tool for basic content",
            "Including <script> tags — they do not execute in the shadow-root renderer; use nb-chart / mermaid conventions instead",
            "Recreating a note that already exists — use list_notebooks to find note_id and share_notebook instead",
        ]
    }
}

/// 将 raw_html 笔记中的可见文本提取出来，供知识库 RAG 检索
fn raw_html_to_searchable_text(html: &str) -> String {
    // 粗略剥离 HTML 标签，保留文本；同时保留 nb-chart / mermaid 的图表标题与流程说明
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => {
                text.push(c);
            }
            _ => {}
        }
    }
    // 压缩空白，逐行保留有意义文本
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 同步 raw_html 笔记到向量知识库
pub(crate) async fn sync_raw_html_to_knowledge(
    app_handle: &AppHandle,
    char_id: &str,
    note_id: &str,
    title: &str,
    tags: &[String],
    html: &str,
) {
    let state = match app_handle.try_state::<Arc<AppState>>() {
        Some(s) => s,
        None => {
            tracing::warn!("[Notebook] AppState 未注入，跳过知识库同步");
            return;
        }
    };
    let memory = match state.get_character(Some(char_id)) {
        Ok(inst) => inst.brain.memory.clone(),
        Err(e) => {
            tracing::warn!("[Notebook] 获取角色 {} 的 MemoryManager 失败: {}", char_id, e);
            return;
        }
    };

    let content = raw_html_to_searchable_text(html);
    if content.trim().is_empty() {
        tracing::warn!("[Notebook] HTML 笔记 {} 无可检索文本，跳过知识库同步", note_id);
        return;
    }

    let mut tag_vec = tags.to_vec();
    tag_vec.push("notebook".to_string());
    tag_vec.sort();
    tag_vec.dedup();

    match memory
        .add_knowledge_document(title, &content, tag_vec, "notebook", Some(-1))
        .await
    {
        Ok(item) => {
            let ref_path = storage::note_memory_ref_path(char_id, note_id);
            let _ = std::fs::write(&ref_path, &item.id);
        }
        Err(e) => {
            tracing::warn!("[Notebook] HTML 笔记同步到知识库失败: {}", e);
        }
    }
}
