//! 检索来源台账：把执行阶段的真实回执收敛成一份可核对的来源索引。
//!
//! 执行阶段的工具循环是私有的，表达阶段只拿到被头尾截断的回执摘要——标题和 URL
//! 在这一步丢掉了，于是只能凭印象组织事实。本模块做两件事：
//!
//! 1. 从真实回执重建来源索引与查询实况，渲染成系统段注入表达阶段；
//! 2. 守卫表达阶段的正文：清占位符、摘编造 URL、补原文标题。
//!
//! 只陈述宿主可核对的事实，不判断相关性——相关性留给读得到标题的模型。
//! 依赖只有 `serde_json`，便于隔离验证。

use serde_json::Value;

/// 台账最多保留的来源条数。
pub const MAX_SOURCES: usize = 8;
/// 标题与发布时间进系统段前的截断长度。
const MAX_TITLE_CHARS: usize = 80;
const MAX_PUBLISHED_CHARS: usize = 24;
/// 正文尾部最多补几条来源 / 标题。
const MAX_APPENDED_SOURCES: usize = 3;

/// 一条来源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRef {
    pub title: String,
    pub url: String,
    pub published: Option<String>,
    /// bilibili / weibo / xiaohongshu …；通用网页搜索为空。
    pub platform: String,
}

/// 一条查询的实况：查了几条、有没有跑起来。不判断相关性。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryOutcome {
    pub query: String,
    pub results: usize,
    /// 检索失败是「没有证据」，不是「没有命中」。
    pub failed: Option<String>,
}

/// 宿主核验过的检索台账。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetrievalLedger {
    pub sources: Vec<SourceRef>,
    pub queries: Vec<QueryOutcome>,
    /// 没跑起来的数据源（未登录 / 接口失败）。空渠道不等于「内容不存在」。
    pub unavailable: Vec<String>,
}

impl RetrievalLedger {
    /// 从序列化后的 `ToolCallResult` 数组构建；没有检索发生则返回 `None`。
    pub fn from_receipts(receipts: &[Value]) -> Option<Self> {
        let mut ledger = Self::default();
        let mut retrieval_seen = false;
        for receipt in receipts {
            let tool = receipt.get("tool_name").and_then(Value::as_str).unwrap_or("");
            if !matches!(tool, "web_search" | "read_content" | "web_fetch" | "platform_search") {
                continue;
            }
            retrieval_seen = true;
            let raw = receipt.get("result").unwrap_or(&Value::Null);
            let data = raw.get("data").filter(|value| value.is_object()).unwrap_or(raw);
            if receipt.get("success").and_then(Value::as_bool) == Some(false) {
                let error = receipt
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("tool failed")
                    .to_string();
                let query = receipt
                    .get("arguments")
                    .and_then(|a| a.get("query"))
                    .and_then(Value::as_str)
                    .unwrap_or(tool)
                    .to_string();
                ledger.queries.push(QueryOutcome { query, results: 0, failed: Some(error) });
                ledger.absorb_unavailable(data);
                continue;
            }
            match tool {
                "web_search" | "platform_search" => {
                    ledger.absorb_search(data);
                    ledger.absorb_unavailable(data);
                }
                _ => ledger.absorb_page(data),
            }
        }
        if !retrieval_seen
            || (ledger.sources.is_empty() && ledger.queries.is_empty() && ledger.unavailable.is_empty())
        {
            return None;
        }
        Some(ledger)
    }

    fn absorb_unavailable(&mut self, data: &Value) {
        if let Some(batches) = data.get("queries").and_then(Value::as_array) {
            for batch in batches { self.absorb_unavailable(batch); }
        }
        let Some(failures) = data.get("failures").and_then(Value::as_array) else { return };
        for failure in failures {
            let platform = failure.get("platform").and_then(Value::as_str).unwrap_or("");
            let reason = failure.get("reason").and_then(Value::as_str).unwrap_or("no detail");
            let note = if platform.is_empty() { reason.to_string() } else { format!("{platform}: {reason}") };
            if !self.unavailable.contains(&note) {
                self.unavailable.push(note);
            }
        }
    }

