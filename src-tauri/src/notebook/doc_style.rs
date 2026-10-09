//! 纸面范式 —— 分析型笔记的版式与写作契约（`create_html_note` 的 `paper` 模式）。
//!
//! 与 `css_guide.rs` 的分工：那边管「结构化内容块渲染器」的自定义 CSS 片段，
//! 这边管自由 HTML 笔记里的一种**内置范式**。
//!
//! 为什么样式表由后端持有、而不是让模型每次写出来：
//! - **省 token**：整份样式表约 7KB，若写进工具 schema，每轮召回都随 prompt 付一遍；
//!   放进 Rust 常量则一次都不付。
//! - **防漂移**：模型每次重写都会漏几行、改几个色值，几十篇笔记长得各不相同；
//!   版式是产品的一部分，不该由每次调用重新发明。
//! - **可升级**：改配色只需改这一个常量，不必再教模型背新的一版。
//!
//! 代价是对应的：**已生成的笔记是固化 HTML，不会跟随本文件升级**——改范式只影响
//! 之后新建的笔记。若将来需要「历史笔记跟着升级」，得把存储格式改成「范式名 + 片段」，
//! 那是另一件事，别顺手做。
//!
//! 范式的形状：**事实进表格、判断进色块**，两者在版面上物理分开。这是这类文档
//! （精读、速查表、对照表、汇报材料）唯一的专业性来源，也是它与卡片风格笔记的分界。
//!
//! 渲染上下文：raw_html 笔记由前端以 iframe + asset 协议 URL 加载完整文档
//! （`sandbox="allow-same-origin"`，不带 `allow-scripts`）。因此 `:root` / `body`
//! 选择器天然生效，但**任何脚本都不执行**——数据可视化只能用表格与内联 SVG。

use once_cell::sync::Lazy;
use regex::Regex;

/// 纸面范式的完整样式表。
///
/// 设计约束（改动时请一并维持）：
/// - 零外部依赖：不引 CDN、不引网络字体、不引图标库
/// - 用字号/字重/留白建立层级，不靠换字体家族
/// - 无渐变、无彩色阴影、无纯装饰色块；三种语义色各有明确用途
pub const PAPER_CSS: &str = r##"/* 纸面范式 · 单页文档样式
   整份由后端注入，正文只写 .wrap 内部片段。
   零外部依赖：不引 CDN、不引网络字体、不引图标库——笔记可能离线打开、发邮件、打印。
   明色为主；需要暗色时覆盖 :root 变量（做法见文件末尾注释）。 */

:root {
  --bg: #faf9f6; --surface: #ffffff; --line: #e2e0d9;
  --text: #24231f; --muted: #6b6960; --hint: #9a978c;
  --accent: #3c3489; --accent-soft: #eeedfe;
  --warn: #993c1d; --warn-soft: #faece7;
  --ok: #0f6e56; --ok-soft: #e1f5ee;
}

* { box-sizing: border-box; }

body {
  margin: 0; background: var(--bg); color: var(--text);
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif;
  font-size: 15px; line-height: 1.78; -webkit-font-smoothing: antialiased;
}

.wrap { max-width: 860px; margin: 0 auto; padding: 48px 28px 96px; overflow-wrap: anywhere; }

img, svg { max-width: 100%; height: auto; }

/* ---------- 页头 ---------- */
header { border-bottom: 2px solid var(--text); padding-bottom: 20px; margin-bottom: 12px; }
h1 { font-size: 26px; font-weight: 600; margin: 0 0 8px; letter-spacing: -0.01em; }
header p { margin: 0; color: var(--muted); font-size: 13.5px; }
.meta { font-size: 12.5px; color: var(--hint); margin-bottom: 40px; }

/* ---------- 标题层级 ---------- */
h2 {
  font-size: 18px; font-weight: 600; margin: 52px 0 14px;
  padding-left: 12px; border-left: 3px solid var(--accent);
}
h3 { font-size: 15px; font-weight: 600; margin: 26px 0 8px; color: var(--accent); }

p { margin: 0 0 12px; }
ul, ol { margin: 0 0 12px; padding-left: 22px; }
li { margin-bottom: 6px; }
.lede { color: var(--muted); }
strong { font-weight: 600; }

