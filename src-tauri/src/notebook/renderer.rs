//! 渲染引擎 - 将 NoteBook JSON + 预设 CSS 主题合成为自包含 HTML 页面
//!
//! 设计要点：
//! - CSS 主题采用纸页排版（阅读字体 / 克制配色 / 清晰层级）
//! - 6 套配色方案（warm/fresh/elegant/cute/cool/nature）
//! - 4 种布局模板（cover_flow/article/gallery/simple）
//! - 可选的自定义 CSS（`NoteBook::custom_css`）：与 palette **互斥**，
//!   有值时预设配色变量降级为兜底，自定义规则在预设 CSS 之后注入
//! - 自定义 HTML 片段经 storage::sanitize_html 清理（移除 script/on*/javascript:）
//! - 输出为自包含 HTML 文件（内联 CSS，可直接在 webview/浏览器打开）

use super::{Block, BlockStyle, ChartSeries, Cover, Layout, NoteBook, Palette, Theme};
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

/// 配色方案对应的 CSS 变量（低饱和纸页风格）
struct PaletteColors {
    /// 主强调色（标题、标签、重点）
    primary: &'static str,
    /// 次要强调色（供自定义主题使用）
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
    /// 纸页投影
    shadow: &'static str,
}

fn palette_colors(p: &Palette) -> PaletteColors {
    match p {
        Palette::Warm => PaletteColors {
            primary: "#9C4F35",
            secondary: "#53766F",
            primary_light: "#E5DDD2",
            primary_soft: "#F5EBE1",
            secondary_soft: "#EBF0EC",
            bg: "#F1EEE8",
            text: "#302B26",
            text_secondary: "#756C61",
            accent_gradient: "linear-gradient(120deg, #F4E9DB, #FBF6ED)",
            shadow: "0 8px 36px rgba(66,48,28,0.045)",
        },
        Palette::Fresh => PaletteColors {
            primary: "#277268",
            secondary: "#536D89",
            primary_light: "#D7E3DD",
            primary_soft: "#E9F3ED",
            secondary_soft: "#EAF0F5",
            bg: "#ECF1ED",
            text: "#24352F",
            text_secondary: "#63756C",
            accent_gradient: "linear-gradient(120deg, #E6F0E8, #F5F8F2)",
            shadow: "0 8px 36px rgba(29,61,45,0.045)",
        },
        Palette::Elegant => PaletteColors {
            primary: "#655477",
            secondary: "#7A6454",
            primary_light: "#E0DCE5",
            primary_soft: "#F0EBF3",
            secondary_soft: "#F2EDE7",
            bg: "#EFECF2",
            text: "#302C37",
            text_secondary: "#706877",
            accent_gradient: "linear-gradient(120deg, #EDE7F1, #FAF7FA)",
            shadow: "0 8px 36px rgba(51,35,67,0.045)",
        },
        Palette::Cute => PaletteColors {
            primary: "#A4546C",
            secondary: "#786644",
            primary_light: "#E9DCE0",
            primary_soft: "#F8ECF0",
            secondary_soft: "#F4F0E5",
            bg: "#F5EEF0",
            text: "#3B2D32",
            text_secondary: "#7D6870",
            accent_gradient: "linear-gradient(120deg, #F5E5E9, #FFF6EE)",
            shadow: "0 8px 36px rgba(86,36,55,0.045)",
        },
        Palette::Cool => PaletteColors {
            primary: "#3F6593",
            secondary: "#567A76",
            primary_light: "#DCE3EB",
            primary_soft: "#EAF0F7",
            secondary_soft: "#EAF1EE",
            bg: "#EDF0F4",
            text: "#293441",
            text_secondary: "#667280",
            accent_gradient: "linear-gradient(120deg, #E5EDF5, #F4F7FB)",
            shadow: "0 8px 36px rgba(32,50,73,0.045)",
        },
        Palette::Nature => PaletteColors {
            primary: "#596D44",
            secondary: "#88684C",
            primary_light: "#DFE3D5",
            primary_soft: "#EDF1E5",
            secondary_soft: "#F3EDE3",
            bg: "#EFF0E9",
            text: "#30362A",
            text_secondary: "#6A7260",
            accent_gradient: "linear-gradient(120deg, #E9EDDC, #F7F7EC)",
            shadow: "0 8px 36px rgba(46,57,31,0.045)",
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
        "color": ["#9C4F35", "#277268", "#655477", "#B28B50", "#3F6593", "#596D44"],
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

/// 当前模板标记。旧的结构化笔记读取时据此重新生成 HTML。
pub const TEMPLATE_MARKER: &str = r#"<meta name="vivian-notebook-template" content="2">"#;

/// 配色与布局参数 + 共用纸页样式，自定义 CSS 仍在其后注入。
fn build_css(palette: &PaletteColors, layout: &Layout) -> String {
    let page_width = match layout {
        Layout::Simple => "600px",
        Layout::Article => "780px",
        Layout::Gallery => "880px",
        Layout::CoverFlow => "820px",
    };
    format!(
        ":root {{ --accent: {primary}; --secondary: {secondary}; --accent-soft: {primary_soft}; \
         --secondary-soft: {secondary_soft}; --ink: {text}; --muted: {text_secondary}; \
         --rule: {primary_light}; --canvas: {bg}; --shadow: {shadow}; \
         --accent-grad: {accent_gradient}; --page-width: {page_width}; }}\n{template}",
        primary = palette.primary,
        secondary = palette.secondary,
        primary_soft = palette.primary_soft,
        secondary_soft = palette.secondary_soft,
        text = palette.text,
        text_secondary = palette.text_secondary,
        primary_light = palette.primary_light,
        bg = palette.bg,
        shadow = palette.shadow,
        accent_gradient = palette.accent_gradient,
        template = include_str!("templates.css"),
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
            let tag = match level {
                3 => "h3",
                _ => "h2",
            };
            format!(
                r#"<div class="block"><{tag} class="heading {}"{style}>{}</{tag}></div>"#,
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
        Block::Card { title, body, style } => {
            let header = match title {
                Some(t) => format!(
                    r#"<div class="card-header"><span class="card-title">{}</span></div>"#,
                    escape_html(t)
                ),
                None => String::new(),
            };
            format!(
                r#"<div class="block block-card"><div class="card">{}<div class="card-body"{style}>{}</div></div></div>"#,
                header,
                escape_html(body).replace('\n', "<br>"),
                style = style_attr(style)
            )
        }
        Block::Quote { text, author, style } => {
            let author_html = author
                .as_ref()
                .map(|a| format!(r#"<span class="quote-author">{}</span>"#, escape_html(a)))
                .unwrap_or_default();
            format!(
                r#"<div class="block"><blockquote class="quote"{style}>{}{}</blockquote></div>"#,
                escape_html(text).replace('\n', "<br>"),
                author_html,
                style = style_attr(style)
            )
        }
        Block::List { items, ordered, style } => {
            let ordered_cls = if *ordered { " ordered" } else { "" };
            let tag = if *ordered { "ol" } else { "ul" };
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
                r#"<div class="block"><{tag} class="list{}"{style}>{}</{tag}></div>"#,
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
                .map(|c| format!(r#"<figcaption class="image-caption">{}</figcaption>"#, escape_html(c)))
                .unwrap_or_default();
            format!(
                r#"<div class="block block-image"><figure class="image-wrap"><img src="{}" alt="{}" loading="lazy">{}</figure></div>"#,
                escape_html(url),
                escape_html(caption.as_deref().unwrap_or("")),
                caption_html
            )
        }
        Block::Divider => {
            format!(r#"<div class="block"><div class="divider"></div></div>"#)
        }
        Block::Callout { text, style } => {
            format!(
                r#"<div class="block"><div class="callout"><div class="callout-text"{style}>{}</div></div></div>"#,
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
                r#"<div class="block"><div class="nb-chart-wrap">{title_html}<div id="{id}" class="nb-chart" data-option='{opt}'><div class="nb-chart-fallback">图表「{cname}」加载中…</div></div></div></div>"#,
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
                super::storage::sanitize_html(html)
            )
        }
        // Meta 块在 render_html 里已被过滤掉；这里兜底返回空串，
        // 保证「误调用render_block(Meta) 也不会往页面里漏内容」
        Block::Meta { .. } => String::new(),
    }
}

/// 渲染封面
fn render_cover(cover: &Cover) -> String {
    // 背景走**内联 style**时优先级高于任何样式表，智能体的自定义 CSS 就覆盖不了它。
    // 所以显式自定义背景时改写成 CSS 变量，由 .cover 规则消费——
    // 这样预设 CSS 与自定义 CSS 都能通过 `.cover { background: ... }` 正常覆盖。
    let background = cover
        .background
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty());
    let bg_var = background
        .map(|b| format!(" style=\"--cover-bg: {};\"", escape_html(b)))
        .unwrap_or_default();

    let custom_class = if background.is_some() { " cover-custom" } else { "" };
    let subtitle_html = cover
        .subtitle
        .as_ref()
        .map(|s| format!(r#"<div class="cover-subtitle">{}</div>"#, escape_html(s)))
        .unwrap_or_default();

    format!(
        r#"<header class="cover{custom_class}"{bg_var}><h1 class="cover-title">{}</h1>{}</header>"#,
        escape_html(&cover.title),
        subtitle_html
    )
}

/// 渲染完整 HTML 页面
pub fn render_html(note: &NoteBook) -> String {
    // 主题只有一个来源：预设配色或自定义 CSS（互斥，见 NoteBook::theme）。
    // 预设配色变量在两种情况下都会注入，作为自定义 CSS 的兜底色板——
    // 自定义只覆盖 .card 间距而没定义颜色时，不至于拿到错误的暖橙。
    let palette = palette_colors(&note.palette);
    let base_css = build_css(&palette, &note.layout);
    let custom_css = match note.theme() {
        Theme::Custom(css) => format!("\n/* === 自定义 CSS（覆盖上方预设主题） === */\n{css}\n"),
        Theme::Preset(_) => String::new(),
    };
    let css = format!("{base_css}{custom_css}");

    let fallback_cover = Cover {
        title: note.title.clone(),
        subtitle: note.cover.as_ref().and_then(|c| c.subtitle.clone()),
        background: None,
    };
    let cover_html = match (&note.layout, &note.cover) {
        (Layout::Article, _) | (Layout::Simple, _) => render_cover(&fallback_cover),
        (_, Some(cover)) => render_cover(cover),
        (_, None) => render_cover(&fallback_cover),
    };
    let layout_class = match note.layout {
        Layout::CoverFlow => "layout-cover-flow",
        Layout::Article => "layout-article",
        Layout::Gallery => "layout-gallery",
        Layout::Simple => "layout-simple",
    };

    // Meta 块不渲染：它只进note.json 与知识库检索，页面上不出现。
    // 显式过滤而不是靠 render_block 返回空串——空串无法与「渲染意外失败」区分。
    let blocks_html: String = note
        .blocks
        .iter()
        .filter(|b| !matches!(b, Block::Meta { .. }))
        .map(render_block)
        .collect();

    let has_chart = note.blocks.iter().any(|b| matches!(b, Block::Chart { .. }));
    let has_mermaid = note.blocks.iter().any(|b| matches!(b, Block::Mermaid { .. }));
    let head_scripts = build_head_scripts(has_chart, has_mermaid);

    // 采集笔记的 tags 是**筛选用的分类标记**（如「知识采集」），不是给读者看的装饰，
    // 所以不渲染到页脚——页脚只留给手写笔记的关键词标签云。
    let show_footer_tags = !crate::notebook::collected::is_collected_note(&note.id);
    let footer_tags: String = if !show_footer_tags || note.tags.is_empty() {
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

    // 笔记不用 emoji，日期以纯文字表达：采集笔记的 created_at 是「首次采集时间」
    // （刷新时保留，见 collected.rs），手写笔记是创建时间。语义差异靠文案区分而非图标。
    let date_line = if crate::notebook::collected::is_collected_note(&note.id) {
        format!("采集于 {date_str}")
    } else {
        date_str.clone()
    };

    let author = match note.char_id.as_str() {
        "vivian" => "Vivian",
        "nana" => "Nana",
        other => other,
    };
    let meta_html = format!(
        r#"<div class="note-meta"><span class="note-author">{}</span><time datetime="{}">{}</time></div>"#,
        escape_html(author), date_str, date_line,
    );

    let footer_html = format!(
        r#"<div class="footer">{}<span>{}</span></div>"#,
        footer_tags, date_line
    );

    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
{template_marker}
<title>{title}</title>
<style>{css}</style>
{head_scripts}
</head>
<body class="{layout_class}">
<main class="container">
{meta}
{cover}
<div class="note-content">{blocks}</div>
{footer}
</main>
</body>
</html>"#,
        template_marker = TEMPLATE_MARKER,
        meta = meta_html,
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
          .catch(function(){ el.innerHTML = '<div class="nb-mermaid-error">流程图解析失败</div>'; });
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
            custom_css: None,
            cover: Some(Cover {
                title: "数据总览".into(),
                subtitle: Some("一张测试封面".into()),
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

    /// 笔记不使用 emoji：整页扫描 emoji 码位，CSS `content` 里的装饰字符也算。
    ///
    /// 这条锁住的是「约定」而非某个具体字符——历史上有两处漏网
    /// （标题装饰 `✦` 与页脚 `📅`），都是靠肉眼 review 漏掉的。
    #[test]
    fn rendered_note_contains_no_emoji() {
        let mut note = sample_note();
        note.blocks.push(Block::Divider);
        note.blocks.push(Block::Callout {
            text: "提示".into(),
            style: None,
        });
        let html = render_html(&note);

        let mut found: Vec<char> = Vec::new();
        for ch in html.chars() {
            let c = ch as u32;
            let is_emoji = (0x1F300..=0x1FAFF).contains(&c)   // 常用 pictograph
                || (0x1F000..=0x1F2FF).contains(&c)   // 麻将/扑克/杂项符号
                || (0x2600..=0x27BF).contains(&c)   // 装饰符号、 dingbat
                || c == 0xFE0F                              // 变体选择符
                || c == 0x2705
                || c == 0x274C;
            if is_emoji && !found.contains(&ch) {
                found.push(ch);
            }
        }
        assert!(found.is_empty(), "笔记渲染不应含 emoji，发现: {found:?}");
    }

    /// 采集笔记的分类标签只用于筛选，不进页脚。
    ///
    /// 断言必须定位到页脚片段再检查，不能用 `html.contains("footer-tags")` ——
    /// 那个字符串在 `<style>` 的 CSS 规则里也会出现，断言会假阳性。
    #[test]
    fn collected_note_tags_stay_out_of_the_footer() {
        use crate::notebook::collected::{build_collected_note, COLLECTED_TAG};
        let note = build_collected_note(
            "vivian",
            "mem_footer_tag",
            "douyin热梗速览",
            "整理自搜索结果。\n\n1. 梗\n用法：自嘲",
            "meme_acquisition",
        );
        assert_eq!(note.tags, vec![COLLECTED_TAG.to_string()]);
        let html = render_html(&note);
        let footer_at = html.find("class=\"footer\"").expect("应渲染页脚");
        let rest = &html[footer_at..];
        let end = rest.find("</div>").unwrap_or(0);
        let footer = &rest[..end];
        assert!(!footer.contains("class=\"tag\""), "页脚不该有标签: {footer}");

        // 反面对照：手写笔记的标签仍要渲染，否则这条测试只是在验证「都没了」
        let mut hand_written = sample_note();
        hand_written.id = "note_handwritten".into();
        hand_written.tags = vec!["手写标签".into()];
        let hand_html = render_html(&hand_written);
        let hand_footer_at = hand_html.find("class=\"footer\"").expect("应渲染页脚");
        let hand_rest = &hand_html[hand_footer_at..];
        let hand_end = hand_rest.find("</div>").unwrap_or(0);
        assert!(
            hand_rest[..hand_end].contains("class=\"tag\""),
            "手写笔记的标签标签云仍应渲染"
        );
    }

    /// `custom_css` 与 `palette` 互斥：有CSS 时自定义是唯一主题来源。
    #[test]
    fn custom_css_takes_precedence_over_palette() {
        let mut note = sample_note();
        assert!(!note.has_custom_css(), "默认走预设配色");

        note.custom_css = Some(":root { --accent: #123456; }".into());
        assert!(note.has_custom_css());
        let html = render_html(&note);
        assert!(html.contains("#123456"), "自定义 CSS 应被注入");
        // 注入在预设 CSS 之后：同优先级下后写先生效
        let custom_at = html.find("/* === 自定义 CSS").expect("应有自定义段标记");
        let preset_var_at = html.find("--accent: #277268").expect("预设变量应仍在");
        assert!(
            custom_at > preset_var_at,
            "自定义 CSS 必须排在预设 CSS 之后才能覆盖"
        );
    }

    #[test]
    fn blank_custom_css_falls_back_to_palette() {
        let mut note = sample_note();
        // 空白串等同未提供——否则 NoteBook::theme 会把空CSS 当成自定义主题，
        // 笔记就变成「没有配色也没有样式」的白板
        note.custom_css = Some("   \n\t ".into());
        assert!(!note.has_custom_css());
        assert!(matches!(note.theme(), Theme::Preset(_)));
        let html = render_html(&note);
        // 断言要精确：CSS 里别处也有「自定义 CSS」这几个字（注释说明），
        // 模糊匹配会假阳性
        assert!(
            !html.contains("/* === 自定义 CSS"),
            "空白 CSS 不该产生注入段"
        );
    }

    /// 封面自定义背景必须能被 CSS 覆盖——曾经用内联 style 导致智能体改不动。
    #[test]
    fn cover_background_is_overridable_by_css() {
        let mut note = sample_note();
        note.cover = Some(Cover {
            title: "封面".into(),
            subtitle: None,
            background: Some("#ABCDEF".into()),
        });
        // 预设 CSS 里 .cover 消费 --cover-bg（而非内联 background）
        let html = render_html(&note);
        assert!(html.contains("--cover-bg: #ABCDEF"), "背景应走 CSS 变量");
        assert!(
            html.contains("background: var(--cover-bg, var(--accent-grad))"),
            ".cover 应通过变量消费背景，自定义 CSS 才能覆盖"
        );
        assert!(
            !html.contains("class=\"cover\" style=\"background:"),
            "不应再有内联 background，否则 CSS 覆盖不了"
        );
    }
}
