// Exercise production unified search/fetch routing with deterministic adapter endpoints.
import { copyFile, mkdir, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
const dir = path.join(root, 'tmp', 'unified-web-tests');
const src = name => path.join(root, 'src-tauri', 'src', name).replaceAll('\\', '/');
await mkdir(path.join(dir, 'src'), { recursive: true });
await copyFile(path.join(root, 'src-tauri', 'Cargo.lock'), path.join(dir, 'Cargo.lock'));
await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-unified-web-tests"
version="0.1.0"
edition="2021"
[dependencies]
async-trait="0.1"
serde={version="1",features=["derive"]}
serde_json="1"
url="2"
once_cell="1"
regex="1"
parking_lot="0.12"
chrono={version="0.4",features=["serde"]}
tokio={version="1",features=["macros","rt-multi-thread","time"]}
futures="0.3"
`);
await writeFile(path.join(dir, 'src', 'lib.rs'), `#![allow(dead_code)]
extern crate url as tauri;
#[cfg(test)] use serde_json::json;
#[derive(Default)] struct State { calls:Vec<String>, fail:bool }
static STATE:once_cell::sync::Lazy<parking_lot::Mutex<State>>=once_cell::sync::Lazy::new(|| parking_lot::Mutex::new(State::default()));
fn reset(fail:bool) { *STATE.lock()=State { fail,..Default::default() }; }
pub mod brain { pub mod coding_agent { #[derive(Clone)] pub struct CodingImage; } }
pub mod config { pub struct WebSearchConfig { pub max_results:u32 } }
pub mod network {
 pub mod web {
  #[path="${src('network/web/types.rs')}"] mod types; pub use types::*;
  pub fn read_search_config() -> (Option<crate::config::WebSearchConfig>,Option<String>) { (None,None) }
  pub struct WebSearchService;
  impl WebSearchService {
   pub fn shared() -> &'static Self { &Self }
   pub async fn search(&self,r:&WebSearchRequest,_:Option<&crate::config::WebSearchConfig>,_:Option<&str>) -> Result<WebSearchResult,WebError> {
    crate::STATE.lock().calls.push(format!("web:{}",r.query));
    let mut source=WebSearchSource::new("https://example.com/article"); source.title=Some("网页标题".into());
    Ok(WebSearchResult { sources:vec![source],..Default::default() })
   }
  }
  pub mod providers { pub mod util {
   pub fn source_id(url:&str) -> String { use std::hash::{Hash,Hasher}; let mut h=std::collections::hash_map::DefaultHasher::new(); url.hash(&mut h); format!("web_{}",h.finish()) }
   pub fn domain_matches(url:&str,domains:&[String]) -> bool { tauri::Url::parse(url).ok().and_then(|u|u.host_str().map(str::to_string)).is_some_and(|h|domains.iter().any(|d|h==*d||h.ends_with(&format!(".{d}")))) }
  } }
 }
 pub mod url_fetcher {
  pub struct FetchedPage { pub url:String,pub title:String,pub text:String,pub content_type:String,pub retrieved_at:String,pub published_at:Option<String>,pub links:Vec<()>,pub images:Vec<()>,pub cached:bool,pub truncated:bool,pub raw_pdf:Option<()>,pub raw_image:Option<()> }
 }
 pub mod web_evidence {
  use super::{web::WebSearchSource,url_fetcher::FetchedPage};
  pub struct EvidenceStore;
  impl EvidenceStore {
   pub fn for_session(_:&str,_:&str,_:&str) -> Self { Self }
   pub fn save_source(&self,_:&WebSearchSource) -> std::io::Result<()> { Ok(()) }
   pub fn source(&self,_:&str) -> std::io::Result<WebSearchSource> { Err(std::io::Error::other("not saved")) }
   pub fn save_page(&self,_:&FetchedPage) -> std::io::Result<serde_json::Value> { Ok(serde_json::json!({"source_id":"saved"})) }
  }
 }
}
pub mod discovery {
 pub mod sources {
  pub mod bilibili_auth {
   pub fn invalidate_context(_:&str,_:u32) {}
   pub async fn fetch_part_context(_:&str,page:u32) -> u32 { crate::STATE.lock().calls.push(format!("subtitle:p{page}")); page }
  }
  pub mod content_detail {
   #[derive(Default,PartialEq)] pub enum DetailStatus { #[default] Full, Excerpt }
   impl DetailStatus { pub fn as_str(&self) -> &str { "full" } }
   #[derive(Default)] pub struct ContentDetail { pub body:Option<String>,pub comments:Vec<String>,pub status:DetailStatus,pub kind:&'static str,pub hint:Option<String>,pub danmaku_line:Option<String> }
   impl ContentDetail {
    pub fn from_bilibili(page:&u32) -> Self { Self { body:Some(format!("P{page} 字幕原文")),comments:vec!["评论".into()],kind:"subtitle",..Default::default() } }
    pub fn evidence(&self) -> &str { "full_subtitle" }
   }
   pub fn invalidate(_:&str,_:&str) {}
   pub async fn resolve_platform_url(url:&str) -> Option<(String,String,String)> { url.contains("bilibili.com/video/").then(||(url.into(),"bilibili".into(),"BV1234567890".into())) }
   pub async fn fetch(_:&str,_:&str,_:&str) -> ContentDetail { crate::STATE.lock().calls.push("subtitle:p1".into()); ContentDetail::from_bilibili(&1) }
  }
 }
 pub struct Client;
 pub static BILI_CLIENT:Client=Client;
 impl Client { pub async fn video_part(&self,_:&str,_:u32,_:Option<&str>) -> Result<(),String> { Ok(()) } }
}
pub mod tools {
 #[path="${src('tools/content_visibility.rs')}"] pub mod content_visibility;
 #[path="${src('tools/types.rs')}"] pub mod types;
 pub mod builtin {
  #[path="${src('tools/builtin/platform_search_policy.rs')}"] pub mod platform_search_policy;
  #[path="${src('tools/builtin/web_platform_search.rs')}"] pub mod web_platform_search;
  #[path="${src('tools/builtin/web_platform_fetch.rs')}"] pub mod web_platform_fetch;
  #[path="${src('tools/builtin/web_tool_compat.rs')}"] pub mod web_tool_compat;
  #[path="${src('tools/builtin/web_search_tool.rs')}"] pub mod web_search_tool;
  pub mod web_fetch_tool {
   pub fn page_payload(page:crate::network::url_fetcher::FetchedPage,_:&serde_json::Value) -> serde_json::Value { serde_json::json!({"url":page.url,"title":page.title,"text":page.text}) }
   pub async fn focus_page(_:&crate::network::url_fetcher::FetchedPage,_:&str,_:&crate::tools::types::ToolUseContext) -> Result<String,String> { Ok("extracted".into()) }
  }
  pub mod platform_search_tool {
   use crate::tools::types::*;
   pub struct PlatformSearchTool;
   impl PlatformSearchTool { pub async fn call(&self,args:serde_json::Value,_:&ToolUseContext) -> ToolResult {
    let platform=args["platforms"][0].as_str().unwrap(); crate::STATE.lock().calls.push(format!("platform:{platform}"));
    let failures=if crate::STATE.lock().fail { vec![serde_json::json!({"platform":platform,"reason":"未登录"})] } else { vec![] };
    let results=if failures.is_empty() { vec![serde_json::json!({"url":"https://www.bilibili.com/video/BV1234567890","title":"平台原始标题","platform":platform})] } else { vec![] };
    let counts=if failures.is_empty() { serde_json::json!({platform:results.len()}) } else { serde_json::json!({}) };
    ToolResult::standard_success("done",Some(serde_json::json!({"results":results,"per_platform":counts,"failures":failures})))
   } }
  }
  pub mod bilibili_danmaku_tool {
   use crate::tools::types::*;
   pub struct ReadBilibiliDanmakuTool;
   impl ReadBilibiliDanmakuTool { pub async fn call(&self,args:serde_json::Value,_:&ToolUseContext) -> ToolResult {
    crate::STATE.lock().calls.push(format!("danmaku:{}:{}",args["page"],args["offset"]));
    if crate::STATE.lock().fail { ToolResult::standard_error("弹幕请求失败",Some("failed"),None) }
    else { ToolResult::standard_success("done",Some(serde_json::json!({"items":[{"text":"弹幕","time_secs":1.5}],"next_offset":args["offset"].as_u64().unwrap()+1}))) }
   } }
  }
 }
}
#[cfg(test)] mod tests {
 use super::*;
 use tools::{types::{Tool,ToolUseContext},builtin::{web_search_tool::WebSearchTool,web_platform_fetch}};
 #[tokio::test] async fn one_search_entry_routes_platforms_and_optional_web_and_filters_domains() {
  reset(false); let tool=WebSearchTool; let ctx=ToolUseContext::default();
  let args=json!({"query":"摄影","platforms":["bilibili"],"include_web":true,"include_domains":["bilibili.com"]});
  let result=tool.call(args,&ctx).await;
  assert!(tools::content_visibility::search_has_sources("web_search",result.success,result.data.as_ref()));
  assert!(result.success); let payload=&result.data.unwrap()["data"];
  assert_eq!(payload["count"],1); assert_eq!(payload["results"][0]["platform"],"bilibili");
  assert!(payload["results"][0]["source_id"].as_str().unwrap().starts_with("web_"));
  assert!(STATE.lock().calls.contains(&"platform:bilibili".into())); assert!(STATE.lock().calls.contains(&"web:摄影".into()));
 }
 #[tokio::test] async fn ordinary_web_search_keeps_existing_behavior() {
  reset(false); let result=WebSearchTool.call(json!({"query":"Rust"}),&ToolUseContext::default()).await;
  assert!(result.success); assert_eq!(STATE.lock().calls,vec!["web:Rust"]);
 }
 #[tokio::test] async fn failed_platform_remains_visible_when_web_succeeds() {
  reset(true); let result=WebSearchTool.call(json!({"query":"茶","platforms":["xiaohongshu"],"include_web":true}),&ToolUseContext::default()).await;
  assert!(result.success); let data=result.data.unwrap(); assert_eq!(data["data"]["failures"][0]["platform"],"xiaohongshu");
  assert_eq!(data["data"]["partial_failure"],true);
 }
 #[tokio::test] async fn danmaku_only_does_not_fetch_body_and_has_independent_pagination() {
  reset(false); let args=json!({"include":["danmaku"],"danmaku_offset":7});
  let store=network::web_evidence::EvidenceStore;
  let result=web_platform_fetch::read("https://www.bilibili.com/video/BV1234567890?p=2",&args,&ToolUseContext::default(),&store).await.unwrap();
  assert!(result.success); let data=result.data.unwrap(); assert_eq!(data["danmaku"]["next_offset"],8);
  assert_eq!(data["status"],"danmaku_only"); assert_eq!(STATE.lock().calls,vec!["danmaku:2:7"]);
 }
 #[tokio::test] async fn missing_danmaku_preserves_the_read_subtitle() {
  reset(true); let args=json!({"include":["body","danmaku"]});
  let result=web_platform_fetch::read("https://www.bilibili.com/video/BV1234567890",&args,&ToolUseContext::default(),&network::web_evidence::EvidenceStore).await.unwrap();
  assert!(result.success); let data=result.data.unwrap(); assert!(data["text"].as_str().unwrap().contains("字幕原文"));
  assert_eq!(data["danmaku"]["status"],"unavailable");
 }
 #[tokio::test] async fn platform_failures_are_errors_and_unsupported_sections_are_not_ignored() {
  reset(true); let result=WebSearchTool.call(json!({"query":"茶","platforms":["xiaohongshu"]}),&ToolUseContext::default()).await;
  assert!(!result.success);
  let result=web_platform_fetch::read("https://example.com/article",&json!({"include":["danmaku"]}),&ToolUseContext::default(),&network::web_evidence::EvidenceStore).await.unwrap();
  assert!(!result.success);
 }
 #[tokio::test] async fn historical_platform_call_dispatches_through_the_new_entry() {
  reset(false);
  let adapter=tools::builtin::web_tool_compat::wrap("platform_search",std::sync::Arc::new(WebSearchTool));
  assert_eq!(adapter.name(),"web_search");
  let result=adapter.call(json!({"query":"摄影","limit":3}),&ToolUseContext::default()).await;
  assert!(result.success); assert_eq!(STATE.lock().calls,vec!["platform:bilibili"]);
 }
}
`);
const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1', '--', '--test-threads=1'], {
  cwd: root, stdio: 'inherit', env: { ...process.env, CARGO_INCREMENTAL: '0', CARGO_PROFILE_DEV_DEBUG: '0' },
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