/* ---------- 表格 ---------- */
.table-wrap { overflow-x: auto; margin: 16px 0 10px; }
.table-wrap table { margin: 0; }
table {
  width: 100%; border-collapse: collapse; margin: 16px 0 10px;
  font-size: 13px; background: var(--surface);
  border: 1px solid var(--line); border-radius: 10px; overflow: hidden;
}
th {
  text-align: left; font-weight: 600; padding: 10px 12px;
  background: #f2f1ec; border-bottom: 1px solid var(--line);
  font-size: 12px; color: var(--muted); white-space: nowrap;
}
td { padding: 10px 12px; border-bottom: 1px solid var(--line); vertical-align: top; }
tbody tr:last-child td { border-bottom: none; }
tbody td:first-child { font-weight: 600; }
.num { font-variant-numeric: tabular-nums; text-align: right; }
.hl { background: var(--accent-soft); }
.hl td { font-weight: 600; }
.hl td:first-child { color: var(--accent); }

/* ---------- 行内代码 ---------- */
code {
  background: #f2f1ec; padding: 1px 5px; border-radius: 4px;
  font-size: 12.5px; font-family: ui-monospace, "SF Mono", Consolas, monospace;
}

/* ---------- 强调块：三种语义（key=判断 / warn=易错 / ok=已证明） ---------- */
.callout { border-radius: 10px; padding: 16px 20px; margin: 18px 0; font-size: 14px; line-height: 1.75; }
.callout.key  { background: var(--accent-soft); }
.callout.warn { background: var(--warn-soft); }
.callout.ok   { background: var(--ok-soft); }
.callout p:last-child { margin-bottom: 0; }
.callout .label {
  display: block; font-size: 11.5px; font-weight: 600;
  letter-spacing: 0.04em; margin-bottom: 6px;
}
.callout.key  .label { color: var(--accent); }
.callout.warn .label { color: var(--warn); }
.callout.ok   .label { color: var(--ok); }

/* ---------- 图例卡（逐元素解读配图） ---------- */
.fig {
  background: var(--surface); border: 1px solid var(--line);
  border-radius: 12px; padding: 20px 24px; margin: 20px 0;
}
.fig h4 { margin: 0 0 12px; font-size: 14px; font-weight: 600; }
.fig ol { margin: 0; padding-left: 20px; font-size: 13.5px; }
.fig li { margin-bottom: 8px; }

/* ---------- 卡片网格 ---------- */
.cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(250px, 1fr)); gap: 14px; margin: 20px 0; }
.card { background: var(--surface); border: 1px solid var(--line); border-radius: 12px; padding: 18px 20px; }
.card h4 { margin: 0 0 4px; font-size: 15px; font-weight: 600; }
.card .tag {
  display: inline-block; font-size: 11.5px; padding: 2px 8px; border-radius: 20px;
  background: var(--accent-soft); color: var(--accent); margin-bottom: 10px; font-weight: 500;
}
.card dl { margin: 0; font-size: 13.5px; }
.card dt { color: var(--hint); font-size: 11.5px; margin-top: 10px; letter-spacing: 0.02em; }
.card dd { margin: 2px 0 0; line-height: 1.65; }

/* ---------- 编号风险清单 ---------- */
ol.pitfall { counter-reset: p; list-style: none; padding: 0; margin: 16px 0; }
ol.pitfall li {
  position: relative; padding: 12px 16px 12px 46px; margin-bottom: 8px;
  background: var(--warn-soft); border-radius: 8px; font-size: 13.5px; line-height: 1.7;
}
ol.pitfall li::before {
  counter-increment: p; content: counter(p);
  position: absolute; left: 16px; top: 12px;
  font-size: 12px; font-weight: 600; color: var(--warn);
  width: 20px; height: 20px; border-radius: 50%; background: #fff;
  display: flex; align-items: center; justify-content: center;
}
ol.pitfall b { color: var(--warn); }

