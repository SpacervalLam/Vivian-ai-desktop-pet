//! web_search 的站内检索分支；平台适配器不再单独暴露为模型工具。
use super::{platform_search_policy, platform_search_tool::PlatformSearchTool};
use crate::network::web::{
    read_search_config, WebSearchRequest, WebSearchService, WebSearchSource,
};
use crate::tools::types::{Tool, ToolResult, ToolUseContext};
use serde_json::{json, Value};

pub fn validate(args: &Value) -> Result<(), String> {
    if let Some(platforms) = args.get("platforms") {
        let platforms = platforms
            .as_array()
            .filter(|p| !p.is_empty() && p.len() <= 3)
            .ok_or("platforms 必须包含 1–3 个平台")?;
        let names: Vec<String> = platforms
            .iter()
            .map(|p| {
                p.as_str()
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_string)
                    .ok_or("平台名称必须是非空字符串")
            })
            .collect::<Result<_, _>>()?;
        platform_search_policy::requested(&names)?;
        let queries = args["queries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .chain(args["query"].as_str());
        if queries
            .into_iter()
            .any(|q| q.chars().count() > platform_search_policy::MAX_QUERY_CHARS)
        {
            return Err("站内查询词最多 200 字".into());
        }
    }
    if args.get("include_web").is_some_and(|v| !v.is_boolean()) {
        return Err("include_web 必须是布尔值".into());
    }
    if args["include_web"].as_bool() == Some(false) && args.get("platforms").is_none() {
        return Err("include_web=false 时必须指定 platforms".into());
    }
    Ok(())
}

pub async fn search(args: &Value, ctx: &ToolUseContext, queries: Vec<String>) -> ToolResult {
    let (config, proxy) = read_search_config();
    let max = args["max_results"].as_u64().unwrap_or(5) as usize;
    let timeout = args["timeout_secs"].as_u64().unwrap_or(40);
    let config_ref = &config;
    let proxy_ref = &proxy;
    let calls = queries.iter().map(|query| async move {
        let mut payload = json!({"query":query,"results":[],"per_platform":{},"failures":[]});
        let work = async {
            let platform_args =
                json!({"query":query,"platforms":args["platforms"],"limit":max.min(10)});
            let platform = PlatformSearchTool.call(platform_args, ctx);
            let web = async {
                if !args["include_web"].as_bool().unwrap_or(false) {
                    return None;
                }
                let mut request = WebSearchRequest::new(query).with_max_results(max);
                request.include_domains = strings(args, "include_domains");
                request.exclude_domains = strings(args, "exclude_domains");
                request.recency_days = args["recency_days"].as_u64().map(|n| n as u32);
                request.engines = args.get("engines").map(|_| strings(args, "engines"));
                request.research = args["mode"]
                    .as_str()
                    .map(|m| m == "research")
                    .unwrap_or(ctx.is_work_agent());
                request.timeout_secs = timeout;
                request.refresh = args["refresh"].as_bool().unwrap_or(false);
                request.language = args["language"].as_str().map(str::to_string);
                request.country = args["country"].as_str().map(str::to_string);
                Some(
                    WebSearchService::shared()
                        .search(&request, config_ref.as_ref(), proxy_ref.as_deref())
                        .await,
                )
            };
            let (platform, web) = tokio::join!(platform, web);
            let data = platform
                .data
                .as_ref()
                .and_then(|value| value.get("data"))
                .cloned()
                .unwrap_or(Value::Null);
            payload["results"] = data["results"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into();
            payload["per_platform"] = data["per_platform"]
                .as_object()
                .cloned()
                .unwrap_or_default()
                .into();
            payload["failures"] = data["failures"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into();
            if let Some(web) = web {
                match web {
                    Ok(result) => {
                        payload["per_platform"]["web"] = json!(result.sources.len());
                        payload["results"].as_array_mut().unwrap().extend(
                            result
                                .sources
                                .into_iter()
                                .filter_map(|source| serde_json::to_value(source).ok()),
                        );
                    }
                    Err(error) => payload["failures"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"platform":"web","reason":error.to_string()})),
                }
            }
        };
        if tokio::time::timeout(std::time::Duration::from_secs(timeout), work)
            .await
            .is_err()
        {
            payload["failures"]
                .as_array_mut()
                .unwrap()
                .push(json!({"platform":"platforms","reason":"查询超时，未取得完整结果"}));
        }
        finalize(&mut payload, args, max, ctx);
        payload
    });
    let batches = futures::future::join_all(calls).await;
    let all_failed = batches.iter().all(|batch| {
        batch["per_platform"]
            .as_object()
            .is_none_or(|p| p.is_empty())
    });
    let partial = batches
        .iter()
        .any(|batch| batch["failures"].as_array().is_some_and(|p| !p.is_empty()));
    let mut payload = if batches.len() == 1 {
        batches[0].clone()
    } else {
        json!({"queries":batches})
    };
    payload["partial_failure"] = json!(partial);
    payload["evidence_status"] = json!("UNVERIFIED");
    payload["hint"] = json!("站内检索命中只是线索；使用 read_content(url) 读取正文、字幕与评论，include=[\"danmaku\"] 读取弹幕。失败和零命中均不代表平台上没有相关内容。");
    if all_failed {
        ToolResult::standard_error("平台检索未成功", Some("SearchFailed"), Some(payload))
    } else {
        ToolResult::standard_success("查询完成；来源摘要仍需读取原文核验", Some(payload))
    }
}

