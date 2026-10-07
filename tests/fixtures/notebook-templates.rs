use crate::notebook::{renderer, storage, Block, Cover, Layout, NoteBook, Palette};

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