    /// 兼容多查询批次 `{"queries":[…]}` 与单查询扁平结构。
    fn absorb_search(&mut self, data: &Value) {
        if let Some(batches) = data.get("queries").and_then(Value::as_array) {
            for batch in batches {
                self.absorb_batch(batch);
            }
        } else {
            self.absorb_batch(data);
        }
    }

    fn absorb_batch(&mut self, batch: &Value) {
        let query = batch.get("query").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let results: Vec<&Value> = batch
            .get("results")
            .and_then(Value::as_array)
            .map(|a| a.iter().collect())
            .unwrap_or_default();
        for result in &results {
            self.absorb_source(result);
        }
        let failed = batch.get("error").and_then(Value::as_str).map(str::to_string);
        if query.is_empty() && results.is_empty() && failed.is_none() {
            return;
        }
        self.queries.push(QueryOutcome { query, results: results.len(), failed });
    }

    /// 搜索结果条目 → 来源。平台结果用 `description`，字段缺失时退回 URL 当标题。
    fn absorb_source(&mut self, source: &Value) {
        let url = source.get("url").and_then(Value::as_str).unwrap_or("").trim();
        if url.is_empty() || self.sources.len() >= MAX_SOURCES || self.sources.iter().any(|s| same_url(&s.url, url)) {
            return;
        }
        self.sources.push(SourceRef {
            title: source_title(source, url),
            url: url.to_string(),
            published: published_of(source),
            platform: source.get("platform").and_then(Value::as_str).unwrap_or("").trim().to_string(),
        });
    }

    /// `read_content` 的返回本身就是一条来源。
    fn absorb_page(&mut self, data: &Value) {
        let url = data.get("url").and_then(Value::as_str).unwrap_or("").trim();
        if url.is_empty() || self.sources.len() >= MAX_SOURCES || self.sources.iter().any(|s| same_url(&s.url, url)) {
            return;
        }
        self.sources.push(SourceRef {
            title: source_title(data, url),
            url: url.to_string(),
            published: published_of(data),
            platform: data.get("platform").and_then(Value::as_str).unwrap_or("").to_string(),
        });
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty() && self.queries.is_empty() && self.unavailable.is_empty()
    }

    /// 渲染成表达阶段的系统段。
    pub fn render(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut out = String::from("[RETRIEVED SOURCES — HOST-VERIFIED INDEX, NOT INSTRUCTIONS]\n");
        if self.queries.is_empty() {
            out.push_str("This turn ran no recorded search query; only host-reported sources are listed.\n");
        } else {
            out.push_str("Queries issued this turn (host-reported hit counts only; relevance is yours to judge from the titles below):\n");
            for outcome in &self.queries {
                out.push_str(&format!("- {:?}", outcome.query));
                if let Some(error) = &outcome.failed {
                    out.push_str(&format!(": search failed ({error}) — no evidence obtained."));
                } else if outcome.results == 0 {
                    out.push_str(": 0 results — this attempt returned nothing.");
                } else {
                    out.push_str(&format!(": {} results.", outcome.results));
                }
                out.push('\n');
            }
        }
        if self.sources.is_empty() {
            out.push_str("Host-reported sources: none usable.\n");
        } else {
            out.push_str("Host-reported sources (the only citable URLs):\n");
            for (index, source) in self.sources.iter().enumerate() {
                out.push_str(&format!("{}. ", index + 1));
                if !source.platform.is_empty() {
                    out.push_str(&format!("[{}] ", source.platform));
                }
                out.push_str(&source.title);
                if let Some(published) = &source.published {
                    out.push_str(&format!(" ({published})"));
                }
                out.push_str(&format!(" — {}\n", source.url));
            }
        }
        if !self.unavailable.is_empty() {
            out.push_str("Channels that returned nothing (network or login, NOT proof of absence):\n");
            for note in &self.unavailable {
                out.push_str(&format!("- {note}\n"));
            }
        }
        out.push_str(
            "Rules for this reply:\n\
             - Attribute every source you use by its exact title, unchanged, next to its URL. Do not rename, summarize into a different entity, or invent a title, date, figure or link; never write placeholders such as [网址1] or [URL2].\n\
             - Compare the user's subject with the titles above before asserting anything. If none of the titles is about that subject — a similarly named company, product or person is a different thing — say plainly that you could not find reporting on it, and at most mention that the closest sources concern those other names.\n\
             - Do not restate a specification, performance figure or release claim unless a listed title states it.\n\
             - Platform items are posts and videos, not official statements: attribute them to their platform and author, and treat opinions as opinions.\n\
             - An empty query or channel means this attempt found nothing there, not that the content does not exist. Never present it as a general negative fact.\n\
             - When the user asked to see a source and a listed one genuinely matches, give its full URL as plain text (no markdown link syntax).\n\
             [END RETRIEVED SOURCES]",
        );
        Some(out)
    }

