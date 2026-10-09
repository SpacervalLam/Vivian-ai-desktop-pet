use crate::notebook::{doc_style, renderer, storage, Block, Cover, Layout, NoteBook, Palette};

fn sample(char_id: &str) -> NoteBook {
    serde_json::from_value(serde_json::json!({
        "id": "note_design_example", "char_id": char_id,
        "title": "把周末留给生活", "created_at": 1791331200.0, "updated_at": 1791331300.0,
        "tags": ["周末", "生活记录"], "layout": "cover_flow", "palette": "warm",
        "cover": { "title": "把周末留给生活", "subtitle": "走一段慢路，读几页书。把时间花在让自己恢复的事情上。" },
        "blocks": [
            { "type": "paragraph", "text": "这周一直在赶进度，到了周末，我想把节奏稍微放慢一点。不用把两天安排得满满当当，挑两三件喜欢的事，好好做完就够了。" },
            { "type": "heading", "text": "一个不赶时间的上午", "level": 2 },
            { "type": "card", "title": "出门走走", "body": "九点半再出门，沿河边走到街角的咖啡店。\n带一本书，手机就留在包里。" },
            { "type": "card", "title": "留一点空白", "body": "午后不用预约新的活动。\n回家整理桌面，给植物浇水，听完一张专辑。" },
            { "type": "quote", "text": "有些时间，不需要用成果来证明。", "author": "写给这个周末的自己" },
            { "type": "heading", "text": "出门前的小清单", "level": 2 },
            { "type": "list", "ordered": true, "items": ["看看天气，带一件薄外套。", "装好水杯和正在读的书。", "找一家步行能到的小店，不赶着打卡。"] },
            { "type": "table", "caption": "两天的大致安排", "headers": ["时间", "周六", "周日"], "rows": [["上午", "河边散步 · 咖啡和阅读", "睡到自然醒 · 做早午餐"], ["下午", "整理房间 · 听音乐", "逛市场 · 买一束花"], ["晚上", "做一顿简单的晚饭", "看看下周安排，早点休息"]] },
            { "type": "callout", "text": "如果天气不好，就把散步换成在家读书。计划是为了舒服地生活，可以随时调整。" },
            { "type": "meta", "key": "来源", "text": "此段仅用于检索，不在页面显示。" }
        ]
    })).unwrap()
}

#[test]
fn layouts_have_a_title_even_without_a_cover_and_preserve_content_order() {
    for layout in [Layout::CoverFlow, Layout::Article, Layout::Gallery, Layout::Simple] {
        let mut note = sample("layouts");
        note.layout = layout;
        note.cover = None;
        let html = renderer::render_html(&note);
        let body = html.split("</head>").nth(1).unwrap();
        assert!(body.contains("<h1 class=\"cover-title\">把周末留给生活</h1>"));
        assert_eq!(body.matches("<h1").count(), 1);
        assert!(body.find("出门走走").unwrap() < body.find("留一点空白").unwrap());
        assert!(body.contains("<ol class=\"list ordered\""));
        assert!(body.contains("<td>河边散步 · 咖啡和阅读</td>"));
        assert!(!body.contains("此段仅用于检索"));
        assert!(!body.contains("— 写给"), "quote dash belongs to CSS, not duplicated text");
    }
}

#[test]
fn title_author_and_body_remain_escaped_with_custom_styles() {
    let mut note = sample("<img src=x onerror=alert(1)>");
    note.cover = Some(Cover { title: "<script>unsafe</script>".into(), subtitle: None, background: None });
    note.blocks = vec![Block::Paragraph { text: "<script>body</script>".into(), style: None }];
    note.custom_css = Some(":root { --ink: #123456; }".into());
    let html = renderer::render_html(&note);
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;unsafe&lt;/script&gt;"));
    assert!(html.find("--ink: #123456").unwrap() > html.find("--ink: #302B26").unwrap());
}

#[test]
fn empty_cover_background_uses_the_readable_default() {
    let mut note = sample("blank-background");
    note.cover.as_mut().unwrap().background = Some("   ".into());
    let html = renderer::render_html(&note);
    assert!(html.contains("<header class=\"cover\">"));
    assert!(!html.contains("style=\"--cover-bg:"));
}

