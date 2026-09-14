//! 工具发现 - BM25 多字段加权搜索索引
//!
//! - 多字段加权 token 化（name/label/search_hint/summary/description/schema_key）
//! - 基于 BM25Okapi 的工具检索
//! - 支持中英文混合查询（jieba 分词）
//!
//! 用于在大量 discoverable 工具中按语义/关键字定位，按需激活。

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::Tool;

/// 字段权重
const FIELD_WEIGHTS: &[(&str, f64)] = &[
    ("name", 6.0),
    ("label", 4.0),
    ("search_hint", 3.0),
    ("summary", 2.0),
    ("description", 2.0),
    ("schema_key", 1.0),
];

/// 可发现工具描述符
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverableTool {
    pub name: String,
    pub label: String,
    pub summary: String,
    pub description: String,
    pub search_hint: String,
    pub schema_keys: Vec<String>,
    pub category: String,
    pub layer: Option<String>,
}

impl DiscoverableTool {
    /// 从 Tool trait 对象提取描述符
    pub fn from_tool(tool: &dyn Tool) -> Self {
        let schema = tool.parameters_schema();
        let schema_keys = extract_schema_keys(&schema);
        Self {
            name: tool.name().to_string(),
            label: tool.name().to_string(),
            summary: tool.description().chars().take(120).collect(),
            description: tool.description().to_string(),
            search_hint: tool.search_hint().to_string(),
            schema_keys,
            category: tool.category().as_str().to_string(),
            layer: None,
        }
    }
}

/// 从 JSON Schema 中提取顶层字段名作为 schema_key
fn extract_schema_keys(schema: &Value) -> Vec<String> {
    schema
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default()
}

/// 对文本进行 token 化（中英文混合）
///
/// - 英文：按非字母数字字符切分，统一小写
/// - 中文：使用 jieba 分词
/// - camelCase：在大小写边界处切分
fn tokenize(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }

    // 1. 在 camelCase 边界插入空格
    let split_camel = split_camel_case(text);

    // 2. 使用 jieba 分词（同时处理中英文）
    let mut tokens: Vec<String> = Vec::new();
    let words = crate::memory::tokenize::jieba().cut(&split_camel, true);
    for w in words {
        let w = w.trim();
        if w.is_empty() {
            continue;
        }
        // 英文/数字 token 小写化
        if w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            let lower = w.to_lowercase();
            if !lower.is_empty() {
                tokens.push(lower);
            }
        } else {
            // 中文 token 原样保留
            tokens.push(w.to_string());
        }
    }
    tokens
}

/// 在 camelCase 边界插入空格
fn split_camel_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut prev_lower = false;
    for ch in s.chars() {
        if ch.is_uppercase() && prev_lower {
            out.push(' ');
        }
        out.push(ch);
        prev_lower = ch.is_lowercase() || ch.is_numeric();
    }
    out
}

/// BM25 索引条目
#[derive(Debug, Clone)]
struct IndexEntry {
    /// 工具名 → 原描述符
    descriptor: DiscoverableTool,
    /// 每个字段的 token 列表
    fields: HashMap<&'static str, Vec<String>>,
}

/// BM25 搜索索引
pub struct ToolSearchIndex {
    entries: Vec<IndexEntry>,
    /// 每个字段的平均长度
    avg_field_len: HashMap<&'static str, f64>,
    /// 词项文档频率（term → 出现该 term 的文档数）
    df: HashMap<String, usize>,
    /// 文档总数
    n_docs: usize,
    /// BM25 参数
    k1: f64,
    b: f64,
}

impl ToolSearchIndex {
    /// 从可发现工具列表构建索引
    pub fn build(descriptors: Vec<DiscoverableTool>) -> Self {
        let mut entries: Vec<IndexEntry> = Vec::with_capacity(descriptors.len());
        let mut field_len_sum: HashMap<&'static str, f64> = HashMap::new();
        let mut df: HashMap<String, usize> = HashMap::new();

        for desc in descriptors {
            let mut fields: HashMap<&'static str, Vec<String>> = HashMap::new();

            for (field_name, _weight) in FIELD_WEIGHTS {
                let text = match *field_name {
                    "name" => &desc.name,
                    "label" => &desc.label,
                    "summary" => &desc.summary,
                    "description" => &desc.description,
                    "search_hint" => &desc.search_hint,
                    "schema_key" => &desc.schema_keys.join(" "),
                    _ => "",
                };
                let tokens = tokenize(text);
                *field_len_sum.entry(field_name).or_insert(0.0) += tokens.len() as f64;
                fields.insert(field_name, tokens);
            }

            // 统计文档频率：每个 term 在该文档中出现即 +1（不重复计数）
            let mut seen: HashSet<String> = HashSet::new();
            for tokens in fields.values() {
                for t in tokens {
                    if seen.insert(t.clone()) {
                        *df.entry(t.clone()).or_insert(0) += 1;
                    }
                }
            }

            entries.push(IndexEntry {
                descriptor: desc,
                fields,
            });
        }

        let n_docs = entries.len();
        let avg_field_len: HashMap<&'static str, f64> = field_len_sum
            .into_iter()
            .map(|(k, v)| (k, if n_docs > 0 { v / n_docs as f64 } else { 0.0 }))
            .collect();

        Self {
            entries,
            avg_field_len,
            df,
            n_docs,
            k1: 1.5,
            b: 0.75,
        }
    }