    /// 台账内所有 URL（归一化），用于判定正文引用是否可放行。
    pub fn urls(&self) -> Vec<String> {
        self.sources.iter().map(|s| normalize_url(&s.url)).collect()
    }
}

/// 守卫结果，只用于日志与断言。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuardReport {
    pub placeholders_removed: usize,
    pub unverified_urls: Vec<String>,
    pub sources_appended: usize,
    /// 补回的「引用了但没写原文标题」的来源数。
    pub titles_appended: usize,
}

impl GuardReport {
    pub fn touched(&self) -> bool {
        self.placeholders_removed > 0 || !self.unverified_urls.is_empty() || self.titles_appended > 0
    }
}

/// 守卫表达阶段正文。
///
/// 三件事都是机械可判定的，不涉及语义：
/// - 网址占位符清掉，有来源就用宿主来源列表替代；
/// - 不在台账、也不在历史里的 URL 摘掉（`known_urls` 传历史里出现过的链接，避免误杀）；
/// - 引用了台账 URL 却没写该来源原文标题时，把标题补回去。
pub fn guard_reply(text: &str, ledger: &RetrievalLedger, known_urls: &[String]) -> (String, GuardReport) {
    let mut report = GuardReport::default();
    let without_placeholders = strip_placeholders(text, &mut report.placeholders_removed);

    let allowed = ledger.urls();
    let known: Vec<String> = known_urls.iter().map(|u| normalize_url(u)).collect();
    let mut cleaned = String::with_capacity(without_placeholders.len());
    let mut cursor = 0;
    for (start, end) in url_spans(&without_placeholders) {
        cleaned.push_str(&without_placeholders[cursor..start]);
        let raw = &without_placeholders[start..end];
        let normalized = normalize_url(raw);
        if allowed.contains(&normalized) || known.contains(&normalized) || same_host_as_any(&normalized, &allowed) {
            cleaned.push_str(raw);
        } else {
            report.unverified_urls.push(raw.to_string());
        }
        cursor = end;
    }
    cleaned.push_str(&without_placeholders[cursor..]);

    // 占位符被清掉后可能留下悬空标点（"…可以去这两个网址瞅瞅：、"）。
    let mut text = trim_dangling(&cleaned).to_string();
    if report.placeholders_removed > 0 {
        report.sources_appended = append_sources(&mut text, ledger);
    }
    let uncited = uncited_titles(&text, ledger);
    if !uncited.is_empty() {
        append_titles(&mut text, &uncited);
        report.titles_appended = uncited.len();
    }
    (text, report)
}

/// 被引用但没交代原文标题的来源（已写出标题的不重复）。
fn uncited_titles(text: &str, ledger: &RetrievalLedger) -> Vec<String> {
    let haystack = squash(text);
    let lowered = text.to_ascii_lowercase();
    let mut titles = Vec::new();
    for source in &ledger.sources {
        if !lowered.contains(&normalize_url(&source.url)) {
            continue;
        }
        // 标题通常很长，模型截前半段引用也算交代过：取前 12 个归一化字符做判据。
        let probe: String = squash(&source.title).chars().take(12).collect();
        if probe.is_empty() || haystack.contains(&probe) || titles.contains(&source.title) {
            continue;
        }
        titles.push(source.title.clone());
        if titles.len() >= MAX_APPENDED_SOURCES {
            break;
        }
    }
    titles
}

