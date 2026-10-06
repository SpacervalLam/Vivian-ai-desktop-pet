//! 自定义 CSS 能力的契约说明（供工具描述与前端提示复用）。
//!
//! 单独成文件的原因：这份「范式」要出现在三个地方——工具 schema 的字段描述、
//! 工具描述正文、前端编辑器的输入提示。写成常量字符串避免三处各写一遍而漂移。

/// 智能体可用的 CSS 类名（来自 `renderer.rs` 实际渲染的类，不含内部实现类）。
///
/// 这份清单是**从渲染器 CSS 里核对出来的**，不是设计意图——让模型只覆盖
/// 真实存在的选择器，避免它去改`.nb-chart-inner` 这类不存在的类。
pub const CSS_SELECTORS_HINT: &str = "\
可用选择器（覆盖预设主题，写在预设 CSS 之后生效）：
布局：.container（内容宽度与外边距）
封面：.cover .cover-title .cover-subtitle（封面背景用 .cover { background: ... }）
卡片：.card .card-title .card-body
文字：.heading .heading-1/2/3 .paragraph .quote .quote-author
列表：.list .list-item
强调：.callout .callout-text .tag .tags
数据：.nb-table .nb-table-wrap .nb-chart .nb-mermaid
其他：.footer .divider .image-wrap .image-caption";

/// 可覆盖的 CSS 变量（改这几个比逐条改选择器更省事）。
pub const CSS_VARS_HINT: &str = "\
--accent 主强调色 --secondary 次强调色 --ink 正文墨色 --muted 次要文字色 \
--rule 分隔线色 --accent-grad 封面渐变 --accent-soft/--secondary-soft 浅底 \
--glass/--glass-hi 玻璃底 --shadow 投影";

/// 完整范式：变量优先、选择器兜底，并给出反例。
pub const CSS_GUIDE: &str = "\
custom_css 与 palette 互斥：填了custom_css 就以它为唯一主题来源，palette 被忽略。
写法建议：先改CSS 变量做整体换色，再用选择器调排版细节——变量改动面小、不易写错。

示例（换一套深色学术风配色 + 加大标题留白）：
:root { --accent: #4A7C9B; --ink: #1C2024; --rule: #CFD8DC; --accent-grad: linear-gradient(135deg,#31576B,#5E8FA3); }
body { background: #FAFBFC; }
.heading { margin-top: 40px; letter-spacing: 0.5px; }
.card { border-radius: 6px; border-color: #E3E8EB; }

注意：
- 不要写 <style> 标签，直接给纯 CSS 片段（渲染器会自己包进 <style>）
- 不要重置 body 字号或字体族，笔记正文默认用中文手写字体（Ma Shan Zheng），换掉会失去手账质感
- .container 的 max-width 决定整页宽度（560/720/760px 随布局而定），改它会影响所有块的排版
- 需要完全自由排版（Grid、复杂布局、多页）时改用 create_html_note 自己写整份HTML";
