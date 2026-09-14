//! 多平台内容源 — 统一候选结构 + 适配器协议
//!
//! 适配器分层：
//! - 匿名公开 API（后端直连，零登录）：bilibili（搜索/热门）、bangumi（v0 搜索/榜单）、
//!   v2ex（官方 API 热门/最新）、weibo（游客态热搜/搜索）、reddit（匿名 JSON，rdt-cli 优先）
//! - 登录态被动采集（browser_signals）：经浏览器桥在用户已登录页面同源 fetch，
//!   读取观看历史等画像信号（不导航、不劫持标签页）
//! - 登录态 cookie 重放（x）：扩展回传 auth_token/ct0，服务端驱动 twitter-cli
//! - 隔离任务 tab 自动发现（task_tabs）：小红书/抖音/知乎后台静默采集
//!   （inactive 标签 + 自动关闭，不触碰用户正在看的标签页）

use async_trait::async_trait;

use super::bilibili::VideoInfo;

pub mod bangumi;
pub mod browser_signals;
pub mod reddit;
pub mod task_tabs;
pub mod v2ex;
pub mod weibo;
pub mod x;

/// 统一内容候选（各平台归一化后的形态，进入评估流水线）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ContentCandidate {
    /// 平台标识：bilibili / bangumi / v2ex / zhihu / xiaohongshu / ...
    pub platform: String,
    /// 平台内容 ID（bvid / subject_id / topic_id）
    pub content_id: String,
    pub title: String,
    pub description: String,
    /// 作者（UP 主 / V2EX 用户名 / 条目无作者时为空）
    pub author: String,
    pub url: String,
    pub cover_url: String,
    pub duration_secs: u64,
    pub view_count: u64,
    pub like_count: u64,
    /// 发布时间（Unix 秒；缺失为 0）
    pub pubdate: i64,
    /// 发现来源：search:{query} / hot / ranked / latest / history
    pub source: String,
}

impl ContentCandidate {
    pub fn from_bilibili(v: &VideoInfo) -> Self {
        Self {
            platform: "bilibili".to_string(),
            content_id: v.bvid.clone(),
            title: v.title.clone(),
            description: v.description.clone(),
            author: v.up_name.clone(),
            url: v.url(),
            cover_url: v.cover_url.clone(),
            duration_secs: v.duration_secs,
            view_count: v.view_count,
            like_count: v.like_count,
            pubdate: v.pubdate,
            source: v.source.clone(),
        }
    }

    /// 唯一键（平台 + 内容 ID），用于跨源去重
    pub fn key(&self) -> String {
        format!("{}:{}", self.platform, self.content_id)
    }
}

/// 内容源适配器：统一的发现入口
///
/// - `search`：按查询词定向发现（画像兴趣 / 探针域驱动）
/// - `popular`：无查询词的热门/榜单发现（画像锚点不足时的兜底供给）
#[async_trait]
pub trait SourceAdapter: Send + Sync {
    /// 平台标识（与 ContentCandidate.platform 一致）
    fn platform(&self) -> &'static str;
    async fn search(&self, query: &str, limit: usize) -> Vec<ContentCandidate>;
    async fn popular(&self, limit: usize) -> Vec<ContentCandidate>;
}