/// 去掉空白与标点，供子串比对使用。
fn squash(text: &str) -> String {
    text.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// 文本里出现过的 URL（归一化），用于构建 `known_urls`。
pub fn urls_in_texts<'a>(texts: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut urls = Vec::new();
    for text in texts {
        for (start, end) in url_spans(text) {
            let normalized = normalize_url(&text[start..end]);
            if !normalized.is_empty() && !urls.contains(&normalized) {
                urls.push(normalized);
            }
        }
    }
    urls
}

/// URL 内允许出现 `:` `.` `?`；句子分隔符不可能属于 URL，遇到即截断。
const URL_STOP_CHARS: &[char] = &[
    '，', '。', '、', '；', '：', '）', '】', '》', '“', '”', '‘', '’', '「', '」', '！', '？', ',',
    ';', ')', ']', '}', '"', '\'', '!', '【', '（', ' ', '\n', '\t', '　',
];

/// 扫描所有 URL 片段，返回字节区间。
fn url_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(index) = find_url_start(rest) {
        let tail = &rest[index..];
        let len = tail
            .char_indices()
            .find(|(_, c)| c.is_whitespace() || URL_STOP_CHARS.contains(c))
            .map(|(i, _)| i)
            .unwrap_or(tail.len());
        let start = text.len() - rest.len() + index;
        spans.push((start, start + len));
        rest = &tail[len..];
    }
    spans
}

fn find_url_start(text: &str) -> Option<usize> {
    match (text.find("http://"), text.find("https://")) {
        (Some(http), Some(https)) => Some(http.min(https)),
        (Some(http), None) => Some(http),
        (None, Some(https)) => Some(https),
        (None, None) => None,
    }
}

fn normalize_url(url: &str) -> String {
    url.trim()
        .trim_end_matches(['.', ',', ';', ':', ')', ']', '}', '/', '。', '，'])
        .to_ascii_lowercase()
}

fn same_url(a: &str, b: &str) -> bool {
    normalize_url(a) == normalize_url(b)
}

/// 同站链接放行：正文可能引用刚抓到页面里提到的链接，删掉等于抹掉真实内容；
/// 编造的链接通常落在没检索过的域名上，按域名足以拦住。
fn same_host_as_any(url: &str, allowed: &[String]) -> bool {
    let Some(host) = url_host(url) else { return false };
    allowed.iter().filter_map(|candidate| url_host(candidate)).any(|candidate| candidate == host)
}

fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    rest.split(['/', '?', '#']).next()?.rsplit('@').next()?.split(':').next().filter(|host| !host.is_empty())
}

fn source_title(source: &Value, url: &str) -> String {
    let raw = source
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(url);
    raw.chars().take(MAX_TITLE_CHARS).collect()
}

fn published_of(source: &Value) -> Option<String> {
    source
        .get("published_at")
        .and_then(Value::as_str)
        .map(|s| s.trim().chars().take(MAX_PUBLISHED_CHARS).collect::<String>())
        .filter(|s| !s.is_empty())
}

/// 清掉网址占位符，半角/全角方括号都认。
fn strip_placeholders(text: &str, removed: &mut usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let open = chars[index];
        if matches!(open, '[' | '［' | '【') {
            let close = match open {
                '[' => ']',
                '［' => '］',
                _ => '】',
            };
            if let Some(offset) = chars[index + 1..].iter().take(12).position(|c| *c == close) {
                let inner: String = chars[index + 1..index + 1 + offset].iter().collect();
                if is_url_placeholder(&inner) {
                    *removed += 1;
                    index = index + 1 + offset + 1;
                    continue;
                }
            }
        }
        out.push(open);
        index += 1;
    }
    out
}

