//! 笔记本模块 - 纸页风格的 HTML 页面生成与管理
//!
//! 智能体根据搜集到的信息，通过结构化 JSON 描述内容编排，
//! 后端渲染引擎将 JSON + 预设 CSS 主题合成为排版清晰的纸页风格 HTML 页面。
//!
//! 架构：
//! - 数据结构：NoteBook（元数据 + 内容块）
//! - 渲染引擎：renderer.rs（CSS 主题 + HTML 生成）
//! - 存储层：storage.rs（按角色隔离的文件 CRUD）

pub mod collected;
pub mod css_guide;
pub mod persona_brief;
pub mod renderer;
pub mod storage;

use serde::{Deserialize, Serialize};

/// 笔记元数据 + 内容
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteBook {
    /// 笔记唯一 ID
    pub id: String,
    /// 笔记标题
    pub title: String,
    /// 创建者角色 ID
    pub char_id: String,
    /// 创建时间戳（Unix 秒）
    pub created_at: f64,
    /// 更新时间戳（Unix 秒）
    pub updated_at: f64,
    /// 标签列表
    #[serde(default)]
    pub tags: Vec<String>,
    /// 布局模板
    pub layout: Layout,
    /// 配色方案
    pub palette: Palette,
    /// 自定义 CSS（覆盖预设主题）。
    ///
    /// 与 `palette` **互斥**：有值时 `renderer.rs` 跳过预设配色变量，改用这段 CSS
    /// 作为唯一主题来源。注入点在预设 CSS 之后，因此同优先级下自定义规则后写先生效。
    /// 智能体可用 `.container`/`.cover`/`.card`/`.heading` 等选择器覆盖排版。
    #[serde(default)]
    pub custom_css: Option<String>,
    /// 封面（CoverFlow/Gallery 布局需要）
    #[serde(default)]
    pub cover: Option<Cover>,
    /// 内容块列表
    pub blocks: Vec<Block>,
}

/// 布局模板
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// 封面手册（封面 + 单栏内容）
    #[default]
    CoverFlow,
    /// 阅读文章（简洁标题 + 连续正文，适合长文）
    Article,
    /// 图文画册（图片和卡片双栏，窄窗口自动单栏）
    Gallery,
    /// 轻量短笺（紧凑纸页，适合短消息）
    Simple,
}

impl Layout {
    /// 与 serde 的布局键一致，供摘要与前端布局标签使用。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CoverFlow => "cover_flow",
            Self::Article => "article",
            Self::Gallery => "gallery",
            Self::Simple => "simple",
        }
    }
}

/// 配色方案
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Palette {
    /// 暖色：陶棕 / 米白
    #[default]
    Warm,
    /// 清新：青绿 / 浅鼠尾草
    Fresh,
    /// 优雅：雾紫 / 浅丁香
    Elegant,
    /// 可爱：柔粉 / 玫瑰灰
    Cute,
    /// 冷色：灰蓝 / 雾白
    Cool,
    /// 自然：苔绿 / 浅草色
    Nature,
}

/// 主题来源：预设配色或智能体自定义 CSS（**互斥**）。
///
/// 存在的意义是把「配色从哪来」收敛成单一决策点——`renderer.rs` 不再到处
/// `if has_custom_css`，前端与工具侧也只需判断这一个枚举就能告诉用户当前生效的是哪套。
#[derive(Debug, Clone)]
pub enum Theme {
    /// 用预设配色（`Palette`）
    Preset(Palette),
    /// 用智能体写的自定义 CSS 片段。
    ///
    /// 渲染时作为**唯一主题来源**：预设的配色变量降级为兜底（自定义 CSS 只写
    /// `.card` 间距而没定义色板时，至少不会拿到错误的暖色），自定义规则注入在
    /// 预设 CSS 之后，同优先级下后写先生效。
    Custom(String),
}

impl NoteBook {
    /// 解析当前生效的主题。`custom_css` 为空白串时视为无（等同 None）。
    pub fn theme(&self) -> Theme {
        match self.custom_css.as_deref().map(str::trim) {
            Some(css) if !css.is_empty() => Theme::Custom(css.to_string()),
            _ => Theme::Preset(self.palette.clone()),
        }
    }

    /// 该笔记是否由智能体自定义了CSS（而非用预设配色）。
    pub fn has_custom_css(&self) -> bool {
        matches!(self.theme(), Theme::Custom(_))
    }
}

/// 封面
///
/// 无 `emoji` 字段：笔记不使用 emoji 装饰。旧的 `note.json` 里若残留该字段，
/// serde 会忽略它，存量数据无需迁移。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cover {
    /// 封面大标题
    pub title: String,
    /// 副标题
    #[serde(default)]
    pub subtitle: Option<String>,
    /// 自定义背景（CSS background 值，如 "#FF6B6B" 或 "linear-gradient(...)"）
    #[serde(default)]
    pub background: Option<String>,
}

