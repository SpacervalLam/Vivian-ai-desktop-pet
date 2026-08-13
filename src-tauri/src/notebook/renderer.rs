//! 渲染引擎 - 将 NoteBook JSON + 预设 CSS 主题合成为自包含 HTML 页面
//!
//! 设计要点：
//! - CSS 主题预设卡片风格（圆角卡片 / 暖色调 / emoji 装饰 / 标签胶囊）
//! - 6 套配色方案（warm/fresh/elegant/cute/cool/nature）
//! - 4 种布局模板（cover_flow/article/gallery/simple）
//! - 自定义 HTML 片段经 sanitize_custom_html 清理（移除 script/on*/javascript:）
//! - 输出为自包含 HTML 文件（内联 CSS，可直接在 webview/浏览器打开）

use super::{Block, BlockStyle, ChartSeries, Cover, Layout, NoteBook, Palette};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};

/// 图表 DOM id 自增计数器
mod chart_id_counter {
    use super::{AtomicUsize, Ordering};
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    pub fn next() -> usize {
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    }
}

/// 配色方案对应的 CSS 变量（编辑式玻璃拟态风格）
struct PaletteColors {
    /// 主强调色（暖橙风格）
    primary: &'static str,
    /// 次要强调色（青绿风格，与主色互补）
    secondary: &'static str,
    /// 主色浅底（用于分隔线/表格头）
    primary_light: &'static str,
    /// 主色柔和透明底（标签/提示背景）
    primary_soft: &'static str,
    /// 次要色柔和透明底
    secondary_soft: &'static str,
    /// 页面背景（暖米白）
    bg: &'static str,
    /// 正文墨色
    text: &'static str,
    /// 次要文字色
    text_secondary: &'static str,
    /// 封面渐变
    accent_gradient: &'static str,
    /// 柔和彩色投影
    shadow: &'static str,
}

fn palette_colors(p: &Palette) -> PaletteColors {
    match p {
        Palette::Warm => PaletteColors {
            primary: "#E25B2A",
            secondary: "#2B7F8B",
            primary_light: "#FCE4D8",
            primary_soft: "rgba(226,91,42,0.10)",
            secondary_soft: "rgba(43,127,139,0.10)",
            bg: "#FBF7F0",
            text: "#2C2A27",
            text_secondary: "#776F63",
            accent_gradient: "linear-gradient(135deg, #E25B2A 0%, #F08A5D 100%)",
            shadow: "0 12px 40px rgba(226,91,42,0.08)",
        },
        Palette::Fresh => PaletteColors {
            primary: "#2BA39B",
            secondary: "#5B8DEF",
            primary_light: "#D9F0ED",
            primary_soft: "rgba(43,163,155,0.10)",
            secondary_soft: "rgba(91,141,239,0.10)",
            bg: "#F4FBF9",
            text: "#1A3A3A",
            text_secondary: "#5A8080",
            accent_gradient: "linear-gradient(135deg, #2BA39B 0%, #45B7D1 100%)",
            shadow: "0 12px 40px rgba(43,163,155,0.08)",
        },
        Palette::Elegant => PaletteColors {
            primary: "#9B59B6",
            secondary: "#6C5CE7",
            primary_light: "#F0E4F6",
            primary_soft: "rgba(155,89,182,0.10)",
            secondary_soft: "rgba(108,92,231,0.10)",
            bg: "#FAF7FC",
            text: "#2D2438",
            text_secondary: "#6B5C7B",
            accent_gradient: "linear-gradient(135deg, #9B59B6 0%, #6C5CE7 100%)",
            shadow: "0 12px 40px rgba(155,89,182,0.08)",
        },
        Palette::Cute => PaletteColors {
            primary: "#FF8FB1",
            secondary: "#FFC75F",
            primary_light: "#FFE4EC",
            primary_soft: "rgba(255,143,177,0.12)",
            secondary_soft: "rgba(255,199,95,0.14)",
            bg: "#FFF9FB",
            text: "#3D2030",
            text_secondary: "#8B5A75",
            accent_gradient: "linear-gradient(135deg, #FF8FB1 0%, #FFC75F 100%)",
            shadow: "0 12px 40px rgba(255,143,177,0.12)",
        },
        Palette::Cool => PaletteColors {
            primary: "#5B8DEF",
            secondary: "#6C5CE7",
            primary_light: "#E3ECFB",
            primary_soft: "rgba(91,141,239,0.10)",
            secondary_soft: "rgba(108,92,231,0.10)",
            bg: "#F6F8FF",
            text: "#1E2A47",
            text_secondary: "#5A6B8C",
            accent_gradient: "linear-gradient(135deg, #5B8DEF 0%, #6C5CE7 100%)",
            shadow: "0 12px 40px rgba(91,141,239,0.08)",
        },
        Palette::Nature => PaletteColors {
            primary: "#6B9E3F",
            secondary: "#C19A6B",
            primary_light: "#E7F1DC",
            primary_soft: "rgba(107,158,63,0.10)",
            secondary_soft: "rgba(193,154,107,0.12)",
            bg: "#F8FBF4",
            text: "#2A3820",
            text_secondary: "#5A7048",
            accent_gradient: "linear-gradient(135deg, #6B9E3F 0%, #C19A6B 100%)",
            shadow: "0 12px 40px rgba(107,158,63,0.08)",
        },
    }
}