    /// 搜索：返回按 BM25 分数降序排列的工具名 + 分数
    pub fn search(&self, query: &str, max_results: usize) -> Vec<(&DiscoverableTool, f64)> {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() || self.n_docs == 0 {
            return Vec::new();
        }
        let query_set: HashSet<&String> = query_tokens.iter().collect();

        let mut scored: Vec<(&DiscoverableTool, f64)> = Vec::with_capacity(self.entries.len());

        for entry in &self.entries {
            let mut score = 0.0f64;
            for (field_name, weight) in FIELD_WEIGHTS {
                let field_tokens = match entry.fields.get(field_name) {
                    Some(t) => t,
                    None => continue,
                };
                let field_len = field_tokens.len() as f64;
                let avg_len = *self.avg_field_len.get(field_name).unwrap_or(&1.0);
                let avg_len = if avg_len > 0.0 { avg_len } else { 1.0 };

                // 统计每个 query token 在该字段中的出现次数
                let mut term_freq: HashMap<&String, usize> = HashMap::new();
                for t in field_tokens {
                    if query_set.contains(t) {
                        *term_freq.entry(t).or_insert(0) += 1;
                    }
                }

                for (term, freq) in &term_freq {
                    let df = *self.df.get(*term).unwrap_or(&0) as f64;
                    let idf = ((self.n_docs as f64 - df + 0.5) / (df + 0.5) + 1.0).ln();
                    let tf = *freq as f64;
                    let denom = tf + self.k1 * (1.0 - self.b + self.b * (field_len / avg_len));
                    let field_score = idf * (tf * (self.k1 + 1.0)) / denom;
                    score += field_score * weight;
                }
            }

            if score > 0.0 {
                scored.push((&entry.descriptor, score));
            }
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(max_results);
        scored
    }
}

/// 从工具集合构建可发现工具描述符列表
pub fn collect_discoverable_tools(
    tools: &[std::sync::Arc<dyn Tool>],
    active_names: &[String],
) -> Vec<DiscoverableTool> {
    let active: HashSet<&str> = active_names.iter().map(|s| s.as_str()).collect();
    tools
        .iter()
        .filter(|t| !active.contains(t.name()))
        .map(|t| DiscoverableTool::from_tool(t.as_ref()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_handles_mixed_text() {
        let tokens = tokenize("read_file 读取文件");
        assert!(!tokens.is_empty());
        // 应包含 "read" "file" 以及中文 token
        assert!(tokens.iter().any(|t| t == "read"));
        assert!(tokens.iter().any(|t| t == "file"));
    }

    #[test]
    fn split_camel_case_works() {
        // 首词在行首不插空格；大写转换只发生在小写之后（ReadFileTool → Read File Tool）
        assert_eq!(split_camel_case("ReadFileTool"), "Read File Tool");
    }

    #[test]
    fn tokenize_segments_chinese_phrase() {
        // 无空格的自然中文短语必须被切成多个词，而非整句单 token
        // （旧的整串 .contains() 打分器把整句当一个 term，导致中文查询恒 0 分）
        let tokens = tokenize("更新供应商预设");
        assert!(
            tokens.len() > 1,
            "中文短语应被 jieba 分词为多个 token，实际：{tokens:?}"
        );
        assert!(
            tokens.iter().any(|t| t == "供应商" || t == "预设"),
            "分词结果应含可命中 search_hint/description 的中文词，实际：{tokens:?}"
        );
    }

    #[test]
    fn bm25_recalls_tool_from_chinese_phrase() {
        let preset = DiscoverableTool {
            name: "update_provider_preset".into(),
            label: "update_provider_preset".into(),
            summary: "更新或新增一条 LLM 供应商预设".into(),
            description: "更新或新增一条 LLM 供应商预设（写入 llm-providers 插件数据文件）。\
                          联网核对某供应商官方 API 文档后用它落修正：过期端点、已退役模型 ID、上下文窗口。"
                .into(),
            search_hint: "provider preset 供应商 预设 厂商 verify 核对 更新预设".into(),
            schema_keys: vec!["id".into(), "providerType".into()],
            category: "system".into(),
            layer: None,
        };
        // 混入无关工具，排除「索引只有一个文档所以必中」的假阳性
        let wallpaper = DiscoverableTool {
            name: "set_wallpaper".into(),
            label: "set_wallpaper".into(),
            summary: "设置桌面壁纸".into(),
            description: "设置桌面壁纸图片".into(),
            search_hint: "wallpaper 壁纸 背景".into(),
            schema_keys: Vec::new(),
            category: "system".into(),
            layer: None,
        };
        let index = ToolSearchIndex::build(vec![preset, wallpaper]);
        let hits = index.search("更新供应商预设", 5);
        assert!(
            !hits.is_empty(),
            "自然中文短语应召回工具（旧手写打分器此处返回空）"
        );
        assert_eq!(
            hits[0].0.name, "update_provider_preset",
            "首位应命中目标工具，实际排序：{:?}",
            hits.iter().map(|(d, s)| (d.name.clone(), *s)).collect::<Vec<_>>()
        );
    }
}