/// 内容块（LLM 自由编排的基本单元）
///
/// 无 `emoji` 字段：笔记不使用 emoji 装饰，视觉层次靠标题层级、配色与卡片
/// 结构表达。旧的 `note.json` 里若残留该字段，serde 会忽略它，存量无需迁移。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    /// 标题
    Heading {
        text: String,
        /// 1-3，1 最大
        #[serde(default = "default_heading_level")]
        level: u8,
        #[serde(default)]
        style: Option<BlockStyle>,
    },
    /// 普通段落
    Paragraph {
        text: String,
        #[serde(default)]
        style: Option<BlockStyle>,
    },
    /// 卡片（带标题和正文的独立块）
    Card {
        #[serde(default)]
        title: Option<String>,
        body: String,
        #[serde(default)]
        style: Option<BlockStyle>,
    },
    /// 引用
    Quote {
        text: String,
        #[serde(default)]
        author: Option<String>,
        #[serde(default)]
        style: Option<BlockStyle>,
    },
    /// 列表
    List {
        items: Vec<String>,
        #[serde(default)]
        ordered: bool,
        #[serde(default)]
        style: Option<BlockStyle>,
    },
    /// 标签云
    Tags {
        items: Vec<String>,
    },
    /// 图片
    Image {
        url: String,
        #[serde(default)]
        caption: Option<String>,
    },
    /// 分割线（水平分隔）
    Divider,
    /// 提示框（高亮重要信息）
    Callout {
        text: String,
        #[serde(default)]
        style: Option<BlockStyle>,
    },
    /// 数据表格
    Table {
        /// 列头
        headers: Vec<String>,
        /// 数据行（每行与列头对齐）
        rows: Vec<Vec<String>>,
        /// 可选标题/说明
        #[serde(default)]
        caption: Option<String>,
    },
    /// 图表（ECharts：bar/line/pie）
    Chart {
        /// 图表类型：bar/line/pie
        #[serde(rename = "chart_type")]
        chart_type: String,
        /// 图表标题（可选）
        #[serde(default)]
        title: Option<String>,
        /// 分类轴（柱状/折线 X 轴，饼图标签）
        categories: Vec<String>,
        /// 数据系列
        series: Vec<ChartSeries>,
    },
    /// Mermaid 流程图
    Mermaid {
        /// Mermaid 图定义源码
        code: String,
        #[serde(default)]
        caption: Option<String>,
    },
    /// 自定义 HTML 片段（经沙箱清理，禁止 script/on*/javascript:）
    Custom {
        html: String,
    },
    /// **不渲染的元数据**（存于 note.json，参与知识库检索，但页面上看不到）
    ///
    /// 用于「数据来源说明」「检索可靠性评估」这类**给智能体自己看、不给读者看**
    /// 的内容：采集链路需要它支撑后续问答与 RAG 召回，但把它渲染出来会变成
    /// 一大段来源声明，破坏笔记的叙事感（尤其正文已是第一人称日记口吻时）。
    ///
    /// 之所以做成块而不是 NoteBook 上的独立字段：块序列是数据层的统一载体，
    /// 智能体用 `create_notebook` 时也能主动写Meta 块，不必为它新增一套参数。
    Meta {
        /// 键（如 "provenance" / "reliability"），便于将来按类检索
        key: String,
        /// 载荷文本，参与向量检索
        text: String,
    },
}

/// 图表数据系列
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartSeries {
    /// 系列名
    pub name: String,
    /// 数据点
    pub data: Vec<f64>,
}

fn default_heading_level() -> u8 {
    2
}

/// 单个内容块的文本行内样式（可视化编辑模式应用）
///
/// 全部字段可选，仅提供被显式设置的样式；未设置时沿用主题默认样式。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlockStyle {
    /// 文本颜色（CSS 颜色值，如 "#e74c3c" 或 "rgb(...)"）
    #[serde(default)]
    pub color: Option<String>,
    /// 字号（像素，如 18）
    #[serde(default)]
    pub font_size: Option<u8>,
    /// 加粗
    #[serde(default)]
    pub bold: bool,
    /// 斜体
    #[serde(default)]
    pub italic: bool,
    /// 水平对齐：left/center/right
    #[serde(default)]
    pub align: Option<String>,
}

impl BlockStyle {
    /// 生成内联 CSS style 字符串（供渲染层应用）
    pub fn to_inline_css(&self) -> String {
        let mut css: Vec<String> = Vec::new();
        if let Some(c) = &self.color {
            css.push(format!("color:{}", c));
        }
        if let Some(sz) = self.font_size {
            css.push(format!("font-size:{}px", sz));
        }
        if self.bold {
            css.push("font-weight:700".to_string());
        }
        if self.italic {
            css.push("font-style:italic".to_string());
        }
        if let Some(a) = &self.align {
            if !a.is_empty() {
                css.push(format!("text-align:{}", a));
            }
        }
        css.join(";")
    }
}

impl NoteBook {
    /// 生成新的笔记 ID
    pub fn generate_id() -> String {
        let ts = chrono::Local::now().timestamp_millis();
        format!("note_{}", ts)
    }
}