/// HTML 转义
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// 清理自定义 HTML 片段：移除 script 标签、on* 事件属性、javascript: 协议
fn sanitize_custom_html(html: &str) -> String {
    let script_re = Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap();
    let on_attr_re = Regex::new(r#"(?i)\son\w+\s*=\s*["'][^"']*["']"#).unwrap();
    let js_proto_re = Regex::new(r"(?i)javascript:").unwrap();

    let cleaned = script_re.replace_all(html, "");
    let cleaned = on_attr_re.replace_all(&cleaned, "");
    let cleaned = js_proto_re.replace_all(&cleaned, "");
    cleaned.to_string()
}

/// 转义单引号（用于单引号包裹的 HTML 属性值，如 data-option='...'）
fn sanitize_single_quote(s: &str) -> String {
    s.replace('\'', "&#39;")
}

/// 转义 Mermaid 源码，使其作为文本安全嵌入 HTML 元素
fn sanitize_mermaid_code(s: &str) -> String {
    escape_html(s)
}

/// 构建 ECharts option JSON（bar/line/pie）
fn build_chart_option(chart_type: &str, categories: &[String], series: &[ChartSeries]) -> String {
    let ctype = match chart_type {
        "pie" => "pie",
        "line" => "line",
        _ => "bar",
    };
    let cats: Vec<String> = categories.iter().map(|c| c.clone()).collect();

    let series_value: Value = if ctype == "pie" {
        // 饼图：categories 作为标签，第一个系列的数据作为数值
        let data: Vec<Value> = cats
            .iter()
            .enumerate()
            .map(|(i, label)| {
                let value = series.first().and_then(|s| s.data.get(i)).copied().unwrap_or(0.0);
                json!({ "name": label, "value": value })
            })
            .collect();
        json!([{
            "name": series.first().map(|s| s.name.clone()).unwrap_or_default(),
            "type": "pie",
            "radius": "58%",
            "center": ["50%", "54%"],
            "label": { "formatter": "{b}: {d}%" },
            "emphasis": { "itemStyle": { "shadowBlur": 10, "shadowOffsetX": 0, "shadowColor": "rgba(0,0,0,0.2)" } },
            "data": data
        }])
    } else {
        let reliable: Vec<Value> = series
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "type": ctype,
                    "data": s.data,
                    "smooth": ctype == "line",
                    "barMaxWidth": 42,
                    "itemStyle": { "borderRadius": 6 }
                })
            })
            .collect();
        Value::Array(reliable)
    };

    let tooltip_trigger = if ctype == "pie" { "item" } else { "axis" };

    let x_axis: Value = if ctype == "pie" {
        Value::Null
    } else {
        json!({
            "type": "category",
            "data": cats,
            "axisLine": { "lineStyle": { "color": "#ccc" } },
            "axisLabel": { "fontFamily": "inherit" }
        })
    };
    let y_axis: Value = if ctype == "pie" {
        Value::Null
    } else {
        json!({
            "type": "value",
            "splitLine": { "lineStyle": { "type": "dashed", "color": "#eee" } },
            "axisLabel": { "fontFamily": "inherit" }
        })
    };

    let option = json!({
        "color": ["#FF6B6B", "#4ECDC4", "#9B59B6", "#FFC75F", "#5B8DEF", "#6B9E3F"],
        "title": { "text": "", "show": false },
        "tooltip": { "trigger": tooltip_trigger },
        "legend": { "bottom": 0, "textStyle": { "fontFamily": "inherit" } },
        "grid": {
            "left": 12, "right": 20, "top": 20, "bottom": 40,
            "containLabel": true
        },
        "xAxis": x_axis,
        "yAxis": y_axis,
        "series": series_value
    });

    serde_json::to_string(&option).unwrap_or_default()
}