fn strings(args: &Value, key: &str) -> Vec<String> {
    args[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|s| s.trim().to_ascii_lowercase())
        .collect()
}

fn finalize(payload: &mut Value, args: &Value, max: usize, ctx: &ToolUseContext) {
    let include = strings(args, "include_domains");
    let exclude = strings(args, "exclude_domains");
    let store = crate::network::web_evidence::EvidenceStore::for_session(
        &ctx.session_id,
        &ctx.char_id,
        &ctx.user_id,
    );
    let mut seen = std::collections::HashSet::new();
    let results = payload["results"].as_array_mut().unwrap();
    results.retain(|result| {
        let url = result["url"].as_str().unwrap_or("");
        !url.is_empty()
            && seen.insert(url.to_string())
            && (include.is_empty()
                || crate::network::web::providers::util::domain_matches(url, &include))
            && !crate::network::web::providers::util::domain_matches(url, &exclude)
            && args["recency_days"].as_u64().is_none_or(|days| {
                result["published_at"]
                    .as_str()
                    .and_then(|date| {
                        chrono::DateTime::parse_from_rfc3339(date)
                            .ok()
                            .map(|d| d.timestamp())
                            .or_else(|| {
                                chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                                    .ok()
                                    .and_then(|d| d.and_hms_opt(0, 0, 0))
                                    .map(|d| d.and_utc().timestamp())
                            })
                    })
                    .is_none_or(|time| time >= chrono::Utc::now().timestamp() - days as i64 * 86400)
            })
    });
    *results = fair_limit(std::mem::take(results), max);
    for result in results.iter_mut() {
        let mut source = WebSearchSource::new(result["url"].as_str().unwrap_or(""));
        source.title = result["title"].as_str().map(str::to_string);
        source.snippet = result["snippet"]
            .as_str()
            .or(result["description"].as_str())
            .map(str::to_string);
        source.published_at = result["published_at"].as_str().map(str::to_string);
        source.source_id = crate::network::web::providers::util::source_id(&source.url);
        source.retrieved_at = chrono::Utc::now().to_rfc3339();
        result["source_id"] = json!(source.source_id);
        result["snippet"] = json!(source.snippet);
        result["retrieved_at"] = json!(source.retrieved_at);
        if let Err(error) = store.save_source(&source) {
            payload_warning(result, &error.to_string());
        }
    }
    payload["count"] = json!(results.len());
}

fn payload_warning(result: &mut Value, error: &str) {
    result["evidence_warning"] = json!(error);
}

fn fair_limit(results: Vec<Value>, limit: usize) -> Vec<Value> {
    let mut buckets: Vec<(String, std::collections::VecDeque<Value>)> = Vec::new();
    for result in results {
        let platform = result["platform"].as_str().unwrap_or("web").to_string();
        if let Some((_, bucket)) = buckets.iter_mut().find(|(key, _)| key == &platform) {
            bucket.push_back(result);
        } else {
            buckets.push((platform, std::collections::VecDeque::from(vec![result])));
        }
    }
    let mut out = Vec::new();
    while out.len() < limit {
        let previous = out.len();
        for (_, bucket) in &mut buckets {
            if let Some(result) = bucket.pop_front() {
                out.push(result);
            }
            if out.len() == limit {
                break;
            }
        }
        if previous == out.len() {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn result_budget_keeps_each_requested_channel_visible() {
        let results = vec![
            json!({"platform":"bilibili","n":1}),
            json!({"platform":"bilibili","n":2}),
            json!({"platform":"xiaohongshu","n":1}),
            json!({"platform":"xiaohongshu","n":2}),
            json!({"n":1}),
        ];
        let limited = fair_limit(results, 4);
        assert_eq!(
            limited
                .iter()
                .map(|v| v["platform"].as_str().unwrap_or("web"))
                .collect::<Vec<_>>(),
            vec!["bilibili", "xiaohongshu", "web", "bilibili"]
        );
    }
}
