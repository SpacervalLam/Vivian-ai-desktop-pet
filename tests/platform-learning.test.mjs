// Exercise production source ordering, background platform-search routing, reading corpus,
// and acquisition orchestration. Network/browser/LLM endpoints are deterministic fakes.
import { copyFile, mkdir, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const dir = path.join(root, 'tmp', 'platform-learning-tests');
const src = file => path.join(root, 'src-tauri', 'src', file).replaceAll('\\', '/');
await mkdir(path.join(dir, 'src'), { recursive: true });
await copyFile(path.join(root, 'src-tauri', 'Cargo.lock'), path.join(dir, 'Cargo.lock'));
await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-platform-learning-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
sha2="0.10"
url="2"
once_cell="1"
parking_lot="0.12"
chrono={version="0.4",features=["serde"]}
tracing="0.1"
tokio={version="1",features=["rt-multi-thread","macros","time"]}
`);
await writeFile(path.join(dir, 'src', 'lib.rs'), `#![allow(dead_code)]
extern crate url as tauri;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
#[derive(Default)] struct State { logs:Vec<String>, logged_in:bool, readable:bool }
static STATE: Lazy<Mutex<State>> = Lazy::new(|| Mutex::new(State::default()));
fn reset(logged_in:bool, readable:bool) { *STATE.lock()=State { logged_in,readable,..Default::default() }; }
fn body() -> String { "这是一段实际读取的资料原文，学习知识与理解表达方式需要自己的判断。".repeat(4) }
pub mod config { pub struct WebSearchConfig; }
pub mod network {
  pub mod web {
    #[path="${src('network/web/types.rs')}"] mod types;
    pub use types::*;
    pub struct WebSearchService;
    impl WebSearchService {
      pub fn shared() -> &'static Self { &Self }
      pub async fn search(&self,_:&WebSearchRequest,_:Option<&crate::config::WebSearchConfig>,_:Option<&str>) -> Result<WebSearchResult,String> {
        crate::STATE.lock().logs.push("search:web".into());
        let mut source=WebSearchSource::new("https://example.com/article"); source.raw_content=Some(crate::body());
        Ok(WebSearchResult { sources:vec![source],..Default::default() })
      }
    }
  }
  pub mod url_fetcher {
    pub struct Page { pub text:String }
    pub async fn fetch_page(_: &str) -> Result<Page,String> { Ok(Page { text:crate::body() }) }
  }
}
pub mod browser_bridge {
  #[path="${src('browser_bridge/sites.rs')}"] pub mod sites;
  pub mod tools {
    pub struct Bridge;
    pub fn global_bridge() -> Option<Bridge> { Some(Bridge) }
    impl Bridge {
      pub fn is_connected(&self) -> bool { true }
      pub fn platform_status(&self) -> Option<((),std::collections::HashMap<String,bool>)> {
        Some(((),[("xiaohongshu".into(),crate::STATE.lock().logged_in)].into()))
      }
      pub async fn request_tool(&self,_:&str,args:&serde_json::Value,_:std::time::Duration,_:Option<&str>) -> Result<String,String> {
        assert!(args.get("url").is_some(), "Direct bridge calls need resolved URLs");
        crate::STATE.lock().logs.push(format!("browser:{}",args["url"].as_str().unwrap()));
        Ok(serde_json::json!([{"id":"note1","title":"读书笔记","url":"https://www.xiaohongshu.com/explore/note1?xsec_token=access","desc":"仅是搜索摘要"}]).to_string())
      }
    }
  }
}
pub mod discovery {
  #[path="${src('discovery/learning_policy.rs')}"] pub mod learning_policy;
  pub fn parse_json_tolerant(text:&str) -> Option<serde_json::Value> {
    let value:serde_json::Value=serde_json::from_str(text).ok()?;
    if let Some(text)=value.as_str() { serde_json::from_str(text).ok() } else { Some(value) }
  }
  pub struct BiliClient;
  pub static BILI_CLIENT:BiliClient=BiliClient;
  impl BiliClient {
    pub async fn search(&self,_:&str,_:u32,_:u32) -> Result<Vec<sources::ContentCandidate>,String> {
      crate::STATE.lock().logs.push("search:bilibili".into());
      Ok(vec![sources::ContentCandidate { platform:"bilibili".into(),content_id:"BV1234567890".into(),title:"视频".into(),url:"https://www.bilibili.com/video/BV1234567890".into(),..Default::default() }])
    }
  }
  pub mod engine { pub async fn admit_candidates(_:&str,_:Vec<super::sources::ContentCandidate>) -> usize { 0 } }
  pub mod sources {
    #[derive(Clone,Default)] pub struct ContentCandidate {
      pub platform:String,pub content_id:String,pub title:String,pub description:String,pub author:String,pub url:String,pub cover_url:String,
      pub duration_secs:u64,pub view_count:u64,pub like_count:u64,pub pubdate:i64,pub source:String
    }
    impl ContentCandidate { pub fn from_bilibili(value:&Self) -> Self { value.clone() } }
    #[path="${src('discovery/sources/task_tabs.rs')}"] pub mod task_tabs;
    pub mod content_detail {
      pub struct Detail { pub body:Option<String>,pub comments:Vec<String>,pub danmaku_line:Option<String> }
      pub fn platform_and_id(url:&str) -> Option<(String,String)> {
        if url.contains("bilibili.com") { Some(("bilibili".into(),"bv".into())) }
        else if url.contains("xiaohongshu.com") { Some(("xiaohongshu".into(),"note1".into())) } else { None }
      }
      pub async fn fetch(platform:&str,_:&str,_:&str) -> Detail {
        let mut state=crate::STATE.lock(); state.logs.push(format!("read:{platform}"));
        Detail { body:state.readable.then(crate::body),comments:vec!["观众看法".into()],danmaku_line:None }
      }
    }
  }
}
pub mod persona {
  #[path="${src('persona/reading_corpus.rs')}"] pub mod reading_corpus;
  #[path="${src('persona/self_profile.rs')}"] pub mod self_profile;
  pub mod self_development {
    pub struct ReadingExperience { pub url:String,pub body:String }
    pub fn spawn_from_reading(char_id:&str,readings:Vec<ReadingExperience>) {
      crate::STATE.lock().logs.push(format!("reflect:{char_id}:{}",readings.len()));
    }
  }
}
#[path="${src('presence/platform_learning.rs')}"] pub mod platform_learning;
#[cfg(test)] mod tests {
  use super::*;
  #[tokio::test] async fn vivian_reads_bilibili_before_considering_web_search() {
    reset(true,true);
    let sources=platform_learning::topic_sources("vivian","摄影",5,&config::WebSearchConfig,None).await;
    assert!(sources[0].url.contains("bilibili.com"));
    assert_eq!(STATE.lock().logs,vec!["search:bilibili","read:bilibili"]);
    platform_learning::reflect_on_sources("vivian",&sources);
    assert!(STATE.lock().logs.contains(&"reflect:vivian:1".into()));
  }
  #[tokio::test] async fn nana_searches_logged_in_xiaohongshu_with_an_encoded_url() {
    reset(true,true);
    let sources=platform_learning::topic_sources("nana","茶 & 花",5,&config::WebSearchConfig,None).await;
    assert!(sources[0].url.contains("xsec_token=access"));
    let state=STATE.lock();
    assert!(state.logs[0].starts_with("browser:https://www.xiaohongshu.com/search_result?"));
    assert!(state.logs[0].contains("%26"));
    assert_eq!(state.logs[1],"read:xiaohongshu");
    assert!(!state.logs.contains(&"search:web".into()));
  }
  #[tokio::test] async fn missing_login_falls_back_without_claiming_platform_was_read() {
    reset(false,true);
    let sources=platform_learning::topic_sources("nana","茶",5,&config::WebSearchConfig,None).await;
    assert_eq!(sources[0].url,"https://example.com/article");
    assert_eq!(STATE.lock().logs,vec!["search:web"]);
  }
  #[tokio::test] async fn title_only_platform_hits_do_not_block_actual_reading_elsewhere() {
    reset(true,false);
    let sources=platform_learning::topic_sources("vivian","摄影",5,&config::WebSearchConfig,None).await;
    assert_eq!(sources[0].url,"https://example.com/article");
    assert_eq!(STATE.lock().logs,vec!["search:bilibili","read:bilibili","search:web"]);
  }
  #[test] fn character_corpora_are_independent_and_survive_serialization() {
    let mut vivian=persona::reading_corpus::ReadingCorpus::default();
    let nana=persona::reading_corpus::ReadingCorpus::default();
    assert!(vivian.observe("https://www.bilibili.com/video/BV1234567890",&body(),1.0));
    let restored:persona::reading_corpus::ReadingCorpus=serde_json::from_str(&serde_json::to_string(&vivian).unwrap()).unwrap();
    assert_eq!(restored.items.len(),1); assert!(nana.items.is_empty());
  }
}
`);
const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1', '--', '--test-threads=1'], {
  cwd: root, stdio: 'inherit', env: { ...process.env, CARGO_INCREMENTAL: '0', CARGO_PROFILE_DEV_DEBUG: '0' },
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