#[test]
fn old_template_refreshes_html_without_changing_note_or_index() {
    let note = sample("migration");
    storage::save(&note).unwrap();
    let dir = crate::utils::path::get_character_data_dir("migration").join("notebook");
    let json_path = dir.join(&note.id).join("note.json");
    let index_path = dir.join("index.json");
    let html_path = dir.join(&note.id).join("note.html");
    let before_note = std::fs::read(&json_path).unwrap();
    let before_index = std::fs::read(&index_path).unwrap();
    std::fs::write(&html_path, "<html>旧模板</html>").unwrap();
    let refreshed = storage::load_html("migration", &note.id).unwrap();
    assert!(refreshed.contains(renderer::TEMPLATE_MARKER));
    assert!(refreshed.contains("出门走走"));
    assert_eq!(std::fs::read(&json_path).unwrap(), before_note);
    assert_eq!(std::fs::read(&index_path).unwrap(), before_index);
    assert_eq!(storage::load_html("migration", &note.id).unwrap(), refreshed);
    // Missing derived HTML is recoverable from the structured source.
    std::fs::remove_file(&html_path).unwrap();
    assert_eq!(storage::load_html("migration", &note.id).unwrap(), refreshed);
}

#[test]
fn summary_layout_keys_match_the_editable_note() {
    let mut note = sample("summary-layouts");
    for layout in [Layout::CoverFlow, Layout::Article, Layout::Gallery, Layout::Simple] {
        note.layout = layout;
        storage::save(&note).unwrap();
        let summary = storage::list("summary-layouts").unwrap().pop().unwrap();
        assert_eq!(summary.layout, serde_json::to_value(&note.layout).unwrap().as_str().unwrap());
    }
}

#[test]
fn original_html_notes_are_never_retemplated() {
    let html = "<!DOCTYPE html><html><body><p>自定义页面</p></body></html>";
    storage::save_raw_html("raw", "note_raw", "自定义", &[], html).unwrap();
    assert_eq!(storage::load_html("raw", "note_raw").unwrap(), html);
    assert_eq!(storage::load_html("raw", "note_raw").unwrap(), html);
}