/// 是否为「本可以放链接、却放了占位符」的方括号片段。
///
/// 主判据是形状而非词：方括号里只要带编号（`[1]` / `[引用3]` / `[source 4]`），
/// 就是在标记「第 n 个来源」，不必逐个列举标签。只有不带编号时才看标签词，
/// 那是一个封闭的协议词汇（网址/链接/来源/原文/url/link），不是领域词。
fn is_url_placeholder(inner: &str) -> bool {
    let compact: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() || compact.chars().count() > 8 {
        return false;
    }
    if compact.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    matches!(
        compact.to_ascii_lowercase().as_str(),
        "网址" | "链接" | "来源" | "原文" | "url" | "urls" | "link" | "source"
    )
}

/// 去掉句尾因占位符移除而悬空的标点与连词。
fn trim_dangling(text: &str) -> &str {
    let mut trimmed = text.trim_end();
    loop {
        let next = trimmed.trim_end_matches(['：', ':', '、', ',', '，', '；', ';', ' ', '和', '与', '及', '\n']);
        if next.len() == trimmed.len() {
            break;
        }
        trimmed = next;
    }
    trimmed.trim_end_matches(['（', '(', '【', '[']).trim_end()
}

/// 尾部补上宿主来源；没有来源时说明没找到链接。
fn append_sources(text: &mut String, ledger: &RetrievalLedger) -> usize {
    if ledger.sources.is_empty() {
        text.push_str(match reply_language(text) {
            "en" => "\n\n(I couldn't find a usable source link for this one.)",
            "ja" => "\n\n（これについて使える出典リンクは見つからなかったよ。）",
            _ => "\n\n（这次没找到可用的原文链接。）",
        });
        return 0;
    }
    text.push_str("\n\n");
    text.push_str(match reply_language(text) {
        "en" => "Sources I actually pulled up:",
        "ja" => "実際に拾ってきた出典：",
        _ => "我实际查到的来源：",
    });
    let mut appended = 0;
    for source in ledger.sources.iter().take(MAX_APPENDED_SOURCES) {
        text.push_str(&format!("\n- {} {}", source.title, source.url));
        appended += 1;
    }
    appended
}

fn append_titles(text: &mut String, titles: &[String]) {
    if titles.is_empty() {
        return;
    }
    text.push_str("\n\n");
    text.push_str(match reply_language(text) {
        "en" => "Titles of the sources cited above, as retrieved:",
        "ja" => "上で引用した出典の実際のタイトル：",
        _ => "上面引用的来源原文标题：",
    });
    for title in titles {
        text.push_str(&format!("\n- {title}"));
    }
}

/// 按正文本身判语言。判定前先摘掉 URL——一条链接几十个拉丁字母会压过汉字。
fn reply_language(text: &str) -> &'static str {
    let without_links = without_urls(text);
    let mut cjk = 0usize;
    let mut kana = 0usize;
    let mut latin = 0usize;
    for c in without_links.chars() {
        if ('\u{3040}'..='\u{30ff}').contains(&c) {
            kana += 1;
        } else if ('\u{4e00}'..='\u{9fff}').contains(&c) {
            cjk += 1;
        } else if c.is_ascii_alphabetic() {
            latin += 1;
        }
    }
    if kana > 0 { "ja" } else if cjk > latin { "zh" } else { "en" }
}

