//! 笔记本模块 - 卡片风格的 HTML 页面生成与管理
//!
//! 智能体根据搜集到的信息，通过结构化 JSON 描述内容编排，
//! 后端渲染引擎将 JSON + 预设 CSS 主题合成为漂亮的卡片风格 HTML 页面。
//!
//! 架构：
//! - 数据结构：NoteBook（元数据 + 内容块）
//! - 渲染引擎：renderer.rs（CSS 主题 + HTML 生成）
//! - 存储层：storage.rs（按角色隔离的文件 CRUD）

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
    /// 封面 + 卡片流（经典卡片风格）
    #[default]
    CoverFlow,
    /// 纯文章流（无封面，适合长文）
    Article,
    /// 图文混排（适合图多文少的场景）
    Gallery,
    /// 简洁卡片（单卡片，适合短消息）
    Simple,
}

/// 配色方案
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Palette {
    /// 暖色：珊瑚粉 / 暖黄 / 米白
    #[default]
    Warm,
    /// 清新：薄荷绿 / 天蓝 / 冰白
    Fresh,
    /// 优雅：紫罗兰 / 深灰蓝 / 淡紫
    Elegant,
    /// 可爱：粉色 / 橙黄 / 粉白
    Cute,
    /// 冷色：天蓝 / 薰衣草紫 / 雾蓝
    Cool,
    /// 自然：橄榄绿 / 棕褐 / 嫩绿
    Nature,
}

/// 封面
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cover {
    /// 封面大标题
    pub title: String,
    /// 副标题
    #[serde(default)]
    pub subtitle: Option<String>,
    /// 装饰 emoji
    #[serde(default)]
    pub emoji: Option<String>,
    /// 自定义背景（CSS background 值，如 "#FF6B6B" 或 "linear-gradient(...)")
    #[serde(default)]
    pub background: Option<String>,
}

/// 内容块（LLM 自由编排的基本单元）
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
        emoji: Option<String>,
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
    /// 分割线（可带 emoji 装饰）
    Divider {
        #[serde(default)]
        emoji: Option<String>,
    },
    /// 提示框（高亮重要信息）
    Callout {
        text: String,
        #[serde(default)]
        emoji: Option<String>,
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