/* ---------- 折叠问答（自测题） ---------- */
details {
  background: var(--surface); border: 1px solid var(--line);
  border-radius: 10px; padding: 13px 18px; margin-bottom: 9px;
}
details[open] { background: #fdfdfc; }
summary {
  cursor: pointer; font-weight: 500; font-size: 14px; list-style: none;
  display: flex; gap: 10px; align-items: baseline;
}
summary::-webkit-details-marker { display: none; }
summary::before { content: "Q"; font-size: 11px; font-weight: 600; color: var(--accent); flex-shrink: 0; }
details p { margin: 12px 0 0; padding-left: 20px; font-size: 13.5px; color: var(--muted); line-height: 1.78; }
details p strong { color: var(--text); }

/* ---------- 分级建议列表 ---------- */
.rank { display: grid; gap: 10px; margin: 16px 0; }
.rank-item {
  background: var(--surface); border: 1px solid var(--line);
  border-radius: 10px; padding: 14px 18px; font-size: 13.5px; line-height: 1.72;
}
.rank-item .tier {
  display: inline-block; font-size: 11px; font-weight: 600; padding: 2px 9px;
  border-radius: 20px; margin-right: 8px; vertical-align: 1px;
}
.t1 { background: var(--ok-soft); color: var(--ok); }
.t2 { background: var(--accent-soft); color: var(--accent); }
.t3 { background: #f2f1ec; color: var(--muted); }
.rank-item b { font-weight: 600; }

/* ---------- 结论块 ---------- */
.final { background: var(--accent-soft); border-radius: 12px; padding: 22px 26px; margin-top: 20px; }
.final p:last-child { margin-bottom: 0; }

/* ---------- 页脚 ---------- */
footer { margin-top: 60px; padding-top: 20px; border-top: 1px solid var(--line); font-size: 12.5px; color: var(--hint); }

/* ---------- 打印 ---------- */
@media print {
  body { background: #fff; font-size: 11pt; }
  .wrap { max-width: none; padding: 0; }
  h2 { page-break-after: avoid; break-after: avoid; }
  table, .callout, .card, .rank-item, details { page-break-inside: avoid; break-inside: avoid; }
  details { border: 1px solid #ddd; }
}

/* ---------- 暗色（可选覆盖） ----------
   需要整页暗色时，在 <style> 之后追加这段覆盖即可：
   :root { --bg:#1c1b19; --surface:#242320; --line:#3a3833; --text:#ecebe6;
           --muted:#a8a59c; --hint:#7d7a72; --accent:#cecbf6; --accent-soft:#2c2860;
           --warn:#f5c4b3; --warn-soft:#3d241a; --ok:#9fe1cb; --ok-soft:#0f3d31; }
   注意 th 与 ol.pitfall li::before 的硬编码底色也要同步改成暗色值。
   默认不给：笔记是纸面，四周的应用主题是暗的也不该把纸染黑。 */
"##;

/// 类名与结构清单（三语共用：里面的标签文案是中文，但选择器与骨架是语言无关的）。
const VOCAB: &str = "\
可用构件（只能用这几个，不要自造类名）：
· 页头 <header><h1>标题</h1><p>一句话定位</p></header>，随后可跟 <div class=\"meta\">材料来源 · 日期</div>
· 分节 <h2>节标题</h2>（左侧强调竖线）、<h3>小节</h3>、<p>段落</p>，节首的定位句用 <p class=\"lede\">灰色引导段</p>
· 对照表 <div class=\"table-wrap\"><table><thead><tr><th>…</th></tr></thead><tbody>…</tbody></table></div>
    —— 关键行加 <tr class=\"hl\">，数值单元格加 <td class=\"num\">（右对齐 + 等宽数字）
· 语义块（三种，各管一件事，颜色不同）
    <div class=\"callout key\"><span class=\"label\">我的判断</span><p>…</p></div>   我的判断、核心洞察、优先级取舍
    <div class=\"callout warn\"><span class=\"label\">没证明什么</span><p>…</p></div> 易错口径、证据边界、必须提醒的坑
    <div class=\"callout ok\"><span class=\"label\">证明了什么</span><p>…</p></div>   材料确实支撑住的结论
· 图例解读 <div class=\"fig\"><h4>图题</h4><ol><li>按左/中/右逐条说清图形元素</li></ol></div>
· 速览卡 <div class=\"cards\"><div class=\"card\"><h4>名称</h4><span class=\"tag\">标签</span><dl><dt>维度</dt><dd>取值</dd></dl></div>…</div>
· 风险清单 <ol class=\"pitfall\"><li>编号项（<b>易错处</b>自动变警示色）</li></ol>
· 自测题 <details><summary>问题</summary><p>答案要给出推理链与反例</p></details>
· 分级建议（三档，颜色不同；三行共用一个 .rank 容器，不是三个）
    <div class=\"rank\">
      <div class=\"rank-item\"><span class=\"tier t1\">先做</span>…</div>      改动点明确、对照组容易建
      <div class=\"rank-item\"><span class=\"tier t2\">次级</span>…</div>    有旁证，但结论还不确定
      <div class=\"rank-item\"><span class=\"tier t3\">最后做</span>…</div>  方向不明，或依赖前两项
    </div>
· 收尾 <div class=\"final\"><p>结论与行动建议</p></div>、<footer>页脚</footer>";

/// 写作纪律（中文）。
const RULES_ZH: &str = "\
纸面范式：系统已注入整份文档与样式表，你只写 .wrap 内部片段——不要写 <!DOCTYPE>/<html>/<head>/<body>/<style>/<link>/<script>。

写作纪律：
1. 数字口径单独成节。同一指标在不同表/摘要/正文里的不同值、每个值的参照系，做成编号清单；不要把口径塞进括号，读者会漏。
2. 事实与判断在版面上物理分开。材料的数据进表格，你的推断进 callout，颜色不同，扫一眼就分得清哪句是谁说的。
3. 不复述材料，只给增量。找矛盾（两份材料没对上的地方）、找遗漏（作者没做的那组对照）、找口径陷阱、给优先级。与原文高度重叠就是失败。
4. 两种形态：多份材料横向对比 → 速查表（坐标系 → 对照表 → 逐项卡片 → 易错清单 → 自测题 → 行动建议）；单份材料深挖 → 精读（定位 → 逐节拆解 → 图例逐元素解读 → 逐数字读法 → 证明/未证明 → 检查点）。精读的图例节末尾附一段能直接照念的 30 秒口播稿——图例精读的终点是「能对着这张图讲出来」，不是「认识每个框」。
5. 表格的每一列都要能回答「所以呢」，列一多就拆成两张表（机制一张、代价一张）；表内不塞长段落，超过两行的解释移到表下方用 callout 承接。
6. 不用 emoji，不用渐变与投影；数据可视化只用表格与内联 SVG——笔记内脚本不执行，图表库不可用。";

/// 写作纪律（日文）。
const RULES_JA: &str = "\
紙面パラダイム：文書全体とスタイルシートはシステムが注入済み。あなたは .wrap 内部の断片だけを書く——<!DOCTYPE>/<html>/<head>/<body>/<style>/<link>/<script> は書かないこと。

執筆規律：
1. 数値の口径は独立した節にする。同じ指標が表・要旨・本文で違う値になっていれば、それぞれの参照系とともに番号付きリストへ。括弧に押し込むと読者は見落とす。
2. 事実と判断は紙面上で物理的に分ける。資料のデータは表へ、あなたの推測は callout へ。色が違うので、どの文が誰のものか一目で分かる。
3. 資料を復唱せず、増分だけを出す。矛盾（二つの資料が噛み合わない点）、抜け（著者がやっていない対照）、口径の罠を探し、優先度を示す。原文と重なるなら失敗。
4. 二つの型：複数資料の横断比較 → 速査表（座標系 → 対照表 → 項目カード → 誤りやすい点 → 自測問題 → 行動提案）；単一資料の深掘り → 精読（位置づけ → 節ごとの分解 → 図の要素ごとの読み → 数値の読み方 → 証明された/されていない → チェックポイント）。精読の図の節は、そのまま読み上げられる30秒の口頭スクリプトで締める——図を精読する終点は「この図を前に説明できる」ことで、枠を一つずつ認識することではない。
5. 表の各列は「だから何か」に答えられること。列が増えたら二枚に割る。表に長い段落を入れず、二行を超える説明は表の下の callout へ。
6. emoji は使わない。グラデーションや影も使わない。可視化は表とインライン SVG のみ——ノート内のスクリプトは実行されず、チャートライブラリは使えない。";

/// 写作纪律（英文）。
const RULES_EN: &str = "\
Paper paradigm: the full document and stylesheet are provided by the system — you write only the fragment inside .wrap. Do not emit <!DOCTYPE>/<html>/<head>/<body>/<style>/<link>/<script>.

Authoring discipline:
1. Give numeric conventions their own section. When one metric carries different values in tables, abstracts and body text, list each value with its frame of reference as a numbered item instead of hiding it in parentheses — readers skip parentheses.
2. Keep facts and judgement physically separate on the page. Data from the material goes into tables; your inference goes into callouts. Different colours, so a glance tells which sentence came from whom.
3. Never restate the material; deliver only the increment. Hunt for contradictions between sources, gaps the authors never tested, traps in how numbers are framed, and rank what to do first. If the output overlaps heavily with the source, it has failed.
4. Two shapes: cross-source comparison → quick-reference sheet (axes → comparison table → per-item cards → pitfalls → self-test → actions); single-source deep dive → close reading (positioning → section-by-section → element-level figure reading → how to read each number → proved/not proved → checkpoints). Close the figure section with a 30-second script you could read aloud — the point of reading a figure closely is being able to talk through it, not recognising each box.
5. Every table column must answer \"so what?\". Too many columns means split into two tables. Never put long prose inside a table — move anything over two lines below it into a callout.
6. No emoji, no gradients, no shadows. Visualise data with tables and inline SVG only — scripts do not run inside a note and chart libraries are unavailable.";

/// 纸面范式的写作契约（拼进工具 schema 用）。
///
/// 语言码只认裸 `zh` / `ja` / `en`，与 `css_guide` 同约定；调用方传入的 BCP-47
/// 必须先经 `prompt_modules::normalize_lang` 归一，否则会静默退回英文。
pub fn guide(lang: &str) -> String {
    let rules = match lang {
        "zh" => RULES_ZH,
        "ja" => RULES_JA,
        _ => RULES_EN,
    };
    format!("{rules}\n\n{VOCAB}")
}

static BODY_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<body\b[^>]*>(.*?)</body\s*>").unwrap());
static HEAD_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<head\b[^>]*>.*?</head\s*>").unwrap());
static DOC_TAG_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)</?(?:html|body)\b[^>]*>").unwrap());

/// 把模型给的正文归一成「只含 `.wrap` 内部」的片段。
///
/// 容忍两种常见误传，而不是直接报错让模型重试一轮：整份文档被塞进来（取 `<body>` 内部）、
/// 或只多写了 `<html>`/`<body>` 外壳标签（去掉）。这两种情形的正确解释唯一且无歧义，
/// 拒绝只会白付一次往返的延迟与 token。
///
/// **不做**进一步清洗：脚本体、`on*` 属性、`javascript:` 由 `storage::sanitize_html`
/// 统一拦截（工具 validate 会在写入前比对拒绝），此处不重复一套黑名单。
pub fn normalize_fragment(fragment: &str) -> String {
    let text = fragment.trim();
    if let Some(inner) = BODY_RE.captures(text).and_then(|caps| caps.get(1)) {
        return inner.as_str().trim().to_string();
    }
    let without_head = HEAD_RE.replace_all(text, "");
    DOC_TAG_RE.replace_all(&without_head, "").trim().to_string()
}

/// 把正文片段合成完整文档：注入样式表 + `.wrap` 容器。
///
/// 标题在这里只用于 `<title>`（浏览器标签/打印页眉）；页面上的大标题由片段自己的
/// `<header><h1>` 承担——段落的版式归写作者，别让后端猜它想怎么排。
pub fn compose_document(title: &str, fragment: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title}</title>
<style>{css}</style>
</head>
<body>
<div class="wrap">
{fragment}
</div>
</body>
</html>"#,
        title = escape_html(title),
        css = PAPER_CSS,
        fragment = fragment.trim(),
    )
}

/// 与 `renderer::escape_html` 同样的最小转义。刻意不复用：那是渲染器的私有实现，
/// 两处转义口径要能各自独立演进（这里只护 `<title>`，不涉及属性上下文）。
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_carries_the_stylesheet_and_the_wrap() {
        let html = compose_document("速查表", "<header><h1>速查表</h1></header>");
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<title>速查表</title>"));
        assert!(html.contains("--accent: #3c3489"));
        assert!(html.contains(r#"<div class="wrap">"#));
        assert!(html.contains("<header><h1>速查表</h1></header>"));
    }

    #[test]
    fn title_is_escaped_but_fragment_is_left_alone() {
        let html = compose_document("<script>x</script>", "<p>正文</p>");
        assert!(html.contains("&lt;script&gt;x&lt;/script&gt;"));
        assert!(!html.contains("<script>"));
        // 片段的清洗归 sanitize_html 管，这里不该顺手改正文
        assert!(html.contains("<p>正文</p>"));
    }

    #[test]
    fn a_whole_document_pasted_into_paper_mode_is_reduced_to_its_body() {
        let pasted = "<!DOCTYPE html><html><head><style>p{color:red}</style></head><body><h2>节</h2><p>正文</p></body></html>";
        let fragment = normalize_fragment(pasted);
        assert!(fragment.contains("<h2>节</h2>"));
        assert!(!fragment.contains("<body"));
        assert!(!fragment.contains("<!DOCTYPE"));
    }

    #[test]
    fn bare_shell_tags_are_dropped_from_the_fragment() {
        let fragment = normalize_fragment("<html><body><p>只有外壳</p></body></html>");
        assert_eq!(fragment, "<p>只有外壳</p>");
        assert_eq!(normalize_fragment("  <p>已修剪</p>  "), "<p>已修剪</p>");
    }

    #[test]
    fn the_vocabulary_the_guide_promises_exists_in_the_stylesheet() {
        // 契约与样式表必须同源：写进提示的构件若在 CSS 里没有，模型写出来的就是死标签。
        // 两侧的写法天然不同——样式表是选择器（`.callout.key`），契约是给模型看的
        // HTML（`class="callout key"`），所以逐对断言，而不是拿同一个字符串比两次。
        let pairs: &[(&str, &str)] = &[
            (".callout.key", "callout key"),
            (".callout.warn", "callout warn"),
            (".callout.ok", "callout ok"),
            (".callout .label", "class=\"label\""),
            ("ol.pitfall", "class=\"pitfall\""),
            (".table-wrap", "table-wrap"),
            (".rank-item", "rank-item"),
            // t2/t3 在契约里只以文字形态出现（`t1 先做 / t2 次级 / t3 最后做`）：
            // 三个 span 全写开会让构件清单变长，而 class="tier t1" 已经示范了写法。
            (".rank-item .tier", "tier t1"),
            (".t1", "t1"),
            (".t2", "t2"),
            (".t3", "t3"),
            (".final", "class=\"final\""),
            (".fig", "class=\"fig\""),
            (".cards", "class=\"cards\""),
            (".card .tag", "class=\"tag\""),
            (".hl", "\"hl\""),
            (".num", "\"num\""),
            (".meta", "class=\"meta\""),
            (".lede", "class=\"lede\""),
            ("details", "details"),
            ("summary", "summary"),
            ("footer", "footer"),
        ];
        for (selector, token) in pairs {
            assert!(PAPER_CSS.contains(selector), "样式表缺少 {selector}");
            assert!(guide("zh").contains(token), "契约缺少 {token}");
        }
        // 三种语言都要挂上同一份构件清单（VOCAB 三语共用，规则各自一份）：
        // 缺一份规则就会静默退回英文，而英文规则配中文骨架是最难自查的一种错。
        for lang in ["zh", "ja", "en"] {
            assert!(guide(lang).contains("callout"), "{lang} 契约不完整");
            assert!(guide(lang).contains("<table"), "{lang} 契约缺少对照表构件");
            assert!(guide(lang).contains("wrap"), "{lang} 契约缺少 .wrap 边界说明");
        }
    }
}