#[test]
fn paper_notes_compose_into_a_complete_page_and_still_travel_the_raw_html_path() {
    let fragment = "<header><h1>标题</h1></header><h2>节</h2><p>正文</p>";
    let html = doc_style::compose_document("标题", fragment);
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(html.contains("<title>标题</title>"));
    assert!(html.contains("<style>"));
    assert!(html.contains(r#"<div class="wrap">"#));
    assert!(html.contains(fragment));
    assert!(!html.contains("<script"));

    // paper 笔记就是 raw_html 笔记：同一条存储与渲染通道，不能另起一套
    storage::save_raw_html("paper", "note_paper", "标题", &["学习".to_string()], &html).unwrap();
    assert!(storage::is_raw_html("paper", "note_paper"));
    assert_eq!(storage::load_html("paper", "note_paper").unwrap(), html);
    let summary = storage::list("paper").unwrap().pop().unwrap();
    assert_eq!(summary.render_type, "raw_html");
}

/// 视觉验收样本：把范式里每个构件都用一遍，人工过一眼有没有塌版。
/// 内容就是这个范式本身——样本兼作文档，不需要编造研究数据。
const PAPER_SAMPLE: &str = r##"<header>
  <h1>纸面范式速查</h1>
  <p>把「材料说的」和「我判断的」在版面上分开——分析型笔记的专业性全在这一条上。</p>
</header>
<div class="meta">内部版式约定 · 适用：精读 / 速查表 / 对照表 / 汇报材料</div>

<h2>一、先选形态，再选构件</h2>
<p class="lede">形态由材料的份数决定，不由篇幅决定：三份以上要横向比，就一定是速查表；只有一份要往下挖，才走精读。两者共用同一套构件。</p>
<div class="table-wrap">
<table>
  <thead><tr><th>形态</th><th>用在</th><th>骨架</th><th class="num">构件数</th></tr></thead>
  <tbody>
    <tr><td>速查表</td><td>3 份以上材料横向对比</td><td>坐标系 → 对照表 → 逐项卡片 → 易错清单 → 自测题 → 行动建议</td><td class="num">6</td></tr>
    <tr class="hl"><td>精读</td><td>单份材料深挖</td><td>定位 → 逐节拆解 → 图例逐元素 → 逐数字读法 → 边界 → 检查点</td><td class="num">6</td></tr>
  </tbody>
</table>
</div>

<h2>二、三种语义色，各管一件事</h2>
<div class="cards">
  <div class="card">
    <h4>callout.key</h4>
    <span class="tag">我的判断</span>
    <dl><dt>放什么</dt><dd>核心洞察、定位说明、优先级取舍</dd><dt>底色</dt><dd>浅紫 · 与材料原文不同源</dd></dl>
  </div>
  <div class="card">
    <h4>callout.warn</h4>
    <span class="tag">风险</span>
    <dl><dt>放什么</dt><dd>易错口径、证据边界、必须提醒的坑</dd><dt>底色</dt><dd>浅赭 · 一眼认出警报</dd></dl>
  </div>
  <div class="card">
    <h4>callout.ok</h4>
    <span class="tag">已证明</span>
    <dl><dt>放什么</dt><dd>材料确实支撑住了的那几条结论</dd><dt>底色</dt><dd>浅绿 · 与 warn 成对出现</dd></dl>
  </div>
</div>
<div class="callout key">
  <span class="label">我的判断</span>
  <p>这一节是全文唯一允许出现「我认为」的地方。材料里的数字进上面的表格，我的取舍留在这里的紫色块里——读者扫一眼颜色就知道哪句该信、哪句该质疑。</p>
</div>
<div class="callout ok">
  <span class="label">证明了什么</span>
  <p>在受控条件下，口径统一后两组数字的差距是可复现的；这一点材料给了两组独立对照。</p>
</div>
<div class="callout warn">
  <span class="label">没证明什么</span>
  <p>作者的对照只换了单一变量，因此「换个场景仍然成立」没有被证明。不要把这条结论外推到其他条件。</p>
</div>

<h2>三、数字口径单独成节</h2>
<p>同一个指标在不同表、摘要、正文里取值不同是常态。这类内容做成编号清单，比在正文里加个括号强得多——<strong>括号一定会被跳过</strong>。</p>
<div class="table-wrap">
<table>
  <thead><tr><th>指标</th><th>出处</th><th>参照系</th><th class="num">取值</th></tr></thead>
  <tbody>
    <tr><td>提升幅度</td><td>摘要</td><td>相对基线方案 A</td><td class="num">13.6</td></tr>
    <tr><td>提升幅度</td><td>表 3</td><td>相对基线方案 B</td><td class="num">9.2</td></tr>
    <tr class="hl"><td>提升幅度</td><td>正文 §4.2</td><td>同参数量复跑，均值</td><td class="num">11.4</td></tr>
  </tbody>
</table>
</div>

<h2>四、图例要逐个元素读</h2>
<div class="fig">
  <h4>主框图的两层「改进」</h4>
  <ol>
    <li>左侧虚线框圈的是<strong>外层</strong>：它在改「怎么找解」的那套流程本身。</li>
    <li>框内右下的小循环是<strong>内层</strong>：它在解某一道具体的题。两层同名，最容易混。</li>
    <li>从内层指向外层的实线箭头，是本文唯一的贡献点；其余箭头都是常规流程。</li>
  </ol>
</div>

<h2>五、容易踩的地方</h2>
<ol class="pitfall">
  <li><b>别用括号塞口径。</b>「+13.6（相对方案 A）」这种写法读者会漏，拆成独立编号项。</li>
  <li><b>别只给结论不给参照。</b>每个数字都要能回答「相比谁」。</li>
  <li><b>别把对照表写成装饰。</b>每一列都要能回答「所以呢」，列一多就拆成两张表。</li>
  <li><b>表内不塞长段落。</b>超过两行的解释移到表下方，用 callout 承接。</li>
</ol>

<h2>六、自测</h2>
<details><summary>为什么「事实进表格、判断进色块」比「都用正文写」更好用？</summary><p>因为读者对一段文字的信赖程度不同：材料的数据是<b>可核对</b>的，你的判断是<b>可质疑</b>的。混在一段里，质疑会被顺带带到数据上；分开排版以后，两者可以各自被接受或推翻。反例也常见：整篇都是紫色块，等于没有强调。</p></details>
<details><summary>什么情况下不该用这个范式？</summary><p>当内容本身是叙事、是感受、是给熟人看的一条记录时。这时候「事实与判断分离」只会把文章读成报告——日记式笔记该走卡片布局，不该套纸面范式。</p></details>

<h2>七、下一步</h2>
<div class="rank">
  <div class="rank-item"><span class="tier t1">先做</span><b>把口径清单一节写出来。</b>它最容易在汇报现场被追问，也最快能查完。</div>
  <div class="rank-item"><span class="tier t2">次级</span>补上「未证明什么」那一段——没有边界的结论，说服力反而更低。</div>
  <div class="rank-item"><span class="tier t3">最后做</span>配色与留白的微调。版式已经定了，这属于锦上添花。</div>
</div>
<div class="final">
  <p>一份纸面文档只回答两件事：<strong>材料证明了什么</strong>，以及<strong>我据此建议做什么</strong>。其余的段落都在为这两件事提供依据——如果某一段两件都不服务，删掉它。</p>
</div>
<footer>纸面范式 · 由笔记系统注入样式表，正文只写 .wrap 内部片段</footer>"##;

#[test]
fn export_the_paper_paradigm_sample_for_visual_review() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("previews");
    std::fs::create_dir_all(&dir).unwrap();
    let html = doc_style::compose_document("纸面范式速查", PAPER_SAMPLE);
    std::fs::write(dir.join("paper-paradigm.html"), &html).unwrap();

    // 样本必须真的把构件都用上，否则「视觉验收」验收不到东西
    for marker in [
        "callout key",
        "callout warn",
        "callout ok",
        "ol class=\"pitfall\"",
        "class=\"fig\"",
        "class=\"cards\"",
        "class=\"rank-item\"",
        "class=\"tier t1\"",
        "class=\"final\"",
        "<details>",
        "tr class=\"hl\"",
        "class=\"num\"",
        "table-wrap",
    ] {
        assert!(html.contains(marker), "样本缺少构件 {marker}");
    }
}

#[test]
fn export_production_previews_for_visual_review() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("previews");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("river.svg"), r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 440"><rect width="640" height="440" fill="#e9eee4"/><path d="M0 160 Q160 10 330 140 T640 100 V440 H0" fill="#a6b9a1"/><path d="M0 290 Q150 160 340 280 T640 210 V440 H0" fill="#7e9980"/><path d="M240 150 Q550 240 275 440 H430 Q640 230 325 150" fill="#d4e1de"/><circle cx="500" cy="80" r="32" fill="#fff5d9"/></svg>"##).unwrap();
    std::fs::write(dir.join("desk.svg"), r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 440"><rect width="640" height="440" fill="#eee4d5"/><path d="M0 300 L640 230 V440 H0" fill="#c4ae91"/><rect x="130" y="160" width="180" height="130" rx="5" fill="#fffaf0" transform="rotate(-8 220 225)"/><path d="M160 200 H275 M160 222 H265 M160 244 H250" stroke="#b9ac99" stroke-width="4"/><circle cx="430" cy="215" r="55" fill="#faf3e4"/><circle cx="430" cy="215" r="38" fill="#765e49"/><path d="M473 194 Q525 190 522 220 Q518 247 478 238" fill="none" stroke="#faf3e4" stroke-width="12"/></svg>"##).unwrap();
    for (name, layout) in [("handbook", Layout::CoverFlow), ("article", Layout::Article), ("journal", Layout::Gallery), ("short-note", Layout::Simple)] {
        let mut note = sample("vivian");
        note.layout = layout;
        if name == "journal" {
            note.blocks.insert(1, Block::Image { url: "river.svg".into(), caption: Some("河边的慢行路线".into()) });
            note.blocks.insert(2, Block::Image { url: "desk.svg".into(), caption: Some("留给阅读的一小段时间".into()) });
        }
        if name == "short-note" {
            note.title = "今天，慢一点也没关系".into();
            note.blocks.truncate(4);
        }
        std::fs::write(dir.join(format!("{name}.html")), renderer::render_html(&note)).unwrap();
    }
    for (name, palette) in [("warm", Palette::Warm), ("fresh", Palette::Fresh), ("elegant", Palette::Elegant), ("cute", Palette::Cute), ("cool", Palette::Cool), ("nature", Palette::Nature)] {
        let mut note = sample("nana");
        note.palette = palette;
        std::fs::write(dir.join(format!("palette-{name}.html")), renderer::render_html(&note)).unwrap();
    }
    let mut stress = sample("vivian");
    stress.cover = None;
    stress.title = "长标题也应该清楚地排版：把周末留给生活，而不是一张填满所有空格的计划表".into();
    stress.tags.push("averylongtagwithoutspaceswhichmustwrapwithoutoverflowing".into());
    stress.blocks.push(Block::Paragraph { text: "https://example.test/this-is-a-very-long-path-without-any-spaces-to-check-small-window-wrapping".into(), style: None });
    stress.blocks.push(Block::Table {
        headers: (1..=7).map(|i| format!("分类 {i}")).collect(),
        rows: vec![(1..=7).map(|i| format!("第 {i} 组记录")).collect()], caption: Some("宽表格保留在自己的滚动区域内".into()),
    });
    std::fs::write(dir.join("narrow-stress.html"), renderer::render_html(&stress)).unwrap();
}