fn without_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, end) in url_spans(text) {
        out.push_str(&text[cursor..start]);
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn unified_web_receipts_preserve_direct_pages_and_nested_channel_failures() {
        let ledger = super::RetrievalLedger::from_receipts(&[
            serde_json::json!({"tool_name":"read_content","success":true,"result":{
                "url":"https://www.bilibili.com/video/BV1234567890","title":"原始标题","platform":"bilibili"}}),
            serde_json::json!({"tool_name":"web_search","success":true,"result":{"data":{"queries":[{
                "query":"茶","results":[],"failures":[{"platform":"xiaohongshu","reason":"未登录"}]}]}}})
        ]).unwrap();
        assert_eq!(ledger.sources[0].title,"原始标题");
        assert_eq!(ledger.sources[0].platform,"bilibili");
        assert!(ledger.unavailable.iter().any(|note| note.contains("xiaohongshu") && note.contains("未登录")));
    }
    use super::*;
    use serde_json::json;

    /// 17:58 事故的最小复刻：结果全是名字相近的公司。
    fn incident_receipts() -> Vec<Value> {
        vec![json!({
            "tool_name": "web_search",
            "success": true,
            "arguments": {"queries": ["星芯科技 新芯片 发布"]},
            "result": {"data": {"queries": [{
                "query": "星芯科技 新芯片 发布",
                "count": 2,
                "results": [
                    {"url": "https://www.news.cn/auto/xinchip.html", "title": "芯驰多款新品亮相2026北京车展", "snippet": "芯驰发布AI座舱处理器"},
                    {"url": "https://finance.ifeng.com/c/yuexin", "title": "粤芯半导体登陆创业板", "snippet": "半导体行业"}
                ]
            }], "evidence_status": "UNVERIFIED"}}
        })]
    }

    /// 台账只报事实，相关性由表达阶段从标题判断。
    #[test]
    fn ledger_reports_mechanics_without_judging_relevance() {
        let ledger = RetrievalLedger::from_receipts(&incident_receipts()).expect("ledger");
        assert_eq!(ledger.sources.len(), 2);
        assert_eq!(ledger.queries[0].results, 2);
        assert!(ledger.queries[0].failed.is_none());
        let rendered = ledger.render().unwrap();
        assert!(rendered.contains(": 2 results."));
        assert!(rendered.contains("https://www.news.cn/auto/xinchip.html"));
        assert!(rendered.contains("Compare the user's subject with the titles above"));
        assert!(!rendered.contains("no listed title or snippet contains"), "宿主不做语义裁决");
    }

    #[test]
    fn single_query_flat_payload_and_dedup_are_supported() {
        let receipts = vec![json!({
            "tool_name": "web_search",
            "success": true,
            "result": {"data": {
                "query": "rust tauri",
                "count": 2,
                "results": [
                    {"url": "https://tauri.app/", "title": "Tauri", "snippet": "desktop apps"},
                    {"url": "https://tauri.app", "title": "Tauri", "snippet": "desktop apps"}
                ]
            }}
        })];
        let ledger = RetrievalLedger::from_receipts(&receipts).unwrap();
        assert_eq!(ledger.sources.len(), 1, "同一 URL 只保留一条");
        assert_eq!(ledger.queries[0].results, 2);
    }

    #[test]
    fn failed_search_is_no_evidence_not_no_match() {
        let receipts = vec![json!({
            "tool_name": "web_search",
            "success": false,
            "arguments": {"query": "星芯科技"},
            "error": "timeout"
        })];
        let ledger = RetrievalLedger::from_receipts(&receipts).unwrap();
        assert!(ledger.sources.is_empty());
        let rendered = ledger.render().unwrap();
        assert!(rendered.contains("search failed"));
        assert!(rendered.contains("no evidence obtained"));
    }

    #[test]
    fn guard_replaces_placeholders_with_host_sources() {
        let ledger = RetrievalLedger::from_receipts(&incident_receipts()).unwrap();
        let text = "我搜了下，消息挺火的。你可以去这两个网址瞅瞅：[网址1]、[网址2]。";
        let (guarded, report) = guard_reply(text, &ledger, &[]);
        assert_eq!(report.placeholders_removed, 2);
        assert!(!guarded.contains("[网址1]"));
        assert!(guarded.contains("芯驰多款新品亮相2026北京车展"));
        assert!(guarded.contains("https://www.news.cn/auto/xinchip.html"));
        assert_eq!(report.sources_appended, 2);
    }

    #[test]
    fn guard_strips_invented_urls_but_keeps_ledger_and_history_links() {
        let ledger = RetrievalLedger::from_receipts(&incident_receipts()).unwrap();
        let text = "原文在 https://fake-xinxin-chip.example.com/news 和 https://www.news.cn/auto/xinchip.html ，之前那个 https://www.bilibili.com/video/BV1xx 还留着。";
        let known = urls_in_texts(["之前发过 https://www.bilibili.com/video/BV1xx"].into_iter());
        let (guarded, report) = guard_reply(text, &ledger, &known);
        assert_eq!(report.unverified_urls, vec!["https://fake-xinxin-chip.example.com/news".to_string()]);
        assert!(guarded.contains("https://www.news.cn/auto/xinchip.html"));
        assert!(guarded.contains("https://www.bilibili.com/video/BV1xx"));
        assert!(!guarded.contains("fake-xinxin-chip"));
    }

    #[test]
    fn guard_keeps_links_from_a_retrieved_site_but_drops_unknown_hosts() {
        let ledger = RetrievalLedger::from_receipts(&incident_receipts()).unwrap();
        let text = "新闻里还提到 https://www.news.cn/auto/other-chip.html ，另见 https://chip-rumor.example.org/story 。";
        let (guarded, report) = guard_reply(text, &ledger, &[]);
        assert!(guarded.contains("https://www.news.cn/auto/other-chip.html"));
        assert_eq!(report.unverified_urls, vec!["https://chip-rumor.example.org/story".to_string()]);
    }

    #[test]
    fn platform_search_receipts_carry_platform_label_and_empty_channels() {
        let receipts = vec![json!({
            "tool_name": "platform_search",
            "success": true,
            "result": {"data": {
                "query": "星芯科技 芯片",
                "results": [{
                    "platform": "bilibili",
                    "title": "某UP聊国产芯片现状",
                    "url": "https://www.bilibili.com/video/BV1xx411c7mD",
                    "author": "某UP",
                    "published_at": "2026-10-20",
                    "description": "聊聊国产芯片和智能设备"
                }],
                "count": 1,
                "failures": [{"platform": "x", "reason": "未启用 cookie"}]
            }}
        })];
        let ledger = RetrievalLedger::from_receipts(&receipts).unwrap();
        assert_eq!(ledger.sources[0].platform, "bilibili");
        assert_eq!(ledger.unavailable, vec!["x: 未启用 cookie".to_string()]);
        let rendered = ledger.render().unwrap();
        assert!(rendered.contains("[bilibili] 某UP聊国产芯片现状"));
        assert!(rendered.contains("NOT proof of absence"));
        assert!(rendered.contains("x: 未启用 cookie"));
    }

    /// 引用 URL 却没写原文标题 → 补回标题，让错配在读者眼前显形。
    #[test]
    fn guard_restores_the_original_title_of_a_cited_source() {
        let ledger = RetrievalLedger::from_receipts(&incident_receipts()).unwrap();
        let text = "这篇讲得挺清楚的：https://www.news.cn/auto/xinchip.html";
        let (guarded, report) = guard_reply(text, &ledger, &[]);
        assert_eq!(report.titles_appended, 1);
        assert!(guarded.contains("上面引用的来源原文标题"));
        assert!(guarded.contains("芯驰多款新品亮相2026北京车展"));
    }

    #[test]
    fn guard_does_not_repeat_a_title_already_named() {
        let ledger = RetrievalLedger::from_receipts(&incident_receipts()).unwrap();
        let text = "《芯驰多款新品亮相2026北京车展》 https://www.news.cn/auto/xinchip.html 值得一看";
        let (guarded, report) = guard_reply(text, &ledger, &[]);
        assert_eq!(report.titles_appended, 0);
        assert!(!guarded.contains("上面引用的来源原文标题"));
    }

    #[test]
    fn guard_does_not_touch_non_retrieval_turns() {
        assert!(RetrievalLedger::from_receipts(&[json!({
            "tool_name": "wallpaper_set", "success": true, "result": {"data": {}}
        })])
        .is_none());
    }

    #[test]
    fn placeholder_detection_is_shape_based_not_a_wordlist() {
        let mut removed = 0;
        // 带编号的一律算占位符，标签是哪个词都不必登记。
        let out = strip_placeholders("[系统提示] 你好 [链接] [URL3] [引用2] [source 4] 结束", &mut removed);
        assert_eq!(removed, 4);
        assert!(out.contains("[系统提示]"), "四字标签不带编号不算占位符");
        // 普通方括号内容保持原样。
        let mut kept = 0;
        let out = strip_placeholders("[图片] [表情] [系统提示]", &mut kept);
        assert_eq!(kept, 0);
        assert!(out.contains("[图片]"));
    }
}