/// 生成完整 CSS（编辑式玻璃拟态风格，参考落地页设计语言）
fn build_css(palette: &PaletteColors, layout: &Layout) -> String {
    let layout_max_width = match layout {
        Layout::Simple => "560px",
        Layout::Article => "760px",
        _ => "720px",
    };

    format!(
        r#"/* ===== 本地中文手写字体（随笔记目录复制，离线可用） ===== */
@font-face {{
    font-family: 'Ma Shan Zheng';
    font-style: normal;
    font-weight: 400;
    font-display: swap;
    src: url('fonts/ma-shan-zheng.woff2') format('woff2');
}}
* {{ margin: 0; padding: 0; box-sizing: border-box; }}
html {{ scroll-behavior: smooth; }}
:root {{
    --accent: {primary};
    --secondary: {secondary};
    --accent-soft: {primary_soft};
    --secondary-soft: {secondary_soft};
    --ink: {text};
    --muted: {text_secondary};
    --rule: {primary_light};
    --glass: rgba(255,255,255,0.72);
    --glass-hi: rgba(255,255,255,0.88);
    --shadow: {shadow};
    --accent-grad: {accent_gradient};
}}
body {{
    font-family: "Ma Shan Zheng", "PingFang SC", "Noto Sans CJK SC", "Microsoft YaHei", sans-serif;
    background: {bg};
    background-image:
        radial-gradient(60% 40% at 12% 8%, var(--accent-soft), transparent 60%),
        radial-gradient(50% 40% at 88% 18%, var(--secondary-soft), transparent 60%),
        radial-gradient(70% 50% at 50% 100%, var(--accent-soft), transparent 72%);
    background-attachment: fixed;
    color: {text};
    line-height: 1.75;
    -webkit-font-smoothing: antialiased;
    text-rendering: optimizeLegibility;
}}
.container {{
    max-width: {layout_max_width};
    margin: 0 auto;
    padding: 40px 20px 64px;
    position: relative;
}}

/* === 封面（渐变横幅） === */
.cover {{
    border-radius: 18px;
    padding: 52px 28px 44px;
    margin-bottom: 34px;
    background: var(--accent-grad);
    color: #fff;
    text-align: center;
    position: relative;
    overflow: hidden;
    box-shadow: var(--shadow);
}}
.cover::before {{
    content: "";
    position: absolute;
    top: -60px; right: -60px;
    width: 200px; height: 200px;
    border-radius: 50%;
    background: rgba(255,255,255,0.10);
}}
.cover::after {{
    content: "";
    position: absolute;
    bottom: -80px; left: -40px;
    width: 220px; height: 220px;
    border-radius: 50%;
    background: rgba(255,255,255,0.07);
}}
.cover-emoji {{
    font-size: 52px;
    margin-bottom: 12px;
    display: block;
    line-height: 1.2;
    position: relative;
    z-index: 1;
    filter: drop-shadow(0 2px 6px rgba(0,0,0,0.15));
}}
.cover-title {{
    font-family: "Ma Shan Zheng", "Caveat", sans-serif;
    font-size: 34px;
    font-weight: 700;
    line-height: 1.35;
    margin-bottom: 8px;
    letter-spacing: 2px;
    text-shadow: 0 2px 10px rgba(0,0,0,0.18);
    position: relative;
    z-index: 1;
}}
.cover-subtitle {{
    font-size: 17px;
    opacity: 0.92;
    font-weight: 400;
    position: relative;
    z-index: 1;
}}

/* === 内容块通用 === */
.block {{ margin-bottom: 18px; position: relative; z-index: 1; }}
.block:last-child {{ margin-bottom: 0; }}

/* === 标题（装饰符号 + 渐变线） === */
.heading {{
    display: flex;
    align-items: baseline;
    gap: 12px;
    font-family: "Ma Shan Zheng", "Caveat", sans-serif;
    font-weight: 700;
    color: {text};
    margin: 34px 0 16px;
    line-height: 1.4;
    letter-spacing: 1px;
}}
.heading::before {{
    content: "✦";
    color: {primary};
    font-size: 0.7em;
}}
.heading::after {{
    content: "";
    flex: 1;
    height: 2px;
    background: linear-gradient(90deg, {primary}, transparent);
    margin-left: 4px;
}}
.heading-1 {{ font-size: 26px; }}
.heading-2 {{ font-size: 22px; }}
.heading-3 {{ font-size: 19px; }}

/* === 段落 === */
.paragraph {{
    font-size: 15px;
    color: {text};
    line-height: 1.9;
    margin: 12px 0;
    word-break: break-word;
}}

/* === 卡片（玻璃卡片） === */
.card {{
    background: var(--glass);
    backdrop-filter: blur(10px);
    border: 1px solid var(--rule);
    border-radius: 14px;
    padding: 20px 22px;
    margin: 22px 0;
    box-shadow: var(--shadow);
}}
.card-header {{ display: flex; align-items: center; gap: 8px; margin-bottom: 10px; }}
.card-emoji {{ font-size: 22px; line-height: 1; }}
.card-title {{
    font-family: "Ma Shan Zheng", "Caveat", sans-serif;
    font-size: 19px;
    font-weight: 700;
    color: {primary};
    letter-spacing: 0.5px;
}}
.card-body {{ font-size: 15px; color: {text}; line-height: 1.85; }}

/* === 引用（左强调线） === */
.quote {{
    background: var(--glass);
    border-left: 4px solid {primary};
    border-radius: 12px;
    padding: 17px 20px;
    margin: 22px 0;
    font-size: 15px;
    color: {text_secondary};
    font-style: italic;
    line-height: 1.8;
    position: relative;
    box-shadow: var(--shadow);
}}
.quote::before {{
    content: "“";
    position: absolute;
    top: -6px;
    left: 10px;
    font-size: 44px;
    color: {primary};
    opacity: 0.22;
    font-family: Georgia, serif;
    line-height: 1;
    font-style: normal;
}}
.quote-author {{
    display: block;
    margin-top: 10px;
    font-size: 13px;
    font-weight: 600;
    color: {primary};
    font-style: normal;
}}
.quote-author::before {{ content: "— "; }}

/* === 列表 === */
.list {{ margin: 16px 0; padding-left: 2px; list-style: none; }}
.list-item {{
    position: relative;
    padding: 9px 0 9px 30px;
    font-size: 15px;
    color: {text};
    line-height: 1.75;
    border-bottom: 1px dashed var(--rule);
}}
.list-item:last-child {{ border-bottom: none; }}
.list-item::before {{
    content: "•";
    position: absolute;
    left: 4px; top: 9px;
    color: {primary};
    font-size: 18px;
}}
.list.ordered {{ counter-reset: list-counter; }}
.list.ordered .list-item {{ counter-increment: list-counter; padding-left: 34px; }}
.list.ordered .list-item::before {{
    content: counter(list-counter) ".";
    left: 0; top: 6px;
    font-weight: 700;
    font-size: 18px;
    color: {primary};
}}

/* === 标签（胶囊） === */
.tags {{ display: flex; flex-wrap: wrap; gap: 8px; margin: 20px 0; }}
.tag {{
    background: var(--accent-soft);
    color: {primary};
    font-size: 12px;
    font-weight: 700;
    padding: 3px 12px;
    border-radius: 999px;
    line-height: 1.5;
    white-space: nowrap;
}}
.tag:nth-child(even) {{
    background: var(--secondary-soft);
    color: {secondary};
}}

/* === 图片（圆角玻璃卡片） === */
.image-wrap {{
    margin: 24px auto;
    text-align: center;
    max-width: 92%;
    background: var(--glass);
    padding: 10px 10px 14px;
    box-shadow: var(--shadow);
    border-radius: 12px;
    border: 1px solid var(--rule);
}}
.image-wrap img {{ max-width: 100%; border-radius: 8px; display: block; }}
.image-caption {{
    font-size: 13px;
    color: {text_secondary};
    margin-top: 8px;
    font-style: italic;
}}

/* === 分割线 === */
.divider {{
    display: flex;
    align-items: center;
    justify-content: center;
    margin: 34px 0;
    color: {primary};
    font-size: 20px;
    gap: 14px;
}}
.divider::before, .divider::after {{
    content: "";
    flex: 1;
    height: 1px;
    background: linear-gradient(90deg, transparent, var(--rule), transparent);
    max-width: 140px;
}}
.divider-emoji {{ font-size: 22px; filter: drop-shadow(0 1px 2px rgba(0,0,0,0.08)); }}

/* === 提示框（左强调线） === */
.callout {{
    background: var(--glass);
    border: 1px solid var(--rule);
    border-left: 4px solid {primary};
    border-radius: 12px;
    padding: 16px 18px;
    margin: 22px 0;
    display: flex;
    gap: 12px;
    align-items: flex-start;
    box-shadow: var(--shadow);
}}
.callout-emoji {{ font-size: 20px; line-height: 1.4; flex-shrink: 0; }}
.callout-text {{ font-size: 15px; color: {text}; line-height: 1.8; flex: 1; }}

/* === 自定义 HTML === */
.custom {{ margin: 16px 0; border-radius: 12px; overflow: hidden; }}

/* === 数据表格（玻璃容器） === */
.nb-table-wrap {{
    margin: 24px 0;
    background: var(--glass);
    border: 1px solid var(--rule);
    border-radius: 14px;
    padding: 16px 16px 12px;
    overflow-x: auto;
    box-shadow: var(--shadow);
}}
.nb-table-caption {{
    font-size: 16px;
    font-weight: 700;
    color: {primary};
    margin-bottom: 12px;
    letter-spacing: 0.5px;
}}
.nb-table {{
    width: 100%;
    border-collapse: collapse;
    font-size: 14px;
    color: {text};
    line-height: 1.6;
}}
.nb-table th {{
    background: var(--accent-soft);
    color: {primary};
    font-weight: 700;
    text-align: left;
    padding: 10px 14px;
    font-size: 13px;
    letter-spacing: 0.5px;
    white-space: nowrap;
}}
.nb-table th:first-child {{ border-radius: 8px 0 0 8px; }}
.nb-table th:last-child {{ border-radius: 0 8px 8px 0; }}
.nb-table td {{
    padding: 9px 14px;
    border-bottom: 1px solid var(--rule);
    vertical-align: top;
}}
.nb-table tr:last-child td {{ border-bottom: none; }}
.nb-table tbody tr:nth-child(even) td {{ background: rgba(255,255,255,0.4); }}
.nb-table tbody tr:hover td {{ background: var(--accent-soft); }}

/* === 图表（ECharts） === */
.nb-chart-wrap {{
    margin: 24px 0;
    background: var(--glass);
    border: 1px solid var(--rule);
    border-radius: 14px;
    padding: 16px 16px 12px;
    box-shadow: var(--shadow);
}}
.nb-chart-title {{
    font-size: 16px;
    font-weight: 700;
    color: {primary};
    margin-bottom: 10px;
    letter-spacing: 0.5px;
}}
.nb-chart {{ width: 100%; height: 320px; }}
.nb-chart-fallback {{
    font-size: 14px;
    color: {text_secondary};
    text-align: center;
    padding: 60px 20px;
    line-height: 1.8;
}}

/* === Mermaid 流程图 === */
.nb-mermaid-wrap {{
    margin: 24px 0;
    background: var(--glass);
    border: 1px solid var(--rule);
    border-radius: 14px;
    padding: 18px 16px 12px;
    box-shadow: var(--shadow);
    overflow-x: auto;
}}
.nb-mermaid {{ display: flex; justify-content: center; }}
.nb-mermaid svg {{ max-width: 100%; }}
.nb-mermaid-caption {{
    font-size: 13px;
    color: {text_secondary};
    text-align: center;
    margin-top: 10px;
    font-style: italic;
}}
.nb-mermaid-error {{
    font-size: 14px;
    color: {text_secondary};
    text-align: center;
    padding: 40px 20px;
    line-height: 1.8;
}}

/* === 底部 === */
.footer {{
    text-align: center;
    margin-top: 48px;
    padding: 26px 20px 0;
    font-size: 14px;
    color: {text_secondary};
    position: relative;
}}
.footer::before {{
    content: "";
    position: absolute;
    top: 0;
    left: 12%;
    right: 12%;
    height: 2px;
    background: linear-gradient(90deg, transparent, {primary}, transparent);
    border-radius: 2px;
}}
.footer-tags {{
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 8px;
    margin-bottom: 16px;
}}

/* === 响应式 === */
@media (max-width: 640px) {{
    .container {{ padding: 26px 14px 48px; }}
    .cover {{ padding: 40px 18px 34px; }}
    .cover-title {{ font-size: 28px; }}
    .paragraph {{ font-size: 14.5px; }}
    .nb-table {{ font-size: 13px; }}
    .nb-table th, .nb-table td {{ padding: 8px 10px; }}
    .nb-chart {{ height: 260px; }}
    .card {{ padding: 16px 16px; }}
}}

/* === 打印样式 === */
@media print {{
    body {{
        background: #fff !important;
        background-image: none !important;
        color: #222 !important;
    }}
    .container {{ max-width: 100%; padding: 0; }}
    .cover, .card, .quote, .callout, .nb-table-wrap, .nb-chart-wrap, .nb-mermaid-wrap {{
        box-shadow: none !important;
        background: #fff !important;
        break-inside: avoid;
    }}
    .nb-chart {{ page-break-inside: avoid; }}
}}"#,
        bg = palette.bg,
        text = palette.text,
        text_secondary = palette.text_secondary,
        primary = palette.primary,
        secondary = palette.secondary,
        primary_light = palette.primary_light,
        primary_soft = palette.primary_soft,
        secondary_soft = palette.secondary_soft,
        accent_gradient = palette.accent_gradient,
        shadow = palette.shadow,
        layout_max_width = layout_max_width,
    )
}

