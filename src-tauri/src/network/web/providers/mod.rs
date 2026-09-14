//! 内置搜索 provider 集合与工厂注册入口。
//!
//! 每个 provider 是独立可替换的供应商实现；`*_factory` 是注册进缝隙
//! `ProviderRegistry` 的构建函数（从配置快照构建，每次搜索执行时调用）。
//!
//! 引擎选择指南（工具描述与用户文档共用同一事实）：
//! - `duckduckgo`：零配置 HTML 爬取，始终可用的兜底
//! - `searxng`：自部署元搜索引擎（需 base_url）
//! - `tavily`：LLM 优化搜索 API（需 api_key）
//! - `bing`：微软官方 API v7，国内直连（需 api_key）
//! - `deepseek`：DeepSeek 官方原生搜索（Anthropic 兼容 Messages API +
//!   `web_search_20250305` server tool）。一次搜索 = 一次模型调用，
//!   引用级摘要质量最高，消耗 DeepSeek API 额度

pub(crate) mod bing;
pub(crate) mod deepseek;
pub(crate) mod duckduckgo;
pub(crate) mod searxng;
pub(crate) mod tavily;
pub mod util;

pub use bing::bing_factory;
pub use deepseek::deepseek_factory;
pub use duckduckgo::duckduckgo_factory;
pub use searxng::searxng_factory;
pub use tavily::tavily_factory;
