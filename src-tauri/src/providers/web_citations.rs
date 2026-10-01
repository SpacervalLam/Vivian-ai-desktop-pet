//! Normalize provider citation metadata without mixing it into structured JSON deltas.
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSource {
    pub url: String,
    pub title: String,
}
/// Append function declarations without replacing the provider's native search tools.
pub fn append_tools(body: &mut Value, declarations: &Value) {
    let Some(fields) = declarations.as_array() else {
        return;
    };
    if !body["tools"].is_array() {
        body["tools"] = serde_json::json!([]);
    }
    body["tools"]
        .as_array_mut()
        .expect("array")
        .extend(fields.iter().cloned());
}
pub fn sources(value: &Value) -> Vec<WebSource> {
    fn walk(v: &Value, out: &mut Vec<WebSource>) {
        if let Some(obj) = v.as_object() {
            if obj.get("type").and_then(Value::as_str) == Some("url_citation") {
                let citation = obj.get("url_citation").unwrap_or(v);
                if let Some(url) = citation["url"].as_str() {
                    out.push(WebSource {
                        url: url.into(),
                        title: citation["title"].as_str().unwrap_or(url).into(),
                    });
                }
            }
            if let Some(chunks) = obj.get("groundingChunks").and_then(Value::as_array) {
                for chunk in chunks {
                    if let Some(url) = chunk["web"]["uri"].as_str() {
                        out.push(WebSource {
                            url: url.into(),
                            title: chunk["web"]["title"].as_str().unwrap_or(url).into(),
                        });
                    }
                }
            }
            for (k, v) in obj {
                if k != "groundingChunks" {
                    walk(v, out);
                }
            }
        } else if let Some(arr) = v.as_array() {
            for v in arr {
                walk(v, out);
            }
        }
    }
    let mut result = vec![];
    walk(value, &mut result);
    let mut seen = std::collections::HashSet::new();
    result.retain(|s| {
        crate::network::url_fetcher::validate_url(&s.url).is_ok() && seen.insert(s.url.clone())
    });
    result
}
pub fn links(sources: &[WebSource], existing: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    let items: Vec<_> = sources
        .iter()
        .filter(|s| {
            crate::network::url_fetcher::validate_url(&s.url).is_ok()
                && !existing.contains(&s.url)
                && seen.insert(s.url.clone())
        })
        .take(20)
        .map(|s| {
            let title = s
                .title
                .replace(['[', ']', '\n', '\r'], " ")
                .chars()
                .take(100)
                .collect::<String>();
            format!(
                "[{}](<{}>)",
                title,
                s.url.replace('>', "%3E").replace('<', "%3C")
            )
        })
        .collect();
    if items.is_empty() {
        String::new()
    } else {
        format!("\n\n来源：{}", items.join("、"))
    }
}
/// Retain valid structured output by adding citations inside its text field.
pub fn attach(text: &str, sources: &[WebSource]) -> String {
    let suffix = links(sources, text);
    if suffix.is_empty() {
        return text.into();
    }
    let trimmed = text.trim();
    let json_text = if trimmed.starts_with("```") {
        trimmed
            .find('\n')
            .and_then(|start| {
                trimmed
                    .rfind("```")
                    .filter(|end| *end > start)
                    .map(|end| &trimmed[start + 1..end])
            })
            .unwrap_or(text)
    } else {
        text
    };
    if let Ok(mut value) = serde_json::from_str::<Value>(json_text) {
        if let Some(body) = value.get_mut("text") {
            if let Some(s) = body.as_str() {
                *body = Value::String(format!("{s}{suffix}"));
                return value.to_string();
            }
        }
        // Arbitrary structured data has no visible text contract; preserve JSON and metadata.
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "web_sources".into(),
                serde_json::to_value(sources).unwrap_or(Value::Null),
            );
            return value.to_string();
        }
        return text.into();
    }
    format!("{text}{suffix}")
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn web_native_search_and_function_tools_coexist() {
        for native in [json!({"type":"web_search"}), json!({"google_search":{}})] {
            let mut body = json!({"tools":[native]});
            let functions = json!([{"type":"function","name":"web_fetch"}]);
            append_tools(&mut body, &functions);
            assert_eq!(body["tools"].as_array().unwrap().len(), 2);
            assert_eq!(body["tools"][0], native);
            assert_eq!(body["tools"][1]["name"], "web_fetch");
        }
    }
    #[test]
    fn native_formats_and_untrusted_urls() {
        let s = sources(
            &json!({"annotations":[{"type":"url_citation","url":"https://example.com","title":"X"},{"type":"url_citation","url":"javascript:alert(1)"}],"groundingMetadata":{"groundingChunks":[{"web":{"uri":"https://example.com","title":"X"}}]}}),
        );
        assert_eq!(s.len(), 1);
        let v: Value =
            serde_json::from_str(&attach("{\"text\":\"答案\",\"motion\":\"idle\"}", &s)).unwrap();
        assert!(v["text"].as_str().unwrap().contains("https://example.com"));
        assert_eq!(v["motion"], "idle");
    }
}