/// 将可选块样式转换为内联 ` style="..."` 属性串（无样式时返回空串）
fn style_attr(style: &Option<BlockStyle>) -> String {
    match style {
        Some(s) => {
            let css = s.to_inline_css();
            if css.is_empty() {
                String::new()
            } else {
                format!(r#" style="{}""#, css)
            }
        }
        None => String::new(),
    }
}

/// 渲染单个 Block 为 HTML
fn render_block(block: &Block) -> String {
    match block {
        Block::Heading { text, level, style } => {
            let cls = match level {
                1 => "heading-1",
                3 => "heading-3",
                _ => "heading-2",
            };
            format!(
                r#"<div class="block"><h2 class="heading {}"{style}>{}</h2></div>"#,
                cls,
                escape_html(text),
                style = style_attr(style)
            )
        }
        Block::Paragraph { text, style } => {
            format!(
                r#"<div class="block"><p class="paragraph"{style}>{}</p></div>"#,
                escape_html(text).replace('\n', "<br>"),
                style = style_attr(style)
            )
        }
        Block::Card { title, body, emoji, style } => {
            let header = if let Some(t) = title {
                let emoji_html = emoji
                    .as_ref()
                    .map(|e| format!(r#"<span class="card-emoji">{}</span>"#, escape_html(e)))
                    .unwrap_or_default();
                format!(
                    r#"<div class="card-header">{}<span class="card-title">{}</span></div>"#,
                    emoji_html,
                    escape_html(t)
                )
            } else {
                String::new()
            };
            format!(
                r#"<div class="block"><div class="card">{}<div class="card-body"{style}>{}</div></div></div>"#,
                header,
                escape_html(body).replace('\n', "<br>"),
                style = style_attr(style)
            )
        }
        Block::Quote { text, author, style } => {
            let author_html = author
                .as_ref()
                .map(|a| format!(r#"<span class="quote-author">— {}</span>"#, escape_html(a)))
                .unwrap_or_default();
            format!(
                r#"<div class="block"><div class="quote"{style}>{}{}</div></div>"#,
                escape_html(text).replace('\n', "<br>"),
                author_html,
                style = style_attr(style)
            )
        }
        Block::List { items, ordered, style } => {
            let ordered_cls = if *ordered { " ordered" } else { "" };
            let items_html: String = items
                .iter()
                .map(|item| {
                    format!(
                        r#"<li class="list-item">{}</li>"#,
                        escape_html(item).replace('\n', "<br>")
                    )
                })
                .collect();
            format!(
                r#"<div class="block"><ul class="list{}"{style}>{}</ul></div>"#,
                ordered_cls,
                items_html,
                style = style_attr(style)
            )
        }
        Block::Tags { items } => {
            let tags_html: String = items
                .iter()
                .map(|item| format!(r#"<span class="tag">{}</span>"#, escape_html(item)))
                .collect();
            format!(r#"<div class="block"><div class="tags">{}</div></div>"#, tags_html)
        }
        Block::Image { url, caption } => {
            let caption_html = caption
                .as_ref()
                .map(|c| format!(r#"<div class="image-caption">{}</div>"#, escape_html(c)))
                .unwrap_or_default();
            format!(
                r#"<div class="block"><div class="image-wrap"><img src="{}" alt="{}" loading="lazy">{}</div></div>"#,
                escape_html(url),
                escape_html(caption.as_deref().unwrap_or("")),
                caption_html
            )
        }
        Block::Divider { emoji } => {
            let emoji_html = emoji
                .as_ref()
                .map(|e| format!(r#"<span class="divider-emoji">{}</span>"#, escape_html(e)))
                .unwrap_or_else(|| r#"<span class="divider-emoji">✿</span>"#.to_string());
            format!(r#"<div class="block"><div class="divider">{}</div></div>"#, emoji_html)
        }
        Block::Callout { text, emoji, style } => {
            let emoji_html = emoji
                .as_ref()
                .map(|e| format!(r#"<span class="callout-emoji">{}</span>"#, escape_html(e)))
                .unwrap_or_else(|| r#"<span class="callout-emoji">💡</span>"#.to_string());
            format!(
                r#"<div class="block"><div class="callout">{}<div class="callout-text"{style}>{}</div></div></div>"#,
                emoji_html,
                escape_html(text).replace('\n', "<br>"),
                style = style_attr(style)
            )
        }
        Block::Table { headers, rows, caption } => {
            let caption_html = caption
                .as_ref()
                .map(|c| format!(r#"<div class="nb-table-caption">{}</div>"#, escape_html(c)))
                .unwrap_or_default();
            let thead_html: String = headers
                .iter()
                .map(|h| format!(r#"<th>{}</th>"#, escape_html(h)))
                .collect();
            let tbody_html: String = rows
                .iter()
                .map(|row| {
                    let cells: String = row
                        .iter()
                        .map(|c| format!(r#"<td>{}</td>"#, escape_html(c).replace('\n', "<br>")))
                        .collect();
                    format!(r#"<tr>{}</tr>"#, cells)
                })
                .collect();
            format!(
                r#"<div class="block"><div class="nb-table-wrap">{caption_html}<table class="nb-table"><thead><tr>{thead_html}</tr></thead><tbody>{tbody_html}</tbody></table></div></div>"#
            )
        }
        Block::Chart { chart_type, title, categories, series } => {
            let title_html = title
                .as_ref()
                .map(|t| format!(r#"<div class="nb-chart-title">{}</div>"#, escape_html(t)))
                .unwrap_or_default();
            let chart_id = format!("nb-chart-{}", chart_id_counter::next());
            let option = build_chart_option(chart_type, categories, series);
            format!(
                r#"<div class="block"><div class="nb-chart-wrap">{title_html}<div id="{id}" class="nb-chart" data-option='{opt}'><div class="nb-chart-fallback">📊 图表「{cname}」加载中…</div></div></div></div>"#,
                id = chart_id,
                opt = sanitize_single_quote(&option),
                cname = escape_html(
                    match chart_type.as_str() {
                        "pie" => "饼图",
                        "line" => "折线图",
                        _ => "柱状图",
                    }
                ),
            )
        }
        Block::Mermaid { code, caption } => {
            let caption_html = caption
                .as_ref()
                .map(|c| format!(r#"<div class="nb-mermaid-caption">{}</div>"#, escape_html(c)))
                .unwrap_or_default();
            format!(
                r#"<div class="block"><div class="nb-mermaid-wrap"><div class="nb-mermaid">{code}</div>{caption_html}</div></div>"#,
                code = sanitize_mermaid_code(code),
            )
        }
        Block::Custom { html } => {
            format!(
                r#"<div class="block"><div class="custom">{}</div></div>"#,
                sanitize_custom_html(html)
            )
        }
    }
}

/// 渲染封面
fn render_cover(cover: &Cover, palette: &PaletteColors) -> String {
    let bg = cover
        .background
        .as_ref()
        .map(|b| b.clone())
        .unwrap_or_else(|| palette.accent_gradient.to_string());

    let emoji_html = cover
        .emoji
        .as_ref()
        .map(|e| format!(r#"<span class="cover-emoji">{}</span>"#, escape_html(e)))
        .unwrap_or_default();

    let subtitle_html = cover
        .subtitle
        .as_ref()
        .map(|s| format!(r#"<div class="cover-subtitle">{}</div>"#, escape_html(s)))
        .unwrap_or_default();

    format!(
        r#"<div class="cover" style="background: {};">{}<div class="cover-title">{}</div>{}</div>"#,
        bg,
        emoji_html,
        escape_html(&cover.title),
        subtitle_html
    )
}

/// 渲染完整 HTML 页面
pub fn render_html(note: &NoteBook) -> String {
    let palette = palette_colors(&note.palette);
    let css = build_css(&palette, &note.layout);

    let cover_html = match (&note.layout, &note.cover) {
        (Layout::Article, _) | (Layout::Simple, _) => String::new(),
        (_, Some(cover)) => render_cover(cover, &palette),
        (_, None) => String::new(),
    };

    let blocks_html: String = note.blocks.iter().map(render_block).collect();

    let has_chart = note.blocks.iter().any(|b| matches!(b, Block::Chart { .. }));
    let has_mermaid = note.blocks.iter().any(|b| matches!(b, Block::Mermaid { .. }));
    let head_scripts = build_head_scripts(has_chart, has_mermaid);

    let footer_tags: String = if note.tags.is_empty() {
        String::new()
    } else {
        let tags: String = note
            .tags
            .iter()
            .map(|t| format!(r#"<span class="tag">{}</span>"#, escape_html(t)))
            .collect();
        format!(r#"<div class="footer-tags">{}</div>"#, tags)
    };

    let date_str = chrono::DateTime::from_timestamp(note.created_at as i64, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default();

    let footer_html = format!(
        r#"<div class="footer">{}<span>📝 {}</span></div>"#,
        footer_tags, date_str
    );

    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title}</title>
<style>{css}</style>
{head_scripts}
</head>
<body>
<div class="container">
{cover}
{blocks}
{footer}
</div>
</body>
</html>"#,
        title = escape_html(&note.title),
        css = css,
        head_scripts = head_scripts,
        cover = cover_html,
        blocks = blocks_html,
        footer = footer_html,
    )
}

/// 按需生成 <head> 中的图表/流程图脚本（无对应块时返回空串）
fn build_head_scripts(has_chart: bool, has_mermaid: bool) -> String {
    let mut parts = Vec::new();
    if has_chart {
        parts.push(
            r#"<script src="https://cdn.jsdelivr.net/npm/echarts@5/dist/echarts.min.js"></script>
<script>
(function(){
  function init(){
    var els = document.querySelectorAll('.nb-chart[data-option]');
    if(!els.length) return;
    if(typeof echarts === 'undefined') return;
    els.forEach(function(el){
      var opt;
      try{ opt = JSON.parse(el.getAttribute('data-option')); }catch(e){ return; }
      var fallback = el.querySelector('.nb-chart-fallback');
      if(fallback) fallback.remove();
      var chart;
      try{ chart = echarts.init(el); }catch(e){ return; }
      chart.setOption(opt);
      window.addEventListener('resize', function(){ try{ chart.resize(); }catch(e){} });
    });
  }
  if(document.readyState === 'complete'){ init(); }
  else { window.addEventListener('load', init); }
})();
</script>"#,
        );
    }
    if has_mermaid {
        parts.push(
            r#"<script src="https://cdn.jsdelivr.net/npm/mermaid@10/dist/mermaid.min.js"></script>
<script>
(function(){
  function init(){
    var els = document.querySelectorAll('.nb-mermaid');
    if(!els.length) return;
    if(typeof mermaid === 'undefined') return;
    try{
      mermaid.initialize({ startOnLoad: false, theme: 'base', securityLevel: 'loose', fontFamily: 'inherit' });
      els.forEach(function(el){
        var codeVar = el.textContent;
        mermaid.render('mmd-' + Math.random().toString(36).slice(2,8), codeVar)
          .then(function(res){ el.innerHTML = res.svg; })
          .catch(function(){ el.innerHTML = '<div class="nb-mermaid-error">⚠️ 流程图解析失败</div>'; });
      });
    }catch(e){}
  }
  if(document.readyState === 'complete'){ init(); }
  else { window.addEventListener('load', init); }
})();
</script>"#,
        );
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_note() -> NoteBook {
        NoteBook {
            id: "note_test".into(),
            title: "测试笔记".into(),
            char_id: "vivian".into(),
            created_at: 1700000000.0,
            updated_at: 1700000000.0,
            tags: vec!["测试".into()],
            layout: Layout::CoverFlow,
            palette: Palette::Fresh,
            cover: Some(Cover {
                title: "数据总览".into(),
                subtitle: Some("一张测试封面".into()),
                emoji: Some("📊".into()),
                background: None,
            }),
            blocks: vec![
                Block::Heading { text: "标题".into(), level: 2, style: None },
                Block::Paragraph { text: "正文段落".into(), style: None },
                Block::Table {
                    headers: vec!["城市".into(), "预算".into()],
                    rows: vec![vec!["成都".into(), "1200".into()], vec!["重庆".into(), "800".into()]],
                    caption: Some("旅行预算表".into()),
                },
                Block::Chart {
                    chart_type: "bar".into(),
                    title: Some("季度销售".into()),
                    categories: vec!["Q1".into(), "Q2".into()],
                    series: vec![ChartSeries { name: "销售额".into(), data: vec![120.0, 180.0] }],
                },
                Block::Mermaid {
                    code: "graph TD\n  A[开始] --> B[结束]".into(),
                    caption: Some("流程".into()),
                },
            ],
        }
    }

    #[test]
    fn renders_new_blocks() {
        let html = render_html(&sample_note());
        // 表格
        assert!(html.contains("nb-table"), "should contain table markup");
        assert!(html.contains("<th>城市</th>"));
        assert!(html.contains("<td>1200</td>"));
        // 图表：注入 echarts CDN + 初始化脚本
        assert!(html.contains("echarts.min.js"));
        assert!(html.contains("nb-chart"));
        assert!(html.contains("data-option"));
        // 流程图：注入 mermaid CDN + 初始化脚本
        assert!(html.contains("mermaid.min.js"));
        assert!(html.contains("nb-mermaid"));
        assert!(html.contains("&gt;"), "mermaid arrow should be html-escaped");
        // 封面
        assert!(html.contains("cover-title"));
        // 标题转义
        assert!(html.contains(">标题</"));
    }

    #[test]
    fn no_scripts_when_no_chart_mermaid() {
        let mut note = sample_note();
        note.blocks = vec![Block::Paragraph { text: "只有文字".into(), style: None }];
        let html = render_html(&note);
        assert!(!html.contains("echarts.min.js"));
        assert!(!html.contains("mermaid.min.js"));
    }
}
