//! 内置搜索 provider 集合与工厂注册入口。
//!
//! 每个 provider 是独立可替换的供应商实现；`*_factory` 是注册进缝隙
//! `ProviderRegistry` 的构建函数（从配置快照构建，每次搜索执行时调用）。
//!
//! 引擎选择指南（工具描述与用户文档共用同一事实）：
//! - `duckduckgo`：零配置 HTML 爬取，无需凭据；目标站点常返回反机器人验证页，
//!   成功率不稳定，适合当末位兜底而非首选
//! - `searxng`：自部署元搜索引擎（需 base_url）
//! - `tavily`：LLM 优化搜索 API（需 api_key）
//! - `deepseek`：DeepSeek 官方原生搜索（Anthropic 兼容 Messages API +
//!   `web_search_20250305` server tool）。一次搜索 = 一次模型调用，
//!   可返回引用摘录，消耗 DeepSeek API 额度

pub(crate) mod deepseek;
pub(crate) mod duckduckgo;
pub(crate) mod searxng;
pub(crate) mod tavily;
pub mod util;

pub use deepseek::deepseek_factory;
pub use duckduckgo::duckduckgo_factory;
pub use searxng::searxng_factory;
pub use tavily::tavily_factory;

pub(crate) mod search_api;
pub use search_api::{exa_factory, perplexity_factory, openai_factory, xai_factory, anthropic_factory};
